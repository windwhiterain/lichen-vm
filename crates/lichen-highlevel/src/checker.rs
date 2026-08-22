//! The builder/checker: compiles an [`ExprTable`] into a lowlevel [`Module`]
//! where the runtime *is* the typechecker.
//!
//! Every expression compiles to a **recursive pair** `[value, type]` — a
//! 2-element array node whose elements are the value and its type, where the
//! type slot is itself such a pair.  Every type spine bottoms out at the
//! canonical universe `K = [Type, ↺]` — the self-referential `Type : Type`
//! node — so a literal is `[5, [int, K]]`, a function value
//! `[f, [[in, out], [FunctionType, K]]]`, a tuple
//! `[[1, 2], [[int, int], [TupleType, K]]]`, and an array type
//! `[[int, 3], [ArrayType, K]]` (instance element 0: the type shared by all
//! elements, element 1: the length).  A function parameter is such
//! a pair (`f(x: int)` maps to the parameter `[x, int-type]`), and a call
//! passes the argument's pair, so the apply-time unification
//! `unify(cloned_param, argument)` matches value-to-value and
//! **type-to-type** — that is the function parameter type check, executed
//! by the VM.
//!
//! The checker only *constructs* (pairs, type expressions, the universe)
//! and issues the unifies that have no apply to express them: annotations,
//! a binary operator's operand-`Int` checks, and the function-ness guard
//! (so applying a non-function is a reported error, not a runtime panic);
//! kinding (a type expression's own type must
//! be a kind) is structural and only fails for concrete non-kinds, like a
//! literal in type position.  It then runs the definition pass so the
//! apply-time checks fire; failures land in [`Module::unify_errors`], which
//! is the checker's error channel.

use std::collections::{HashMap, HashSet};

use lichen_lowlevel::{
    BlockId, Function, Module, NodeId, Operation, Operator, UnifyError, Value, is_unbound,
};

use crate::diagnostic::{DiagKind, DiaryEntry};
use crate::ir::{BinOp, Constant, ExprId, ExprKind, IR, Span};
use crate::program::{HighOperator, HighProgram, HighValue};

/// Term position or type position.  The same expression can be a value in
/// one place and a type in another (first-class types); in type position its
/// value *is* the type and kinding (`ty : Type`) is enforced.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Term,
    Type,
}

/// A parameter in scope: the parameter pair `[value, type]` plus its type
/// cell.  Uses reference the pair (so the apply's clone always includes it —
/// the lowlevel's `function_apply` only runs the argument unify when the
/// parameter was cloned); a value use extracts element 0 via
/// `Index(pair, 0)`.
#[derive(Clone, Copy)]
struct Binding {
    term: NodeId,
    ty: NodeId,
}

pub struct Checker {
    ir: IR,
    module: Module<HighProgram>,
    current_block: BlockId,
    scopes: Vec<HashMap<ExprId, Binding>>,
    /// The blocks created while compiling each enclosing function's body —
    /// one frame per [`Checker::check_lam`] on the stack, innermost last.
    /// A function's scope is the union of its frame's blocks, so an apply's
    /// clone instantiates a nested function value (with its captures) as a
    /// fresh closure instead of referencing the template's never-bound
    /// parameter cells.
    body_blocks: Vec<Vec<BlockId>>,
    /// The compiled pair node `[value, type]` per expression.
    term: Vec<Option<NodeId>>,
    /// Element 0 of the pair (the value).  `None` for call results, whose
    /// value is only known at runtime — built lazily as
    /// `Index(pair, 0)` via [`Checker::value_of`] when the value is needed.
    val: Vec<Option<NodeId>>,
    /// Element 1 of the pair (the type).
    ty: Vec<Option<NodeId>>,
    /// The checker's own unification sequence, attributed for diagnostics:
    /// each entry records which `unify_errors` entry (if any) the unify
    /// produced, plus its span and check kind.
    diary: Vec<DiaryEntry>,
    /// The arrow nodes built by [`Checker::check_lam`] — the type printer
    /// renders these `[param, body]` shapes as `param → body`.
    arrows: HashSet<NodeId>,
    /// The value nodes of recursive bindings' functions, collected by
    /// [`Checker::check_lam`].  [`Checker::build`] deep-evaluates them before
    /// the definition pass (proving them concrete), so a recursive reference
    /// stays in place instead of cloning the function per application.
    recursive_func_nodes: Vec<NodeId>,
    int_marker: NodeId,
    type_marker: NodeId,
    function_type_marker: NodeId,
    tuple_type_marker: NodeId,
    array_type_marker: NodeId,
    /// The shared `[int, Type]` type expression every literal's pair carries.
    int_type: NodeId,
    /// The canonical universe `[Type, ↺]` — the self-referential `Type : Type`.
    type_expr: NodeId,
}

/// The result of building a program: the compiled Module plus the checker's
/// records.  `ok` is false when any unification failed (`unify_errors` is
/// non-empty); rendering diagnostics from those is future work.
pub struct Build {
    pub ir: IR,
    pub module: Module<HighProgram>,
    pub term: Vec<Option<NodeId>>,
    pub val: Vec<Option<NodeId>>,
    pub ty: Vec<Option<NodeId>>,
    pub root_term: NodeId,
    pub root_val: NodeId,
    pub root_ty: NodeId,
    pub int_marker: NodeId,
    pub type_marker: NodeId,
    /// The shared `[int, Type]` type expression.
    pub int_type: NodeId,
    /// The canonical universe `[Type, ↺]`.
    pub type_expr: NodeId,
    /// The checker's attributed unification sequence (see [`DiaryEntry`]).
    pub diary: Vec<DiaryEntry>,
    /// Arrow nodes; read by the diagnostics.
    pub arrows: HashSet<NodeId>,
    pub ok: bool,
}

impl Checker {
    pub fn build(ir: IR) -> Build {
        let mut module = Module::new();
        // The lowlevel's default application guard (10k nested calls) sits
        // below what a thread stack survives once the checker's per-call
        // machinery (clone, unify, deep pass) is on the stack — a
        // non-terminating recursion would overflow the stack before the
        // guard fires.  Lower it so the guard panics cleanly; legitimate
        // recursion (fib, countdown) nests far below this.
        module.apply_depth_limit = 500;
        let root_block = module.add_block(None);
        let n = ir.expr.len();
        let mut checker = Checker {
            ir,
            module,
            current_block: root_block,
            scopes: Vec::new(),
            body_blocks: Vec::new(),
            term: vec![None; n],
            val: vec![None; n],
            ty: vec![None; n],
            diary: Vec::new(),
            arrows: HashSet::new(),
            recursive_func_nodes: Vec::new(),
            int_marker: NodeId::default(),
            type_marker: NodeId::default(),
            function_type_marker: NodeId::default(),
            tuple_type_marker: NodeId::default(),
            array_type_marker: NodeId::default(),
            int_type: NodeId::default(),
            type_expr: NodeId::default(),
        };
        checker.install_constants();
        // Prove the canonical structures concrete before the definition
        // pass, so the apply clone machinery references them in place
        // instead of cloning them: cloning the self-referential universe
        // `[Type, ↺]` would create a fresh self-loop that unification cannot
        // equate with the canonical one (the path guard would report a
        // conflict).
        checker.module.evaluate_node_deep(checker.type_expr, None);
        checker.module.evaluate_node_deep(checker.int_type, None);
        let root = checker.ir.root;
        let root_term = checker.check_expr(root, Role::Term);
        let root_ty = checker.ty[root].expect("the root expression must have a type");
        // Prove the recursive bindings' function values concrete before the
        // definition pass: a recursive reference (the apply in the body)
        // then stays in place, so every recursion level re-applies the
        // template — whose own parameter is never deep-evaluated and is
        // cloned fresh per level.  Without this, the deep pass evaluates the
        // parameter clone of the first application, a second application
        // reuses the already-bound clone instead of cloning it fresh, and
        // the argument unify conflicts (the recursion cannot descend).
        for &func_node in &checker.recursive_func_nodes {
            checker.module.evaluate_node_deep(func_node, None);
        }
        // The definition pass: run the program so the apply-time type checks
        // fire.  Each function body runs once first — its apply-time checks
        // fire even when the function is never applied, and a body ending in
        // a call resolves its result cell before the root pass walks the
        // function's type spine (which would otherwise read the cell while
        // it is still unbound).  The order against the root pass is
        // irrelevant: reads alias their target cells (see the lowlevel Index
        // arm), so bindings propagate class-wise however they happen.
        // Skipped when the checker-side unifies (annotations, kinding,
        // guards) already failed — the graph may then hit a non-function
        // apply, which the runtime panics on.
        // The definition pass: run the program so the apply-time type checks
        // fire.  Each function body runs once first — its apply-time checks
        // fire even when the function is never applied, and a body ending in
        // a call resolves its result cell before the root pass walks the
        // function's type spine (which would otherwise read the cell while
        // it is still unbound).  The order against the root pass is
        // irrelevant: reads alias their target cells (see the lowlevel Index
        // arm), so bindings propagate class-wise however they happen.
        // Skipped when the checker-side unifies (annotations, kinding,
        // guards) already failed — the graph may then hit a non-function
        // apply, which the runtime panics on.
        if checker.module.unify_errors.is_empty() {
            let functions: Vec<lichen_lowlevel::FunctionId> = checker.module.functions.keys().collect();
            for function in functions {
                let ret = checker.module.functions[function].r#return;
                // Only bodies whose apply-time checks can fire need the pass.
                // A body that is a function value or a plain annotation has
                // no apply of its own, and deep-evaluating it would prove
                // the body concrete — the clone rule would then reference it
                // in place and the parameter check would never run.
                if self_subtree_contains_apply(&checker.module, ret) {
                    checker.module.evaluate_node_deep(ret, None);
                }
            }
        }
        if checker.module.unify_errors.is_empty() {
            checker.module.evaluate_node_deep(root_term, None);
        }
        let ok = checker.module.unify_errors.is_empty() && checker.module.eval_errors.is_empty();
        let root_val = checker.value_of(root);
        // The definition pass above evaluates the program's applies; each
        // apply's runtime evaluation syncs its result cell with the return
        // pair, so an unannotated call's root type is the return type by the
        // time the pass finishes.  A polymorphic template's lazy result
        // leaves the cell unbound — a generic function's ends stay
        // underdetermined, which is not an error.
        Build {
            ir: checker.ir,
            module: checker.module,
            term: checker.term,
            val: checker.val,
            ty: checker.ty,
            root_term,
            root_val,
            root_ty,
            int_marker: checker.int_marker,
            type_marker: checker.type_marker,
            int_type: checker.int_type,
            type_expr: checker.type_expr,
            diary: checker.diary,
            arrows: checker.arrows,
            ok,
        }
    }

    /// The type constants as plain nodes in the root block (types are
    /// first-class values — they live in the runtime graph), plus the two
    /// canonical structures: the universe `K = [Type, ↺]` whose type slot is
    /// itself (`Type : Type` closes every type spine) and the shared int
    /// type expression `[int, K]` every literal's pair carries.
    fn install_constants(&mut self) {
        let root = self.current_block;
        self.int_marker = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::TypeInt)));
        self.type_marker = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::TypeType)));
        self.function_type_marker =
            self.module
                .add_node(root, None, Some(Value::Ext(HighValue::TypeFunction)));
        self.tuple_type_marker =
            self.module
                .add_node(root, None, Some(Value::Ext(HighValue::TypeTuple)));
        self.array_type_marker =
            self.module
                .add_node(root, None, Some(Value::Ext(HighValue::TypeArray)));
        // `K = [Type, K]`: allocate the node, then point its type slot at
        // itself.  The self-loop is cut by the lowlevel deep-evaluation
        // cycle guard whenever the definition pass reaches it.
        let universe = self.module.add_node(root, None, None);
        let slice = self.module.blocks[root]
            .arena
            .alloc_slice_copy(&[self.type_marker, universe]);
        self.module.nodes[universe].value = Some(Value::Array(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )));
        self.type_expr = universe;
        self.int_type = self.array_node(root, &[self.int_marker, self.type_expr]);
    }

    // --- allocation ------------------------------------------------------

    /// A fresh, unbound type cell (a parameterized node — evaluating it
    /// yields the lazy marker, never a panic).
    fn fresh_cell(&mut self) -> NodeId {
        self.module
            .add_node(self.current_block, None, Some(Value::Parameterized))
    }

    fn array_node(&mut self, block: BlockId, ids: &[NodeId]) -> NodeId {
        let slice = self.module.blocks[block].arena.alloc_slice_copy(ids);
        self.module.add_node(
            block,
            None,
            Some(Value::Array(std::ptr::slice_from_raw_parts(
                slice.as_ptr(),
                slice.len(),
            ))),
        )
    }

    fn op_node(
        &mut self,
        block: BlockId,
        operator: Operator<HighProgram>,
        operand: Option<NodeId>,
    ) -> NodeId {
        self.module
            .add_node(block, Some(Operation { operator, operand }), None)
    }

    /// A pair node `[value, type]` built around two already-compiled nodes.
    fn pair_of(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        self.array_node(self.current_block, &[value, ty])
    }

    /// The kind expression of a compound type: `[marker, Type]`, where
    /// `marker` is one of the kind markers (`FunctionType`, `TupleType`,
    /// `ArrayType`).
    fn kind_expr(&mut self, block: BlockId, marker: NodeId) -> NodeId {
        self.array_node(block, &[marker, self.type_expr])
    }

    /// The children of a variadic expression (`Tuple`, `TypeTuple`,
    /// `Array`, `TypeStruct`).
    fn range_children(&self, e: ExprId) -> Vec<ExprId> {
        let range = match self.ir[e].kind {
            ExprKind::Tuple(range)
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::TypeStruct(range) => range,
            _ => unreachable!("expected a variadic expression kind"),
        };
        self.ir.children[range.start as usize..range.end as usize].to_vec()
    }

    /// The value of an expression: element 0 of its pair.  For expressions
    /// whose pair is a static node (literals, variables, lambdas) this is
    /// stored; for call results it is extracted at runtime with
    /// `Index(pair, 0)` and memoized.
    fn value_of(&mut self, e: ExprId) -> NodeId {
        if let Some(value) = self.val[e] {
            return value;
        }
        let pair = self.term[e].expect("expression must be compiled");
        let zero = self
            .module
            .add_node(self.current_block, None, Some(Value::USize(0)));
        let operands = self.array_node(self.current_block, &[pair, zero]);
        let index = self.op_node(self.current_block, Operator::Index, Some(operands));
        self.val[e] = Some(index);
        index
    }

    // --- the check -------------------------------------------------------

    /// A checker-issued unification, diary-attributed: records which error
    /// (if any) it produced, with the span and check kind that drive the
    /// diagnostic's wording and expected/found direction.
    fn check_unify(&mut self, a: NodeId, b: NodeId, span: Option<Span>, kind: DiagKind) {
        let before = self.module.unify_errors.len();
        self.module.unify(a, b);
        if self.module.unify_errors.len() > before {
            self.diary.push(DiaryEntry {
                error_index: before,
                a,
                b,
                span,
                kind,
            });
        }
    }

    fn check_expr(&mut self, e: ExprId, role: Role) -> NodeId {
        // The variant decides the compilation (a `Tuple` is a value, a
        // `TypeTuple` a type expression — the frontend picks per syntactic
        // role); the role only gates kinding.
        let node = self.check_term(e);
        if role == Role::Type {
            self.check_kinding(e);
        }
        node
    }

    /// Kinding: a type expression's own type must be a kind — the universe
    /// `K` itself (atomic types: `int : Type`) or a kind expression
    /// `[Kind, K]` (compound types: `[in, out] : FunctionType : Type`).  An
    /// unbound type (a parameter, a call result) defers to the runtime; a
    /// concrete non-kind — a literal in type position, e.g. `1 : 3` — is a
    /// reported error rather than a check that silently passes.
    fn check_kinding(&mut self, e: ExprId) {
        let ty = self.ty[e].unwrap();
        if self.is_kind(ty) || is_unbound(self.module.nodes[ty].value) {
            return;
        }
        let error_index = self.module.unify_errors.len();
        self.module.unify_errors.push(UnifyError {
            a: ty,
            b: self.type_expr,
            value_a: self.module.nodes[ty].value,
            value_b: self.module.nodes[self.type_expr].value,
        });
        self.diary.push(DiaryEntry {
            error_index,
            a: ty,
            b: self.type_expr,
            span: self.ir[e].span,
            kind: DiagKind::Kinding,
        });
    }

    /// Whether `node` is a kind: the universe `K` itself, or a kind
    /// expression `[FunctionType | TupleType | ArrayType | TypeId(n), K]`.
    /// An unevaluated kind marker (the pending `Fresh` call of a struct
    /// type, which has no value until the runtime forces it) defers the
    /// check — it is always concrete by the time the definition pass has
    /// run.
    fn is_kind(&mut self, node: NodeId) -> bool {
        if self.is_universe(node) {
            return true;
        }
        let Some(ids) = self.module.array_ids(node) else {
            return false;
        };
        if ids.len() != 2 {
            return false;
        }
        // Copy the ids out of the slice borrow: the mutating helpers below
        // need `&mut self` while `array_ids` borrows `self.module`.
        let marker = ids[0];
        let kind = ids[1];
        self.is_universe(kind)
            && matches!(
                self.module.nodes[marker].value,
                Some(Value::Ext(
                    HighValue::TypeFunction
                        | HighValue::TypeTuple
                        | HighValue::TypeArray
                        | HighValue::TypeId(_)
                )) | None
            )
    }

    /// Whether the class of `node` is the canonical universe `K`.
    fn is_universe(&mut self, node: NodeId) -> bool {
        self.module.equality_representative(node)
            == self.module.equality_representative(self.type_expr)
    }

    /// Whether `kind` is the kind expression `[marker, K]`.
    fn kind_marker_is(&mut self, kind: NodeId, marker: HighValue) -> bool {
        let Some(ids) = self.module.array_ids(kind) else {
            return false;
        };
        if ids.len() != 2 {
            return false;
        }
        let head = ids[0];
        let tail = ids[1];
        self.module.nodes[head].value == Some(Value::Ext(marker)) && self.is_universe(tail)
    }

    /// Whether `ty` is a concrete function type expression:
    /// `[shape, [FunctionType, K]]`.  The function-ness guard skips these —
    /// only concretely *non*-function types are caught statically.
    fn is_function_type(&mut self, ty: NodeId) -> bool {
        let Some(ids) = self.module.array_ids(ty) else {
            return false;
        };
        if ids.len() != 2 {
            return false;
        }
        let kind = ids[1];
        self.kind_marker_is(kind, HighValue::TypeFunction)
    }

    /// Whether `ty` is a concrete indexable type expression — a tuple type
    /// (`[shape, [TypeTuple, K]]`), an array type (`[shape, [TypeArray,
    /// K]]`), or a struct type (`[shape, [TypeId(n), K]]`, whose shape is the
    /// positional field list).  The index-target guard skips these; only
    /// concretely *non*-indexable types are caught statically.
    fn is_indexable_type(&mut self, ty: NodeId) -> bool {
        let Some(ids) = self.module.array_ids(ty) else {
            return false;
        };
        if ids.len() != 2 {
            return false;
        }
        let kind = ids[1];
        self.kind_marker_is(kind, HighValue::TypeTuple)
            || self.kind_marker_is(kind, HighValue::TypeArray)
            || self.kind_marker_is_type_id(kind)
    }

    /// Whether `kind` is a struct type's kind expression `[TypeId(n), K]` —
    /// the fresh nominal marker, as opposed to the fixed structural markers
    /// [`HighValue::TypeTuple`] / [`HighValue::TypeArray`].  The marker slot
    /// holds the *pending* [`HighOperator::Fresh`] call at check time — its
    /// value, the nominal id, only appears when the definition pass runs the
    /// operator — so an unevaluated Fresh marker counts as a struct kind too.
    fn kind_marker_is_type_id(&mut self, kind: NodeId) -> bool {
        let Some(ids) = self.module.array_ids(kind) else {
            return false;
        };
        if ids.len() != 2 {
            return false;
        }
        let head = ids[0];
        let tail = ids[1];
        self.is_universe(tail)
            && (matches!(
                self.module.nodes[head].value,
                Some(Value::Ext(HighValue::TypeId(_)))
            ) || self.module.nodes[head].operation.is_some_and(|op| {
                matches!(op.operator, Operator::Ext(HighOperator::Fresh))
            }))
    }

    fn check_term(&mut self, e: ExprId) -> NodeId {
        // The IR is a graph: statement bindings pre-resolve every use of a
        // name to the value's own `ExprId`, so one expression may be
        // referenced from several parents (a DAG, not a tree).  Compile each
        // expression once and reuse the compiled pair — recompiling would
        // duplicate fresh state (a struct type's nominal id comes from a
        // per-compilation `Fresh` call), silently breaking the sharing the
        // frontend relies on.
        if let Some(pair) = self.term[e] {
            return pair;
        }
        match self.ir[e].kind {
            ExprKind::Constant(Constant::USize(n)) => {
                let value = self
                    .module
                    .add_node(self.current_block, None, Some(Value::USize(n)));
                let pair = self.pair_of(value, self.int_type);
                self.term[e] = Some(pair);
                self.val[e] = Some(value);
                self.ty[e] = Some(self.int_type);
                pair
            }
            ExprKind::Constant(Constant::TypeType) => {
                // `Type` is the canonical universe node itself — `Type : Type`.
                self.term[e] = Some(self.type_expr);
                self.val[e] = Some(self.type_marker);
                self.ty[e] = Some(self.type_expr);
                self.type_expr
            }
            ExprKind::Constant(Constant::TypeInt) => {
                // The int type constant: `[int, Type]` — its value is the
                // marker and its type is the universe.
                self.term[e] = Some(self.int_type);
                self.val[e] = Some(self.int_marker);
                self.ty[e] = Some(self.type_expr);
                self.int_type
            }
            ExprKind::Constant(value) => {
                // A kind marker as an expression: `[marker, Type]` — the
                // kind expression, first-class like any type.
                let marker = match value {
                    Constant::TypeFunction => self.function_type_marker,
                    Constant::TypeTuple => self.tuple_type_marker,
                    Constant::TypeArray => self.array_type_marker,
                    _ => unreachable!("USize, TypeType, and TypeInt are handled above"),
                };
                let pair = self.kind_expr(self.current_block, marker);
                self.term[e] = Some(pair);
                self.val[e] = Some(marker);
                self.ty[e] = Some(self.type_expr);
                pair
            }
            ExprKind::Parameter => {
                // A use of the parameter: the function's return expression
                // references the parameter's own `ExprId`.  The enclosing
                // function compiled the parameter pair (and entered the
                // scope) before compiling the return expression, so this
                // resolves to it.  The value slot is left alone — `value_of`
                // builds and memoizes the shared `Index(pair, 0)` lazily.
                let binding = self.lookup(e);
                self.term[e] = Some(binding.term);
                self.ty[e] = Some(binding.ty);
                binding.term
            }
            ExprKind::Function {
                parameter,
                r#return,
            } => self.check_lam(e, parameter, r#return),
            ExprKind::Apply { function, argument } => self.check_app(e, function, argument),
            ExprKind::BinOp {
                operator,
                left,
                right,
            } => self.check_binop(e, operator, left, right),
            ExprKind::Instantiate { type_expr, value } => {
                self.check_instantiate(e, type_expr, value)
            }
            ExprKind::Index { array, index } => self.check_index(e, array, index),
            ExprKind::Annotation {
                value,
                r#type: type_expr,
            } => self.check_ann(e, value, type_expr),
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => {
                self.check_expr(parameter, Role::Type);
                self.check_expr(r#return, Role::Type);
                let shape = self.array_node(
                    self.current_block,
                    &[self.term[parameter].unwrap(), self.term[r#return].unwrap()],
                );
                self.arrows.insert(shape);
                let kind = self.kind_expr(self.current_block, self.function_type_marker);
                let pair = self.array_node(self.current_block, &[shape, kind]);
                self.term[e] = Some(pair);
                self.val[e] = Some(shape);
                self.ty[e] = Some(kind);
                pair
            }
            ExprKind::Tuple(_) => self.check_tuple_term(e),
            ExprKind::TypeTuple(_) => self.check_tuple_type(e),
            ExprKind::TypeStruct(_) => self.check_type_struct(e),
            ExprKind::Array(_) => self.check_array_term(e),
            ExprKind::Placeholder => {
                // `_` — an inferrable type position: two fresh unbound
                // cells, one for the type's value slot and one for its
                // kind, so whatever the context unifies them with binds
                // them.  The kind slot must be a cell too, not the
                // universe: a compound type's kind slot holds a kind
                // expression (`[FunctionType, Type]`), which would clash
                // with `Type` itself.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                let pair = self.pair_of(val, ty_cell);
                self.term[e] = Some(pair);
                self.val[e] = Some(val);
                self.ty[e] = Some(ty_cell);
                pair
            }
            ExprKind::TypeArray {
                element_type,
                length,
            } => self.check_array_type(e, element_type, length),
        }
    }

    fn check_lam(&mut self, e: ExprId, parameter: ExprId, r#return: ExprId) -> NodeId {
        let return_block = self.module.add_block(None);
        self.body_blocks.push(vec![return_block]);
        let saved = self.current_block;
        self.current_block = return_block;
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        // The parameter *is* the pair `[value, type]`; the cells live in the
        // function's scope so the apply's clone yields fresh cells per call
        // (that is what makes a polymorphic value usable at several types).
        let param = self.array_node(return_block, &[value_cell, type_cell]);
        self.term[parameter] = Some(param);
        self.ty[parameter] = Some(type_cell);
        // A recursive binding (`rec fib = n => e`): the IR is a cycle — the
        // body references the function's own `ExprId`.  Register the
        // function's pair *before* the body compiles, so the reference
        // resolves to the pre-registered pair (whose value node is the
        // function's own) instead of re-entering this check.  The pair's
        // type slot is a cell, bound to the arrow below once the return
        // type is known.
        let recursive = self.ir.recursive.contains(&e);
        let self_ref = if recursive {
            let func_node = self.module.add_node(return_block, None, None);
            let ty_cell = self.fresh_cell();
            let pair = self.array_node(return_block, &[func_node, ty_cell]);
            self.term[e] = Some(pair);
            self.val[e] = Some(func_node);
            self.ty[e] = Some(ty_cell);
            Some((func_node, ty_cell))
        } else {
            None
        };
        self.scopes.push(HashMap::from([(
            parameter,
            Binding {
                term: param,
                ty: type_cell,
            },
        )]));
        let ret = self.check_expr(r#return, Role::Term);
        self.scopes.pop();
        self.current_block = saved;
        // The scope is the closure the body can reach: every block created
        // while compiling it, nested lambdas' blocks included, plus the
        // return even when it lives in a deeper block.  The apply's clone
        // then instantiates a nested function value as a fresh closure — its
        // references to this function's members are rewritten to the clones
        // the argument unify binds (a captured parameter), and its own cells
        // are fresh per call.
        let blocks = self.body_blocks.pop().expect("a function's body blocks");
        if let Some(outer) = self.body_blocks.last_mut() {
            outer.extend(blocks.iter().copied());
        }
        let mut nodes: HashSet<NodeId> = blocks
            .iter()
            .flat_map(|&block| self.module.blocks[block].nodes.iter().copied())
            .collect();
        nodes.insert(ret);
        // A recursive binding's value node pre-exists (the self-reference
        // applied it during the body); it fills in now with the function id.
        // `add_function` creates its own value node, so the manual insert
        // mirrors it for the pre-registered node.
        let func_node = if let Some((func_node, _)) = self_ref {
            let function = self.module.functions.insert(Function {
                nodes,
                r#return: ret,
                parameter: param,
                block: return_block,
            });
            self.module.blocks[return_block].functions.push(function);
            self.module.nodes[func_node].value = Some(Value::Function(function));
            self.recursive_func_nodes.push(func_node);
            func_node
        } else {
            self.module.add_function(return_block, ret, param, nodes)
        };
        // The function's own type: the arrow shape `[parameter type, return
        // type]` kinded as a function — `[[in, out], [FunctionType, Type]]`.
        let shape = self.array_node(return_block, &[type_cell, self.ty[r#return].unwrap()]);
        self.arrows.insert(shape);
        let kind = self.kind_expr(return_block, self.function_type_marker);
        let arrow = self.array_node(return_block, &[shape, kind]);
        if let Some((_, ty_cell)) = self_ref {
            // The self-reference's type cell now carries the arrow, so the
            // in-body applications see the function's real type.
            self.module.unify(ty_cell, arrow);
        }
        let pair = self.array_node(return_block, &[func_node, arrow]);
        self.term[e] = Some(pair);
        self.val[e] = Some(func_node);
        self.ty[e] = Some(arrow);
        pair
    }

    fn check_app(&mut self, e: ExprId, function: ExprId, argument: ExprId) -> NodeId {
        self.check_expr(function, Role::Term);
        self.check_expr(argument, Role::Term);
        // The function slot is the function's *value* (the runtime apply
        // needs a `Value::Function`, not the pair); the argument slot is the
        // full pair, so the apply's unify compares type cell to type cell.
        let function_value = self.value_of(function);
        let argument_pair = self.term[argument].unwrap();
        // Function-ness guard: catch *concretely* non-function types
        // statically (applying a literal is an error, not a runtime panic).
        // Concrete function types and unbound types (parameters, lambdas,
        // call results) are left to the runtime apply — unifying the shared
        // cell here would chain the type cells of every use of a polymorphic
        // value.  A failed unify never merges classes, so this cannot chain
        // either.
        let function_ty = self.ty[function].unwrap();
        let concrete = matches!(
            self.module.nodes[function_ty].value,
            Some(Value::Ext(_)) | Some(Value::USize(_)) | Some(Value::Array(_))
        );
        if concrete && !self.is_function_type(function_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            let shape = self.array_node(self.current_block, &[d, c]);
            let kind = self.kind_expr(self.current_block, self.function_type_marker);
            let fn_ty = self.array_node(self.current_block, &[shape, kind]);
            self.check_unify(function_ty, fn_ty, self.ir[e].span, DiagKind::Guard);
        }
        // The result's type cell: unbound unless the apply's evaluation
        // syncs it.  The cell rides in the apply's operand; the runtime
        // apply unifies the return pair with the apply node — the apply
        // node *is* the return pair — and binds the cell to the return
        // type: a concrete result syncs its type, a polymorphic template's
        // lazy result leaves it unbound.
        let c = self.fresh_cell();
        let operands = self.array_node(self.current_block, &[function_value, argument_pair, c]);
        let node = self.op_node(self.current_block, Operator::Apply, Some(operands));
        self.term[e] = Some(node);
        self.val[e] = None;
        self.ty[e] = Some(c);
        node
    }

    /// A binary integer operation `a op b`: both operands must be `Int`, and
    /// the result is `Int` (a comparison yields `0/1` to drive an `if`'s
    /// lazy `Index` branch).  Each operand's type is unified against the int
    /// type expression — a concretely non-`Int` operand is a check error,
    /// and an unbound operand (a parameter) is *pinned* to `Int`, so a
    /// later apply at a non-`Int` argument is a runtime failure in the
    /// argument unify, not a panic inside the operator.
    fn check_binop(&mut self, e: ExprId, operator: BinOp, left: ExprId, right: ExprId) -> NodeId {
        self.check_expr(left, Role::Term);
        self.check_expr(right, Role::Term);
        let span = self.ir[e].span;
        self.check_unify(self.ty[left].unwrap(), self.int_type, span, DiagKind::BinOp);
        self.check_unify(self.ty[right].unwrap(), self.int_type, span, DiagKind::BinOp);
        let operator = match operator {
            BinOp::Add => HighOperator::Add,
            BinOp::Sub => HighOperator::Sub,
            BinOp::Leq => HighOperator::Leq,
            BinOp::Eq => HighOperator::Eq,
        };
        let left = self.value_of(left);
        let right = self.value_of(right);
        let operands = self.array_node(self.current_block, &[left, right]);
        let value = self
            .op_node(self.current_block, Operator::Ext(operator), Some(operands));
        let pair = self.pair_of(value, self.int_type);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(self.int_type);
        pair
    }

    /// An indexing expression `a[i]`: the value is the structural `Index`
    /// over the array's value; the type is the element type.  For a
    /// statically-known tuple or struct the type is a structural `Index`
    /// over the element/field-type list — both sides then catch an
    /// out-of-bounds index.  For an array (or a type known only at runtime
    /// — a parameter, a call result) it is the custom
    /// [`HighOperator::IndexType`], which checks the index against the
    /// ArrayType's *length*: the array type's shape `[element_type, length]`
    /// holds the length as data, so no structural selection can check it.
    fn check_index(&mut self, e: ExprId, array: ExprId, index: ExprId) -> NodeId {
        self.check_expr(array, Role::Term);
        self.check_expr(index, Role::Term);
        let array_value = self.value_of(array);
        let index_value = self.value_of(index);
        let value_ops = self.array_node(self.current_block, &[array_value, index_value]);
        let value_node = self.op_node(self.current_block, Operator::Index, Some(value_ops));
        let array_ty = self.ty[array].unwrap();
        // Index-target guard: indexing a *concretely* non-indexable type —
        // a function, an atomic type — is an error, not a runtime panic
        // (mirroring the apply guard).  Tuple, array, and struct types pass
        // (a struct's shape is its field list, so instances are indexable);
        // an unbound type (a parameter, a call result) defers to the
        // runtime Index, which stays lazy until the type is known.
        let concrete = matches!(
            self.module.nodes[array_ty].value,
            Some(Value::Ext(_)) | Some(Value::USize(_)) | Some(Value::Array(_))
        );
        if concrete && !self.is_indexable_type(array_ty) {
            let error_index = self.module.unify_errors.len();
            self.module.unify_errors.push(UnifyError {
                a: array_ty,
                b: array_ty,
                value_a: self.module.nodes[array_ty].value,
                value_b: self.module.nodes[array_ty].value,
            });
            self.diary.push(DiaryEntry {
                error_index,
                a: array_ty,
                b: array_ty,
                span: self.ir[e].span,
                kind: DiagKind::IndexTarget,
            });
        }
        // The element-type list of a tuple type, or the field-type list of a
        // struct type — both positional lists, so a literal index into a
        // statically-known tuple or struct is bounds-checked at check time.
        // An array type's shape is `[element_type, length]` — not a
        // positional list — so it (and any type known only at runtime) goes
        // to the IndexType operator below.
        let field_shape = match self.module.array_ids(array_ty) {
            Some(ids) if ids.len() == 2 => {
                // Copy the ids out of the slice borrow: kind_marker_is
                // needs `&mut self` while `array_ids` borrows the module.
                let (shape, kind) = (ids[0], ids[1]);
                if self.kind_marker_is(kind, HighValue::TypeTuple)
                    || self.kind_marker_is_type_id(kind)
                {
                    Some(shape)
                } else {
                    None
                }
            }
            _ => None,
        };
        let ty_node = match field_shape {
            // The element-type list is structural, so an out-of-bounds
            // index is caught by the lowlevel `Index` bounds check.
            Some(tys) => {
                let ty_ops = self.array_node(self.current_block, &[tys, index_value]);
                self.op_node(self.current_block, Operator::Index, Some(ty_ops))
            }
            // The length lives inside the ArrayType as data: the check
            // runs in the custom operator, dispatched on the kind the
            // bound type carries at runtime.
            None => {
                let ty_ops = self.array_node(self.current_block, &[array_ty, index_value]);
                self.op_node(
                    self.current_block,
                    Operator::Ext(HighOperator::IndexType),
                    Some(ty_ops),
                )
            }
        };
        let pair = self.pair_of(value_node, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(ty_node);
        pair
    }

    fn check_ann(&mut self, e: ExprId, value: ExprId, type_expr: ExprId) -> NodeId {
        self.check_expr(type_expr, Role::Type);
        self.check_expr(value, Role::Term);
        // The annotation compares the full type expressions: the value
        // expression's type against the type expression itself — both are
        // pairs in the recursive encoding.  (Struct instantiation is not an
        // annotation — it is the dedicated [`ExprKind::Instantiate`].)
        let type_pair = self.term[type_expr].unwrap();
        self.check_unify(
            self.ty[value].unwrap(),
            type_pair,
            self.ir[e].span,
            DiagKind::Annotation,
        );
        let value_node = self.value_of(value);
        let pair = self.pair_of(value_node, type_pair);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(type_pair);
        pair
    }

    /// Struct instantiation: `s(1, 2)` — the positional tuple `value` is
    /// wrapped in the struct type `type_expr`.  The tuple's element-type
    /// list is checked against the struct's field list, and the
    /// expression's type is the struct type itself (the instance carries
    /// the nominal id; the tuple's own kind marker is discarded).  A
    /// non-tuple value fails the list check — a literal is not a struct
    /// value.
    fn check_instantiate(&mut self, e: ExprId, type_expr: ExprId, value: ExprId) -> NodeId {
        self.check_expr(type_expr, Role::Type);
        self.check_expr(value, Role::Term);
        let type_pair = self.term[type_expr].unwrap();
        let value_ty = self.ty[value].unwrap();
        // The value's shape: the element-type list of a tuple type, or the
        // type itself for anything else (which then fails the list check).
        let value_shape = match self.module.array_ids(value_ty) {
            Some(ids) if ids.len() == 2 => {
                // Copy the ids out of the slice borrow: kind_marker_is needs
                // `&mut self` while `array_ids` borrows the module.
                let (shape, kind) = (ids[0], ids[1]);
                if self.kind_marker_is(kind, HighValue::TypeTuple) {
                    shape
                } else {
                    value_ty
                }
            }
            _ => value_ty,
        };
        let field_list = self.module.array_ids(type_pair).unwrap()[0];
        self.check_unify(
            value_shape,
            field_list,
            self.ir[e].span,
            DiagKind::Annotation,
        );
        let value_node = self.value_of(value);
        let pair = self.pair_of(value_node, type_pair);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(type_pair);
        pair
    }

    fn check_tuple_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el, Role::Term);
            vals.push(self.value_of(el));
            tys.push(self.ty[el].unwrap());
        }
        // A tuple: `[values, [[element types], [TupleType, Type]]]`.
        let value = self.array_node(self.current_block, &vals);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A tuple type expression: `[[element types], [TupleType, Type]]`.
    fn check_tuple_type(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el, Role::Type);
            tys.push(self.term[el].unwrap());
        }
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.tuple_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }

    /// A struct type expression: `[[field types], [TypeId(n), Type]]` —
    /// like a tuple type, but the kind slot holds a *fresh nominal* id
    /// instead of the fixed `TupleType` marker.  The id comes from the
    /// [`HighOperator::Fresh`] call in the kind slot, so each occurrence
    /// allocates a new id and two occurrences never unify (nominal
    /// identity); a struct type is reused by binding it once through a
    /// parameter.  Fields are positional (no names in v1).
    fn check_type_struct(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el, Role::Type);
            tys.push(self.term[el].unwrap());
        }
        let shape = self.array_node(self.current_block, &tys);
        let id = self.op_node(self.current_block, Operator::Ext(HighOperator::Fresh), None);
        let kind = self.array_node(self.current_block, &[id, self.type_expr]);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }

    /// An array instance `[v1, ..., vn]` — every element shares one type:
    /// `[values, [[element type, length], [ArrayType, Type]]]`.  The element
    /// type is a fresh cell unified with each element's type (a
    /// heterogeneous literal is an error — the array's type would otherwise
    /// claim one type for elements that differ), and the length slot holds
    /// the element count.
    fn check_array_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let element_ty = self.fresh_cell();
        for &el in &elements {
            self.check_expr(el, Role::Term);
            vals.push(self.value_of(el));
            // Found = this element's type, expected = the shared cell: the
            // first element binds the cell, a later one that differs
            // conflicts against it.
            self.check_unify(
                self.ty[el].unwrap(),
                element_ty,
                self.ir[el].span,
                DiagKind::ArrayElement,
            );
        }
        let value = self.array_node(self.current_block, &vals);
        let length = self
            .module
            .add_node(self.current_block, None, Some(Value::USize(vals.len())));
        let shape = self.array_node(self.current_block, &[element_ty, length]);
        let kind = self.kind_expr(self.current_block, self.array_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// The real array type `{ element_type, length }` — the instance is the
    /// 2-element shape `[element_type, length]` (element 0: the type shared
    /// by all elements, element 1: the length), kinded as an array.  The
    /// element type is checked in type position (it must be a type), the
    /// length in term position (it is a value — e.g. `3` or, dependently,
    /// a parameter holding the length).  Both roles compile identically: a
    /// type expression is a first-class value, so the array type *is* its
    /// pair.
    fn check_array_type(&mut self, e: ExprId, element_type: ExprId, length: ExprId) -> NodeId {
        self.check_expr(element_type, Role::Type);
        self.check_expr(length, Role::Term);
        let length_value = self.value_of(length);
        let shape = self.array_node(
            self.current_block,
            &[self.term[element_type].unwrap(), length_value],
        );
        let kind = self.kind_expr(self.current_block, self.array_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }

    fn lookup(&self, target: ExprId) -> Binding {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&target).copied())
            .expect("unresolved parameter (frontend bug)")
    }

}

/// Whether the compiled subtree of `root` contains an [`Operator::Apply`]
/// operation — the criterion for the per-function definition pass (see
/// [`Checker::build`]).
fn self_subtree_contains_apply(module: &Module<HighProgram>, root: NodeId) -> bool {
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        let n = &module.nodes[node];
        if let Some(operation) = n.operation {
            if matches!(operation.operator, Operator::Apply) {
                return true;
            }
            if let Some(operand) = operation.operand {
                stack.push(operand);
            }
        }
        if let Some(Value::Array(ptr)) = n.value {
            stack.extend(unsafe { &*ptr }.iter().copied());
        }
    }
    false
}


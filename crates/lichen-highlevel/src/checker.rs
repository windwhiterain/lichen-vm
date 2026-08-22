//! The builder/checker: compiles an [`ExprTable`] into a lowlevel [`Module`]
//! where the runtime *is* the typechecker.
//!
//! Every expression compiles to a **recursive pair** `[value, type]` — a
//! 2-element array node whose elements are the value and its type, where the
//! type slot is itself such a pair.  Every type spine bottoms out at the
//! canonical universe `K = [Type, ↺]` — the self-referential `Type : Type`
//! node — so a literal is `[5, [int, K]]`, a function value
//! `[f, [[in, out], [FunctionType, K]]]`, and a tuple
//! `[[1, 2], [[int, int], [ArrayType, K]]]`.  A function parameter is such
//! a pair (`f(x: int)` maps to the parameter `[x, int-type]`), and a call
//! passes the argument's pair, so the apply-time unification
//! `unify(cloned_param, argument)` matches value-to-value and
//! **type-to-type** — that is the function parameter type check, executed
//! by the VM.
//!
//! The checker only *constructs* (pairs, type expressions, the universe)
//! and issues the unifies that have no apply to express them: annotations
//! and the function-ness guard (so applying a non-function is a reported
//! error, not a runtime panic); kinding (a type expression's own type must
//! be a kind) is structural and only fails for concrete non-kinds, like a
//! literal in type position.  It then runs the definition pass so the
//! apply-time checks fire; failures land in [`Module::unify_errors`], which
//! is the checker's error channel.

use std::collections::{HashMap, HashSet};

use lichen_vm::lowlevel::{BlockId, Module, NodeId, Operation, Operator, UnifyError, Value};

use crate::diag::{DiagKind, DiaryEntry};
use crate::expr::{ExprId, ExprKind, ExprTable, Span};
use crate::program::{HighProgram, HighValue};

/// Term position or type position.  The same expression can be a value in
/// one place and a type in another (first-class types); in type position its
/// value *is* the type and kinding (`ty : Type`) is enforced.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Term,
    Type,
}

/// A variable in scope: the parameter pair `[value, type]` plus its type
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
    ir: ExprTable,
    module: Module<HighProgram>,
    current_block: BlockId,
    scopes: Vec<HashMap<ExprId, Binding>>,
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
    int_node: NodeId,
    type_node: NodeId,
    function_type_node: NodeId,
    array_type_node: NodeId,
    /// The shared `[int, Type]` type expression every literal's pair carries.
    int_type: NodeId,
    /// The canonical universe `[Type, ↺]` — the self-referential `Type : Type`.
    type_expr: NodeId,
}

/// The result of building a program: the compiled Module plus the checker's
/// records.  `ok` is false when any unification failed (`unify_errors` is
/// non-empty); rendering diagnostics from those is future work.
pub struct Build {
    pub ir: ExprTable,
    pub module: Module<HighProgram>,
    pub term: Vec<Option<NodeId>>,
    pub val: Vec<Option<NodeId>>,
    pub ty: Vec<Option<NodeId>>,
    pub root_term: NodeId,
    pub root_val: NodeId,
    pub root_ty: NodeId,
    pub int_node: NodeId,
    pub type_node: NodeId,
    /// The shared `[int, Type]` type expression.
    pub int_type: NodeId,
    /// The canonical universe `[Type, ↺]`.
    pub type_expr: NodeId,
    /// The checker's attributed unification sequence (see [`DiaryEntry`]).
    pub diary: Vec<DiaryEntry>,
    /// Arrow nodes (see [`Checker::arrows`]); read by the diagnostics.
    pub arrows: HashSet<NodeId>,
    pub ok: bool,
}

impl Checker {
    pub fn build(ir: ExprTable) -> Build {
        let mut module = Module::new();
        let root_block = module.add_block(None);
        let n = ir.expr.len();
        let mut checker = Checker {
            ir,
            module,
            current_block: root_block,
            scopes: Vec::new(),
            term: vec![None; n],
            val: vec![None; n],
            ty: vec![None; n],
            diary: Vec::new(),
            arrows: HashSet::new(),
            int_node: NodeId::default(),
            type_node: NodeId::default(),
            function_type_node: NodeId::default(),
            array_type_node: NodeId::default(),
            int_type: NodeId::default(),
            type_expr: NodeId::default(),
        };
        checker.install_constants();
        let root = checker.ir.root;
        let root_term = checker.check_expr(root, Role::Term);
        let root_ty = checker.ty[root].expect("the root expression must have a type");
        // The definition pass: run the program so the apply-time type checks
        // fire.  Skipped when the checker-side unifies (annotations, kinding,
        // guards) already failed — the graph may then hit a non-function
        // apply, which the runtime panics on.
        if checker.module.unify_errors.is_empty() {
            checker.module.evaluate_node_deep(root_term, None);
        }
        let ok = checker.module.unify_errors.is_empty();
        let root_val = checker.value_of(root);
        Build {
            ir: checker.ir,
            module: checker.module,
            term: checker.term,
            val: checker.val,
            ty: checker.ty,
            root_term,
            root_val,
            root_ty,
            int_node: checker.int_node,
            type_node: checker.type_node,
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
        self.int_node = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::Int)));
        self.type_node = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::Type)));
        self.function_type_node = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::FunctionType)));
        self.array_type_node = self
            .module
            .add_node(root, None, Some(Value::Ext(HighValue::ArrayType)));
        // `K = [Type, K]`: allocate the node, then point its type slot at
        // itself.  The self-loop is cut by the lowlevel deep-evaluation
        // cycle guard whenever the definition pass reaches it.
        let universe = self.module.add_node(root, None, None);
        let slice = self.module.blocks[root]
            .arena
            .alloc_slice_copy(&[self.type_node, universe]);
        self.module.nodes[universe].value = Some(Value::Array(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )));
        self.type_expr = universe;
        self.int_type = self.array_node(root, &[self.int_node, self.type_expr]);
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
        let node = match self.ir[e].kind {
            ExprKind::Array(_) if role == Role::Type => self.check_array_type(e),
            _ => self.check_term(e),
        };
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
        if self.is_kind(ty) || unbound(self.module.nodes[ty].value) {
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
    /// expression `[FunctionType | ArrayType, K]`.
    fn is_kind(&mut self, node: NodeId) -> bool {
        if self.is_universe(node) {
            return true;
        }
        let Some(Value::Array(ptr)) = self.module.nodes[node].value else {
            return false;
        };
        let ids = unsafe { &*ptr };
        ids.len() == 2
            && self.is_universe(ids[1])
            && matches!(
                self.module.nodes[ids[0]].value,
                Some(Value::Ext(HighValue::FunctionType) | Value::Ext(HighValue::ArrayType))
            )
    }

    /// Whether the class of `node` is the canonical universe `K`.
    fn is_universe(&mut self, node: NodeId) -> bool {
        self.module.equality_representative(node)
            == self.module.equality_representative(self.type_expr)
    }

    /// Whether `kind` is the kind expression `[marker, K]`.
    fn kind_marker_is(&mut self, kind: NodeId, marker: HighValue) -> bool {
        let Some(Value::Array(ptr)) = self.module.nodes[kind].value else {
            return false;
        };
        let ids = unsafe { &*ptr };
        ids.len() == 2
            && self.module.nodes[ids[0]].value == Some(Value::Ext(marker))
            && self.is_universe(ids[1])
    }

    /// Whether `ty` is a concrete function type expression:
    /// `[shape, [FunctionType, K]]`.  The function-ness guard skips these —
    /// only concretely *non*-function types are caught statically.
    fn is_function_type(&mut self, ty: NodeId) -> bool {
        let Some(Value::Array(ptr)) = self.module.nodes[ty].value else {
            return false;
        };
        let ids = unsafe { &*ptr };
        ids.len() == 2 && self.kind_marker_is(ids[1], HighValue::FunctionType)
    }

    fn check_term(&mut self, e: ExprId) -> NodeId {
        match self.ir[e].kind {
            ExprKind::Int(n) => {
                let value = self
                    .module
                    .add_node(self.current_block, None, Some(Value::USize(n as usize)));
                let pair = self.pair_of(value, self.int_type);
                self.term[e] = Some(pair);
                self.val[e] = Some(value);
                self.ty[e] = Some(self.int_type);
                pair
            }
            ExprKind::Type | ExprKind::Const(HighValue::Type) => {
                // `Type` is the canonical universe node itself — `Type : Type`.
                self.term[e] = Some(self.type_expr);
                self.val[e] = Some(self.type_node);
                self.ty[e] = Some(self.type_expr);
                self.type_expr
            }
            ExprKind::Const(HighValue::Int) => {
                // The int type constant: `[int, Type]` — its value is the
                // marker and its type is the universe.
                self.term[e] = Some(self.int_type);
                self.val[e] = Some(self.int_node);
                self.ty[e] = Some(self.type_expr);
                self.int_type
            }
            ExprKind::Const(value) => {
                // A kind marker as an expression: `[marker, Type]` — the
                // kind expression, first-class like any type.
                let marker = match value {
                    HighValue::FunctionType => self.function_type_node,
                    HighValue::ArrayType => self.array_type_node,
                    _ => unreachable!("Int and Type are handled above"),
                };
                let pair = self.array_node(self.current_block, &[marker, self.type_expr]);
                self.term[e] = Some(pair);
                self.val[e] = Some(marker);
                self.ty[e] = Some(self.type_expr);
                pair
            }
            ExprKind::Binder => {
                debug_assert!(false, "a binder used as a value");
                let value = self.fresh_cell();
                let ty = self.fresh_cell();
                let pair = self.pair_of(value, ty);
                self.term[e] = Some(pair);
                self.val[e] = Some(value);
                self.ty[e] = Some(ty);
                pair
            }
            ExprKind::Var(target) => {
                let binding = self.lookup(target);
                self.term[e] = Some(binding.term);
                self.val[e] = None;
                self.ty[e] = Some(binding.ty);
                binding.term
            }
            ExprKind::Lam(binder, body) => self.check_lam(e, binder, body),
            ExprKind::Let(binder, value, body) => self.check_let(e, binder, value, body),
            ExprKind::App(f, x) => self.check_app(e, f, x),
            ExprKind::Ann(inner, t) => self.check_ann(e, inner, t),
            ExprKind::Arrow(d, c) => {
                self.check_expr(d, Role::Type);
                self.check_expr(c, Role::Type);
                let shape = self.array_node(
                    self.current_block,
                    &[self.term[d].unwrap(), self.term[c].unwrap()],
                );
                let kind = self.array_node(
                    self.current_block,
                    &[self.function_type_node, self.type_expr],
                );
                let pair = self.array_node(self.current_block, &[shape, kind]);
                self.term[e] = Some(pair);
                self.val[e] = Some(shape);
                self.ty[e] = Some(kind);
                pair
            }
            ExprKind::Array(_) => self.check_array_term(e),
        }
    }

    fn check_lam(&mut self, e: ExprId, binder: ExprId, body: ExprId) -> NodeId {
        let body_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = body_block;
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        // The parameter *is* the pair `[value, type]`; the cells live in the
        // function's scope so the apply's clone yields fresh cells per call
        // (that is what makes a polymorphic value usable at several types).
        let param = self.array_node(body_block, &[value_cell, type_cell]);
        self.term[binder] = Some(param);
        self.ty[binder] = Some(type_cell);
        self.scopes.push(HashMap::from([(
            binder,
            Binding {
                term: param,
                ty: type_cell,
            },
        )]));
        let ret = self.check_expr(body, Role::Term);
        self.scopes.pop();
        self.current_block = saved;
        // The return (the body's pair) must be in the function's scope, even
        // when the body is a nested structure whose pair lives in a deeper
        // block (e.g. a lambda inside a let body).
        let mut nodes = self.module.blocks[body_block].nodes.clone();
        if !nodes.contains(&ret) {
            nodes.push(ret);
        }
        let func_node = self.module.add_function(body_block, ret, param, &nodes);
        // The function's own type: the arrow shape `[param type, body type]`
        // kinded as a function — `[[in, out], [FunctionType, Type]]`.
        let shape = self.array_node(body_block, &[type_cell, self.ty[body].unwrap()]);
        self.arrows.insert(shape);
        let kind = self.array_node(body_block, &[self.function_type_node, self.type_expr]);
        let arrow = self.array_node(body_block, &[shape, kind]);
        let pair = self.array_node(body_block, &[func_node, arrow]);
        self.term[e] = Some(pair);
        self.val[e] = Some(func_node);
        self.ty[e] = Some(arrow);
        pair
    }

    fn check_let(&mut self, e: ExprId, binder: ExprId, value: ExprId, body: ExprId) -> NodeId {
        self.check_expr(value, Role::Term);
        let body_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = body_block;
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        let param = self.array_node(body_block, &[value_cell, type_cell]);
        self.term[binder] = Some(param);
        self.ty[binder] = Some(type_cell);
        self.scopes.push(HashMap::from([(
            binder,
            Binding {
                term: param,
                ty: type_cell,
            },
        )]));
        let ret = self.check_expr(body, Role::Term);
        self.scopes.pop();
        self.current_block = saved;
        let mut nodes = self.module.blocks[body_block].nodes.clone();
        if !nodes.contains(&ret) {
            nodes.push(ret);
        }
        let func = self.module.add_function(body_block, ret, param, &nodes);
        // `let x = e in b` runs as `(\x. b) e`: the argument is e's pair, and
        // the apply's unify pairs x's cells with e's value and type — the
        // runtime binding *is* the assignment.
        let operands = self.array_node(saved, &[func, self.term[value].unwrap()]);
        let node = self.op_node(saved, Operator::Apply, Some(operands));
        self.term[e] = Some(node);
        self.val[e] = None;
        self.ty[e] = Some(self.ty[body].unwrap());
        node
    }

    fn check_app(&mut self, e: ExprId, f: ExprId, x: ExprId) -> NodeId {
        self.check_expr(f, Role::Term);
        self.check_expr(x, Role::Term);
        // The function slot is the function's *value* (the runtime apply
        // needs a `Value::Function`, not the pair); the argument slot is the
        // full pair, so the apply's unify compares type cell to type cell.
        let ft = self.value_of(f);
        let xt = self.term[x].unwrap();
        // Function-ness guard: catch *concretely* non-function types
        // statically (applying a literal is an error, not a runtime panic).
        // Concrete function types and unbound types (parameters, lambdas,
        // call results) are left to the runtime apply — unifying the shared
        // cell here would chain the type cells of every use of a polymorphic
        // value.  A failed unify never merges classes, so this cannot chain
        // either.
        let f_ty = self.ty[f].unwrap();
        let concrete = matches!(
            self.module.nodes[f_ty].value,
            Some(Value::Ext(_)) | Some(Value::USize(_)) | Some(Value::Array(_))
        );
        if concrete && !self.is_function_type(f_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            let shape = self.array_node(self.current_block, &[d, c]);
            let kind = self.array_node(self.current_block, &[self.function_type_node, self.type_expr]);
            let fn_ty = self.array_node(self.current_block, &[shape, kind]);
            self.check_unify(f_ty, fn_ty, self.ir[e].span, DiagKind::Guard);
        }
        // The result's type cell: unbound unless anchored by an annotation.
        let c = self.fresh_cell();
        let operands = self.array_node(self.current_block, &[ft, xt]);
        let node = self.op_node(self.current_block, Operator::Apply, Some(operands));
        self.term[e] = Some(node);
        self.val[e] = None;
        self.ty[e] = Some(c);
        node
    }

    fn check_ann(&mut self, e: ExprId, inner: ExprId, t: ExprId) -> NodeId {
        self.check_expr(t, Role::Type);
        self.check_expr(inner, Role::Term);
        // The annotation compares the full type expressions: the inner
        // expression's type against the type expression `t` itself — both
        // are pairs in the recursive encoding.
        let t_ty = self.term[t].unwrap();
        self.check_unify(
            self.ty[inner].unwrap(),
            t_ty,
            self.ir[e].span,
            DiagKind::Annotation,
        );
        let inner_val = self.value_of(inner);
        let pair = self.pair_of(inner_val, t_ty);
        self.term[e] = Some(pair);
        self.val[e] = Some(inner_val);
        self.ty[e] = Some(t_ty);
        pair
    }

    fn check_array_term(&mut self, e: ExprId) -> NodeId {
        let range = match self.ir[e].kind {
            ExprKind::Array(range) => range,
            _ => unreachable!(),
        };
        let elements: Vec<ExprId> =
            self.ir.children[range.start as usize..range.end as usize].to_vec();
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el, Role::Term);
            vals.push(self.value_of(el));
            tys.push(self.ty[el].unwrap());
        }
        // A tuple: `[values, [[element types], [ArrayType, Type]]]`.
        let value = self.array_node(self.current_block, &vals);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.array_node(self.current_block, &[self.array_type_node, self.type_expr]);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// An array in type position: the tuple type `[[element types], [ArrayType, Type]]`.
    fn check_array_type(&mut self, e: ExprId) -> NodeId {
        let range = match self.ir[e].kind {
            ExprKind::Array(range) => range,
            _ => unreachable!(),
        };
        let elements: Vec<ExprId> =
            self.ir.children[range.start as usize..range.end as usize].to_vec();
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el, Role::Type);
            tys.push(self.term[el].unwrap());
        }
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.array_node(self.current_block, &[self.array_type_node, self.type_expr]);
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
            .expect("unresolved variable reference (frontend bug)")
    }
}

/// A class is unbound while it carries no value or only the lazy marker.
fn unbound(value: Option<Value<HighProgram>>) -> bool {
    matches!(value, None | Some(Value::Parameterized))
}

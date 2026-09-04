//! The builder/checker: compiles an [`IR`] into a lowlevel [`Module`]
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
//! (so applying a non-function is a reported error, not a runtime panic).
//! It then runs the definition pass so the
//! apply-time checks fire; failures land in [`Module::unify_errors`], which
//! is the checker's error channel.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, ArrayItem, BlockId, Function, FunctionId, LowOperator, LowValue,
    Module, NodeId, Operation, Registry, UnifyError,
};

use lichen_utils::extend::AsEnum;

use crate::attr::AttrExt;
use crate::diagnostic::{DiagKind, DiaryEntry};
use crate::ir::{BinOp, ChildRange, ExprId, ExprKind, IR, Loc, LocStep};
use crate::native::{no_native_ops, NativeArg, NativeOps};
use crate::program::{Ctx, HighProgram, LiteralExt, TypeOperator, ValueType};

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

pub struct Checker<P: HighProgram + 'static>
where
    P::Value: ValueType,
{
    ir: IR<P::Attr, P::Literal>,
    module: Module<P>,
    pub current_block: BlockId,
    /// The attribute extension registry: maps an attribute marker
    /// (`P::Attr`) to its lowering behaviour.  The checker never names a
    /// concrete attribute — it asks this registry for the `AttrExt` and
    /// calls through it.
    attr_ext: Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>,
    /// The native-operator registry: a private, name→operator mapping for the
    /// compiling module's plugin (see [`crate::native`]).  The empty default
    /// `no_native_ops` rejects every `$name` call (the frontend reports it as
    /// unresolved); a plugin whose source is being compiled (e.g.
    /// `lichen-compute`'s `jit`/`launch`) supplies its own slice.
    native_ops: NativeOps<P>,
    scopes: Vec<HashMap<ExprId, Binding>>,
    /// The lexical function stack: one `(id, depth)` entry per enclosing
    /// lambda whose body is being compiled, innermost last — `depth` is the
    /// frontend's lexical depth ([`ExprKind::Function::depth`], the value
    /// at the lambda's definition point).  The innermost entry is the
    /// current function ([`Checker::current_function`]): every node the
    /// checker allocates is tagged with it (its template membership
    /// back-pointer) and registered in its scope ([`Function::nodes`]), and
    /// asserts registered in it join [`Function::asserts`].  [`Checker::check_lam`]
    /// pushes its shell while the body compiles and pops on exit — the old
    /// per-frame block bookkeeping is gone, the lexical nesting lives in
    /// [`Function::parent`], and the depth keeps *sibling* bindings
    /// disjoint: a lambda compiled while another lambda's body is being
    /// checked may be a same-depth sibling (mutual recursion), which hangs
    /// under nothing, never under the lambda being checked.
    function_stack: Vec<(FunctionId, u32)>,
    /// The compiled pair node `[value, type]` per expression.
    term: Vec<Option<NodeId>>,
    /// Element 0 of the pair (the value).  `None` for call results, whose
    /// value is only known at runtime — built lazily as
    /// `Index(pair, 0)` via [`Checker::value_of`] when the value is needed.
    val: Vec<Option<NodeId>>,
    /// Element 1 of the pair (the type).
    ty: Vec<Option<NodeId>>,
    /// Element 2+ of the pair — the static-schema attributes' slots.  Indexed
    /// like [`Checker::ty`]: `None` for an expression whose schema is the
    /// ordinary `[value, type]` pair; the attribute slot for one carrying
    /// the `Perspective` tail (a `# p` annotation or a `x # n` parameter).
    attr: Vec<Option<NodeId>>,
    /// The parameter-attribute slot of each function whose parameter is
    /// annotated `x # n` — keyed by the `Function` expression id, carrying the
    /// attribute marker (the parameter's schema tail) and the fresh attribute
    /// cell.  The apply uses it (or `missing` for an unannotated parameter) to
    /// run the attribute equality check.
    function_param_attr: HashMap<ExprId, (P::Attr, NodeId)>,
    /// The checker's own unification sequence, attributed for diagnostics:
    /// each entry records which `unify_errors` entry (if any) the unify
    /// produced, plus its span and check kind.
    diary: Vec<DiaryEntry>,
    /// The arrow nodes built by [`Checker::check_lam`] — the type printer
    /// renders these `[param, body]` shapes as `param → body`.
    arrows: HashSet<NodeId>,
    /// The apply edges, keyed by apply op node — the argument structure the
    /// diagnostics use to attribute a runtime parameter-check failure to the
    /// argument's source span (see [`ApplyEdge`]).
    apply_edges: HashMap<NodeId, ApplyEdge>,
    /// The runtime-attribution edges: a node a runtime failure will
    /// reference (an `Index`/`TableGet` operand, an assert condition) mapped
    /// to its source-blind location.  Recorded by the checker as it builds the
    /// operation, so the diagnostic layer attributes a runtime error to the
    /// *expression* that built it without storing any span *on* the node —
    /// which is what lets the lowlevel graph be freely shared.
    node_edges: HashMap<NodeId, Loc>,
    /// The assert conditions that are *user-facing* — the explicit `assert`
    /// expressions (not the generated array-bounds guard `check_index`
    /// registers, which duplicates the index eval error).  The diagnostics
    /// layer renders only these as `DiagKind::Assert`; a bounds guard fires
    /// a separate `EvalError::Index`, so rendering both would double report.
    user_asserts: HashSet<NodeId>,
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
    type_struct_marker: NodeId,
    table_type_marker: NodeId,
    /// The shared `USize(0)` constant — the attribute attribute's
    /// `missing()` value, read for any expression whose schema has no
    /// attribute slot.  Reused so `attr_or_missing` adds no node per use.
    zero_marker: NodeId,
    /// The shared `[int, Type]` type expression every literal's pair carries.
    int_type: NodeId,
    /// The canonical universe `[Type, ↺]` — the self-referential `Type : Type`.
    type_expr: NodeId,
}

/// The highlevel structure of one application's argument edge, recorded by
/// [`Checker::check_app`] when it wires the apply.  A runtime parameter-check
/// failure is attributed to the *edge* between the function's parameter and
/// the argument — keyed by the apply op node in [`Build::apply_edges`] — not
/// to a shared node, so the argument's own source span stays reachable even
/// when the argument node is reused (`Int`'s term is the shared int type).
#[derive(Clone, Copy, Debug)]
pub struct ApplyEdge {
    /// The argument's IR expression — the caret target.
    pub argument_expr: ExprId,
    /// The whole apply's IR expression (the call site) — context.
    pub apply_expr: ExprId,
}

/// The result of building a program: the compiled Module plus the checker's
/// records.  `ok` is false when any unification failed (`unify_errors` is
/// non-empty), any runtime evaluation failed (`eval_errors`), or any assert
/// failed (`assert_errors`); rendering diagnostics from those is future
/// work.
pub struct Build<P: HighProgram>
where
    P::Value: ValueType,
{
    pub ir: IR<P::Attr, P::Literal>,
    pub module: Module<P>,
    pub term: Vec<Option<NodeId>>,
    pub val: Vec<Option<NodeId>>,
    pub ty: Vec<Option<NodeId>>,
    pub attr: Vec<Option<NodeId>>,
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
    /// The apply edges keyed by apply op node (see [`ApplyEdge`]) — the
    /// argument structure for attributing a runtime parameter-check failure.
    pub apply_edges: HashMap<NodeId, ApplyEdge>,
    /// The runtime-attribution edges (see [`Checker::node_edges`]) — a runtime
    /// failure's node mapped to the source-blind location that built it.  No
    /// span is stored on a node, so the graph is freely shareable.
    pub node_edges: HashMap<NodeId, Loc>,
    /// The user-facing assert condition nodes (see [`Checker::user_asserts`]).
    pub user_asserts: HashSet<NodeId>,
    pub ok: bool,
}

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// Compile an IR with a fresh private registry and no attribute
    /// extension (a program whose schemas carry no attribute is unaffected).
    pub fn build(ir: IR<P::Attr, P::Literal>) -> Build<P> {
        Self::build_with(ir, Module::new(), Self::no_attr_ext(), no_native_ops())
    }

    /// Compile an IR whose module is bound to a caller-provided shared
    /// registry — the entry point for importers that resolve [`ExprKind::Static`]
    /// leaves through a `PackageStore`.
    pub fn build_in(ir: IR<P::Attr, P::Literal>, registry: Arc<RwLock<Registry<P>>>) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(ir, module, Self::no_attr_ext(), no_native_ops())
    }

    /// Compile an IR with a caller-supplied attribute extension registry — the
    /// entry point for a language that plugs in a concrete attribute (e.g.
    /// `Perspective`).  `attr_ext` maps an attribute marker to its lowering
    /// behaviour; the checker never names a concrete attribute.
    pub fn build_in_attr(
        ir: IR<P::Attr, P::Literal>,
        registry: Arc<RwLock<Registry<P>>>,
        attr_ext: Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>,
    ) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(ir, module, attr_ext, no_native_ops())
    }

    /// [`Self::build_in_attr`] with a native-operator registry — the entry
    /// point for a plugin whose embedded source calls `$name(args…)`.  The
    /// `native_ops` slice is that plugin's *private* registry: it is what a
    /// `$name` in its source resolves against, and it is empty for every
    /// ordinary file.
    pub fn build_in_attr_native(
        ir: IR<P::Attr, P::Literal>,
        registry: Arc<RwLock<Registry<P>>>,
        attr_ext: Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>,
        native_ops: NativeOps<P>,
    ) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(ir, module, attr_ext, native_ops)
    }

    /// The no-op registry of a program with no attribute extension: no schema
    /// carries an attribute, so it is never consulted.
    fn no_attr_ext() -> Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>> {
        Box::new(|_attr: &P::Attr| -> &'static dyn AttrExt<P> {
            unreachable!("this program has no attribute extension")
        })
    }

    fn build_with(
        ir: IR<P::Attr, P::Literal>,
        mut module: Module<P>,
        attr_ext: Box<dyn Fn(&P::Attr) -> &'static dyn AttrExt<P>>,
        native_ops: NativeOps<P>,
    ) -> Build<P> {
        // The lowlevel's default application guard (10k nested calls) sits
        // below what a thread stack survives once the checker's per-call
        // machinery (clone, unify, deep pass) is on the stack — a
        // non-terminating recursion would overflow the stack before the
        // guard fires.  Lower it so the guard panics cleanly; legitimate
        // recursion (fib, countdown) nests far below this.
        module.apply_depth_limit = 500;
        // The lazy graph flattens most recursion (an apply returns its
        // result pair; the deep pass descends into it), so nested depth
        // alone does not bound a run — an infinite loop behind a lazy
        // branch stays at depth 1, and a wide recursion (fib) is never deep.
        // The total-application budget is the work bound that catches those;
        // the definition pass runs the whole program, so it must be set
        // here, before it starts.  Each application also grows the module's
        // classes (every recursion level's parameter unifies into one
        // shared class), so a tight budget stops a runaway recursion in
        // seconds; legitimate programs (the examples, fib up to ~15,
        // countdown) apply far fewer times than this.
        module.apply_total_limit = 2_000;
        let root_block = module.add_block(None);
        let n = ir.expr.len();
        let mut checker = Checker {
            ir,
            module,
            current_block: root_block,
            attr_ext,
            native_ops,
            scopes: Vec::new(),
            function_stack: Vec::new(),
            term: vec![None; n],
            val: vec![None; n],
            ty: vec![None; n],
            attr: vec![None; n],
            function_param_attr: HashMap::new(),
            diary: Vec::new(),
            arrows: HashSet::new(),
            apply_edges: HashMap::new(),
            node_edges: HashMap::new(),
            user_asserts: HashSet::new(),
            recursive_func_nodes: Vec::new(),
            int_marker: NodeId::default(),
            type_marker: NodeId::default(),
            function_type_marker: NodeId::default(),
            tuple_type_marker: NodeId::default(),
            array_type_marker: NodeId::default(),
            type_struct_marker: NodeId::default(),
            table_type_marker: NodeId::default(),
            zero_marker: NodeId::default(),
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
        let root_term = checker.check_expr(root);
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
        // Skipped when the checker-side unifies (annotations,
        // guards) already failed — the graph may then hit a non-function
        // apply, which the runtime panics on.
        if checker.module.unify_errors.is_empty() {
            let functions: Vec<lichen_lowlevel::FunctionId> =
                checker.module.functions.keys().collect();
            for function in functions {
                let (ret, asserts) = {
                    let function = &checker.module.functions[function];
                    (function.r#return, function.asserts.clone())
                };
                // Every body runs once: its apply-time checks fire even when
                // the function is never applied, and the deep pass decides
                // what the apply clone may reference in place — a body the
                // pass never computed keeps every operation node unproven
                // and clones them all, silently re-running per-application
                // computations like a body-local struct's nominal-id
                // `Fresh`.
                checker.module.evaluate_node_deep(ret, None);
                // The body's asserts are reachability entry points like the
                // return: each condition gets its own concreteness proof, so
                // an apply references a per-call-invariant condition in
                // place instead of cloning and re-registering it, while a
                // condition reading the parameter stays unproven and clones.
                for &condition in &asserts {
                    checker.module.evaluate_node_deep(condition, None);
                }
            }
        }
        if checker.module.unify_errors.is_empty() {
            checker.module.evaluate_node_deep(root_term, None);
        }
        // The assert pass: drain the module's constraint worklist — the
        // originals and the clones the definition pass's applies produced —
        // force-evaluating each condition (ignoring laziness) and requiring
        // `USize(1)`.  Decided points are consumed; an assert whose condition
        // stays lazy is *not triggered* and stays pending on the worklist:
        // an in-body assert whose parameter was never bound (the function was
        // never applied) is deferred instead of failing, and the clone
        // re-checks it per call.  Skipped when the definition pass was (the
        // graph may be broken enough to panic on the forced evaluation).
        if checker.module.unify_errors.is_empty() {
            checker.module.check_asserts();
        }
        let ok = checker.module.unify_errors.is_empty()
            && checker.module.eval_errors.is_empty()
            && checker.module.assert_errors.is_empty();
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
            attr: checker.attr,
            root_term,
            root_val,
            root_ty,
            int_marker: checker.int_marker,
            type_marker: checker.type_marker,
            int_type: checker.int_type,
            type_expr: checker.type_expr,
            diary: checker.diary,
            arrows: checker.arrows,
            apply_edges: checker.apply_edges,
            node_edges: checker.node_edges,
            user_asserts: checker.user_asserts,
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
        self.int_marker = self.alloc_node(root, None, Some(P::Value::int_marker()));
        self.type_marker = self.alloc_node(root, None, Some(P::Value::type_marker()));
        self.function_type_marker = self.alloc_node(root, None, Some(P::Value::function_type_marker()));
        self.tuple_type_marker = self.alloc_node(root, None, Some(P::Value::tuple_type_marker()));
        self.array_type_marker = self.alloc_node(root, None, Some(P::Value::array_type_marker()));
        self.type_struct_marker = self.alloc_node(root, None, Some(P::Value::type_struct_marker()));
        self.table_type_marker = self.alloc_node(root, None, Some(P::Value::table_type_marker()));
        self.zero_marker = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(0))));
        // `K = [Type, K]`: allocate the node, then point its type slot at
        // itself.  The self-loop is cut by the lowlevel deep-evaluation
        // cycle guard whenever the definition pass reaches it.
        let universe = self.alloc_node(root, None, None);
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(self.type_marker)),
            ArrayItem::new(AnyNodeId::Dynamic(universe)),
        ];
        self.module.write_node_value(
            universe,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, root),
            ))),
        );
        self.type_expr = universe;
        self.int_type = self.array_node(root, &[self.int_marker, self.type_expr]);
    }

    // --- allocation ------------------------------------------------------

    /// The innermost function whose body is being compiled — [`None`] at
    /// top level.
    fn current_function(&self) -> Option<FunctionId> {
        self.function_stack.last().map(|&(function, _)| function)
    }

    /// A node owned by the current function — the checker's only node
    /// creation point.  The node is tagged with [`Checker::current_function`]
    /// and registered in its scope ([`Function::nodes`]), so the apply clone
    /// walk's chain membership test recognizes it as part of the template.
    /// Top-level nodes (no current function) are untagged and belong to no
    /// template, exactly like the lowlevel's raw [`Module::add_node`].
    fn alloc_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.module.add_node(block, operation, value);
        if let Some(function) = self.current_function() {
            self.module.nodes[node].function = Some(function);
            self.module.functions[function].nodes.push(node);
        }
        node
    }

    /// A fresh, unbound type cell (a parameterized node — evaluating it
    /// yields the lazy marker, never a panic).
    pub fn fresh_cell(&mut self) -> NodeId {
        self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Parameterized)),
        )
    }

    /// A plain value node in the current block — the way a native operator
    /// extension builds a custom value (e.g. `Kernel`/`LaunchTarget`) without
    /// an operation.
    pub fn value_node(&mut self, value: P::Value) -> NodeId {
        self.alloc_node(self.current_block, None, Some(value))
    }

    pub fn array_node(&mut self, block: BlockId, ids: &[NodeId]) -> NodeId {
        let items: Vec<ArrayItem> = ids
            .iter()
            .map(|&node| ArrayItem::new(AnyNodeId::Dynamic(node)))
            .collect();
        self.alloc_node(
            block,
            None,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, block),
            ))),
        )
    }

    /// [`Self::array_node`] with per-position shallow flags — the `~`
    /// markers of a shallow array.  An all-`false` flag set is the plain
    /// unmarked form.
    fn array_node_masked(&mut self, block: BlockId, ids: &[NodeId], mask: &[bool]) -> NodeId {
        let items: Vec<ArrayItem> = ids
            .iter()
            .zip(mask.iter())
            .map(|(&node, &shallow)| ArrayItem {
                node: AnyNodeId::Dynamic(node),
                shallow,
            })
            .collect();
        self.alloc_node(
            block,
            None,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, block),
            ))),
        )
    }

    pub fn op_node(
        &mut self,
        block: BlockId,
        operator: P::Operator,
        operand: Option<NodeId>,
    ) -> NodeId {
        self.alloc_node(block, Some(Operation { operator, operand }), None)
    }

    /// A pair node `[value, type]` built around two already-compiled nodes.
    fn pair_of(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        self.array_node(self.current_block, &[value, ty])
    }

    /// The kind expression of a compound type: `[marker, Type]`, where
    /// `marker` is one of the kind markers (`FunctionType`, `TupleType`,
    /// `ArrayType`, `TypeStruct`).
    fn kind_expr(&mut self, block: BlockId, marker: NodeId) -> NodeId {
        self.array_node(block, &[marker, self.type_expr])
    }

    /// The children of a variadic expression (`Tuple`, `TypeTuple`,
    /// `Array`, `TypeStruct`, `ShallowArray`).
    fn range_children(&self, e: ExprId) -> Vec<ExprId> {
        let range = match self.ir[e].kind {
            ExprKind::Tuple(range)
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::TypeStruct(range)
            | ExprKind::Table(range)
            | ExprKind::ShallowArray { range, .. } => range,
            ExprKind::NativeCall { args, .. } => args,
            _ => unreachable!("expected a variadic expression kind"),
        };
        self.ir.children[range.start as usize..range.end as usize].to_vec()
    }

    /// The per-element `~` depths of a shallow array, one per child of
    /// [`Self::range_children`].
    fn range_depths(&self, e: ExprId) -> Vec<usize> {
        let range = match self.ir[e].kind {
            ExprKind::ShallowArray { depths, .. } => depths,
            _ => unreachable!("expected a shallow array expression kind"),
        };
        self.ir.depths[range.start as usize..range.end as usize].to_vec()
    }

    /// The direct sub-expressions of a compound whose perspectives participate
    /// in a `# p` annotation's meet (gcd) — the reference.  A leaf has none.
    /// Per the plan's combine table: the named sub-expressions of a `BinOp`/
    /// `Apply`/`Instantiate`/`Index`/`Field`/`Find`/`TypeArray`/`TypeFunction`
    /// and a `TypeFunction`, the `children` range of a variadic, transparent
    /// through an `Annotation`, and empty for a leaf.
    fn persp_combine_children(&self, e: ExprId) -> Vec<ExprId> {
        match self.ir[e].kind {
            ExprKind::BinOp { left, right, .. } => vec![left, right],
            ExprKind::Apply { function, argument } => vec![function, argument],
            ExprKind::Instantiate { type_expr, value } => vec![type_expr, value],
            ExprKind::Index { array, index } => vec![array, index],
            ExprKind::Field { container, key } => vec![container, key],
            ExprKind::Find { container, key } => vec![container, key],
            ExprKind::TypeArray {
                element_type,
                length,
            } => vec![element_type, length],
            ExprKind::TypeFunction { parameter, r#return } => vec![parameter, r#return],
            ExprKind::Tuple(_)
            | ExprKind::TypeTuple(_)
            | ExprKind::Array(_)
            | ExprKind::TypeStruct(_)
            | ExprKind::Table(_)
            | ExprKind::ShallowArray { .. } => self.range_children(e),
            // Transparent: a `# p` on an annotated value is `value`'s own
            // attribute.
            ExprKind::Annotation { value, .. } => vec![value],
            // Leaf kinds — the annotation binds the slot directly.  An
            // `ErrorBlock` is a leaf too: a masked region carries no children
            // to combine.
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => Vec::new(),
            // A lambda is a leaf for stage 1 (its own `# p` binds a slot).
            ExprKind::Function { .. } => Vec::new(),
            ExprKind::Assert { .. } => Vec::new(),
            ExprKind::NativeCall { .. } => self.range_children(e),
        }
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
        let zero =
            self.module
                .add_node(self.current_block, None, Some(P::Value::from(LowValue::USize(0))));
        let operands = self.array_node(self.current_block, &[pair, zero]);
        let index = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        self.val[e] = Some(index);
        index
    }

    /// The attribute slot of an expression, reading the attribute's
    /// `missing()` value (`USize(0)` — neutral in `gcd`) for an expression
    /// whose schema has no attribute slot.  Only meaningful after the
    /// expression has been compiled.
    fn attr_or_missing(&self, e: ExprId) -> NodeId {
        self.attr[e].unwrap_or(self.zero_marker)
    }

    /// The value currently held by `node`'s equality class — the
    /// representative's value — or `None` when the class is unbound.
    /// Read-only (a parent-pointer walk, no path compression).  An attribute's
    /// [`AttrExt::is_subtype`] uses this to compare two slot values after a
    /// failed equality unify.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        self.module.class_value(node)
    }

    /// The canonical universe node `[Type, ↺]` (`Type : Type`) — the kind slip a
    /// type expression's kind slot closes on (`[marker, Type]`).  Native
    /// operator extensions build type expressions with it.
    pub fn type_expr_node(&self) -> NodeId {
        self.type_expr
    }

    /// The canonical, shared `[int, Type]` type expression — the type of every
    /// int value.  A native operator extension (e.g. `jit`) compares a
    /// function's signature against it.
    pub fn int_type_node(&self) -> NodeId {
        self.int_type
    }

    // --- the check -------------------------------------------------------

    /// A checker-issued unification, diary-attributed: records which error
    /// (if any) it produced, with the source-blind [`Loc`] and check kind that
    /// drive the diagnostic's expected/found direction.  The `loc`'s `slot`
    /// names which element of the expression's `[value, type, attrs…]` pair
    /// the check is about; its `path` is filled from the unify's own descent
    /// (`steps`).
    pub fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) {
        let before = self.module.unify_errors.len();
        self.module.unify(a, b);
        if self.module.unify_errors.len() > before {
            let err = &self.module.unify_errors[before];
            let path = tag_descent(&self.module, loc.path.clone(), b, &err.steps);
            self.diary.push(DiaryEntry {
                error_index: before,
                a,
                b,
                loc: Loc {
                    expr: loc.expr,
                    path,
                },
                kind,
            });
        }
    }

    /// A checker-issued unification that may be relaxed by an attribute's
    /// optional subtype relation.  Attempts the ordinary unify; if it fails
    /// **and** `is_subtype` holds for the two operands, the error(s) this
    /// unify produced are discarded and the check counts as passed.
    ///
    /// `is_subtype` receives the unify's two operands `(a, b)` — the
    /// *found/value* side first and the *expected/declared* side second, the
    /// same convention as [`AttrExt::unify_slots`] — as a `&dyn Ctx<P>` (the
    /// curated context), and returns whether the relation is satisfied.  The
    /// attribute decides which operand is the subtype and which the supertype.
    ///
    /// Suppression is a safe truncate: an attribute unify's operands are
    /// scalar (a perspective is a `USize` or an unbound cell, never a
    /// compound array), so a failed unify merges nothing and records exactly
    /// the errors the truncate removes.  The checker's attribute check is a
    /// *validation gate* — the value itself flows in through the lowlevel
    /// apply's separate clone-unify — so suppressing leaves the graph correct.
    pub fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    ) {
        let before = self.module.unify_errors.len();
        self.module.unify(a, b);
        if self.module.unify_errors.len() > before {
            if is_subtype(self, a, b) {
                self.module.unify_errors.truncate(before);
            } else {
                let err = &self.module.unify_errors[before];
                let path = tag_descent(&self.module, loc.path.clone(), b, &err.steps);
                self.diary.push(DiaryEntry {
                    error_index: before,
                    a,
                    b,
                    loc: Loc {
                        expr: loc.expr,
                        path,
                    },
                    kind,
                });
            }
        }
    }

    fn check_expr(&mut self, e: ExprId) -> NodeId {
        // The variant decides the compilation (a `Tuple` is a value, a
        // `TypeTuple` a type expression — the frontend picks per syntactic
        // role).  There is no term/type distinction in the checker: every
        // expression compiles to its pair, and correctness is decided by the
        // unifications the surrounding construct issues.
        self.check_term(e)
    }

    /// The array items behind either a dynamic node or a static ref.
    fn any_items(&self, id: AnyNodeId) -> Option<&'static [ArrayItem]> {
        let value = self.module.node_value(id)?;
        let LowValue::Array(array) = value.as_enum()? else {
            return None;
        };
        Some(array.items())
    }

    /// Whether a static ref names the canonical universe.  A frozen universe
    /// is a 2-item self-referential array (`[Type, itself]`), so it is
    /// recognized by content instead of equality classes.
    fn is_static_universe(&self, sref: lichen_lowlevel::StaticNodeId) -> bool {
        let Some(items) = self.any_items(AnyNodeId::Static(sref)) else {
            return false;
        };
        items.len() == 2
            && self.module.node_value(items[0].node) == Some(P::Value::type_marker())
            && matches!(items[1].node, AnyNodeId::Static(tail) if tail.module == sref.module && tail.index == sref.index)
    }

    fn is_universe_any(&mut self, id: AnyNodeId) -> bool {
        match id {
            AnyNodeId::Dynamic(node) => {
                self.module.equality_representative(node)
                    == self.module.equality_representative(self.type_expr)
            }
            AnyNodeId::Static(sref) => self.is_static_universe(sref),
        }
    }

    /// Whether `kind` is the kind expression `[marker, K]`.
    fn kind_marker_is_any(&mut self, kind: AnyNodeId, marker: P::Value) -> bool {
        let Some(items) = self.any_items(kind) else {
            return false;
        };
        items.len() == 2
            && self.module.node_value(items[0].node) == Some(marker)
            && self.is_universe_any(items[1].node)
    }

    fn kind_marker_is(&mut self, kind: NodeId, marker: P::Value) -> bool {
        self.kind_marker_is_any(AnyNodeId::Dynamic(kind), marker)
    }

    /// Whether `ty` is a concrete function type expression:
    /// `[shape, [FunctionType, K]]`.  The function-ness guard skips these —
    /// only concretely *non*-function types are caught statically.
    fn is_function_type_any(&mut self, ty: AnyNodeId) -> bool {
        let Some(items) = self.any_items(ty) else {
            return false;
        };
        items.len() == 2 && self.kind_marker_is_any(items[1].node, P::Value::function_type_marker())
    }

    fn is_function_type(&mut self, ty: NodeId) -> bool {
        self.is_function_type_any(AnyNodeId::Dynamic(ty))
    }

    /// Whether `ty` is a struct type: `[shape, [id, [TypeStruct, K]]]`.  A
    /// struct's nominal id lives in the kind slot's head; the `TypeStruct`
    /// tag is the inner layer `[TypeStruct, K]` at `kind[1]`.
    fn is_struct_type_any(&mut self, ty: AnyNodeId) -> bool {
        let Some(items) = self.any_items(ty) else {
            return false;
        };
        if items.len() != 2 {
            return false;
        }
        let Some(kind_items) = self.any_items(items[1].node) else {
            return false;
        };
        kind_items.len() == 2
            && self.kind_marker_is_any(kind_items[1].node, P::Value::type_struct_marker())
    }

    /// Whether `ty` is a concrete positional type expression — a tuple type
    /// (`[shape, [TypeTuple, K]]`) or a struct type (`[shape, [id,
    /// [TypeStruct, K]]]`, whose shape is the positional field-type list).
    /// The field-read guard (`a(k)`) skips these; an array reads with
    /// `a[i]` (its type is pinned, so misuse fails the pin unify), a table
    /// with `t{k}`, and only concretely *non*-positional types are caught
    /// statically here.
    fn is_positional_type_any(&mut self, ty: AnyNodeId) -> bool {
        let Some(items) = self.any_items(ty) else {
            return false;
        };
        if items.len() != 2 {
            return false;
        }
        self.kind_marker_is_any(items[1].node, P::Value::tuple_type_marker()) || self.is_struct_type_any(ty)
    }

    fn is_positional_type(&mut self, ty: NodeId) -> bool {
        self.is_positional_type_any(AnyNodeId::Dynamic(ty))
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
        // A cycle can only form through a block-wide binding placeholder —
        // an inline compound term's subtree can never reference its own root,
        // so pre-registering a skeleton for one would only add spurious cells
        // that poison the apply-time unify (a placeholder reached through an
        // index-typed apply would stay an unbound `?a` instead of binding to
        // the actual type).  Gate the skeleton on `block_roots`.
        let skeleton = if self.ir.block_roots.contains(&e)
            && matches!(
                self.ir[e].kind,
                ExprKind::Apply { .. }
                    | ExprKind::BinOp { .. }
                    | ExprKind::Instantiate { .. }
                | ExprKind::Assert { .. }
                | ExprKind::Index { .. }
                | ExprKind::Find { .. }
                | ExprKind::Annotation { .. }
                    | ExprKind::TypeFunction { .. }
                    | ExprKind::Tuple(_)
                    | ExprKind::TypeTuple(_)
                    | ExprKind::TypeStruct(_)
                    | ExprKind::Array(_)
                    | ExprKind::Table(_)
                    | ExprKind::ShallowArray { .. }
                    | ExprKind::TypeArray { .. }
                    | ExprKind::NativeCall { .. }
            ) {
            let vc = self.fresh_cell();
            let tc = self.fresh_cell();
            let skel = self.array_node(self.current_block, &[vc, tc]);
            self.term[e] = Some(skel);
            self.val[e] = Some(vc);
            self.ty[e] = Some(tc);
            Some((vc, tc))
        } else {
            None
        };
        let pair = match self.ir[e].kind {
            ExprKind::Literal(lit) => {
                // A literal builds its `[value, type]` pair itself through
                // its `LiteralExt::build` — the built-in int literal and
                // type-constant literal each build their value and type
                // nodes (referencing the prebuilt singleton exprs the
                // context exposes); a custom literal builds any value and
                // type pair, potentially referencing other exprs.  The
                // checker records the pair and its two halves — `Type : Type`
                // is built as the self-referential universe node, so it is
                // not `[value, type]`.
                let built = lit.build(self);
                self.term[e] = Some(built.pair);
                self.val[e] = Some(built.value);
                self.ty[e] = Some(built.ty);
                built.pair
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
                parameter_type,
                parameter_attribute,
                r#return,
                depth,
            } => self.check_lam(
                e,
                depth,
                parameter_type,
                parameter_attribute,
                parameter,
                r#return,
            ),
            ExprKind::Apply { function, argument } => self.check_app(e, function, argument),
            ExprKind::BinOp {
                operator,
                left,
                right,
            } => self.check_binop(e, operator, left, right),
            ExprKind::Instantiate { type_expr, value } => {
                self.check_instantiate(e, type_expr, value)
            }
            ExprKind::Assert { condition } => self.check_assert(e, condition),
            ExprKind::Index { array, index } => self.check_index(e, array, index),
            ExprKind::Field { container, key } => self.check_field(e, container, key),
            ExprKind::Find { container, key } => self.check_table_find(e, container, key),
            ExprKind::Annotation {
                value,
                r#type,
                attribute,
            } => self.check_ann(e, value, r#type, attribute),
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => {
                let parameter_ty = self.check_type_element(parameter);
                let return_ty = self.check_type_element(r#return);
                let shape = self.array_node(self.current_block, &[parameter_ty, return_ty]);
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
            ExprKind::Table(_) => self.check_table_term(e),
            ExprKind::ShallowArray { .. } => self.check_shallow_array_term(e),
            ExprKind::ErrorBlock => {
                // A recovered-error region, masked at the frontend: an
                // opaque leaf.  Compile it to a pair of fresh, *never*
                // unified cells — nothing inside the region is checked, so
                // it cannot introduce a spurious *type*-level "expected X,
                // found Y" from inside itself (the parser's own syntactic
                // diagnostic still fires at the parse layer), and the region
                // is distinct from a real `_` (`Placeholder`) so the frontend
                // can mask it for a diff.  The fresh cells stay unbound, so
                // they never cause a cascade.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                let pair = self.pair_of(val, ty_cell);
                self.term[e] = Some(pair);
                self.val[e] = Some(val);
                self.ty[e] = Some(ty_cell);
                pair
            }
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
            ExprKind::Static { export } => {
                // Imported package export: the static ref names the package's
                // final `[value, type]` pair.  Materialize that pair leaf,
                // then extract dynamic value/type leaves from its static
                // items; the payloads stay in the package's static arena.
                let pair = self.module.materialize_leaf(export, self.current_block);
                let items = self
                    .module
                    .array_items(pair)
                    .expect("a package export must be the final [value, type] pair");
                let value_node = self.module.as_dynamic(items[0].node, self.current_block);
                let ty_node = self.module.as_dynamic(items[1].node, self.current_block);
                self.term[e] = Some(pair);
                self.val[e] = Some(value_node);
                self.ty[e] = Some(ty_node);
                pair
            }
            ExprKind::NativeCall { op, args } => self.check_native_call(e, op, args),
        };
        // Bind the skeleton's cells to the real value and type, so every
        // reference that resolved to the skeleton during the descent now
        // equals the finished expression.
        if let Some((vc, tc)) = skeleton {
            let value = self.value_of(e);
            let ty = self.ty[e].expect("a compound kind sets a type");
            self.module.unify(vc, value);
            self.module.unify(tc, ty);
        }
        pair
    }

    /// A `$name(args…)` call: compile each argument, look `name` up in this
    /// module's private [`NativeOps`] registry, and adopt the `[value, type]`
    /// pair the plugin's [`NativeOp`] builder returns.  The checker has no
    /// knowledge of what the operator does — the plugin's registration owns the
    /// lowering and the type construction (the private contract with its own
    /// source).
    fn check_native_call(&mut self, e: ExprId, op: &str, args: ChildRange) -> NodeId {
        let arg_ids: Vec<ExprId> =
            self.ir.children[args.start as usize..args.end as usize].to_vec();
        for &arg in &arg_ids {
            self.check_expr(arg);
        }
        let native_args: Vec<NativeArg> = arg_ids
            .iter()
            .map(|&arg| NativeArg {
                expr: arg,
                value: self.value_of(arg),
                ty: self.ty[arg].expect("a compiled argument has a type"),
            })
            .collect();
        let loc = self.loc(e, 0);
        let ops = self.native_ops;
        let built = ops
            .iter()
            .copied()
            .find(|(name, _)| *name == op)
            .map(|(_, op)| op.build(self, e, &native_args, loc))
            .expect(
                "a native op name must be validated by the frontend against the module's registry",
            );
        self.term[e] = Some(built.node);
        self.val[e] = built.val;
        self.ty[e] = Some(built.ty);
        built.node
    }

    fn check_lam(
        &mut self,
        e: ExprId,
        depth: u32,
        parameter_type: Option<ExprId>,
        parameter_attribute: Option<ExprId>,
        parameter: ExprId,
        r#return: ExprId,
    ) -> NodeId {
        let return_block = self.module.add_block(None);
        let saved = self.current_block;
        self.current_block = return_block;
        let value_cell = self.fresh_cell();
        let type_cell = self.fresh_cell();
        // The parameter *is* the pair `[value, type]`; the cells live in the
        // function's scope so the apply's clone yields fresh cells per call
        // (that is what makes a polymorphic value usable at several types).
        // A `x # n` parameter carries a third attribute slot (a fresh cell
        // bound to the attribute value in body scope), so the pair becomes
        // the schema-shaped `[value, type, attribute]`.
        let attr_cell = parameter_attribute.is_some().then(|| self.fresh_cell());
        let param = match attr_cell {
            Some(attr) => {
                self.attr[parameter] = Some(attr);
                self.array_node(return_block, &[value_cell, type_cell, attr])
            }
            None => self.array_node(return_block, &[value_cell, type_cell]),
        };
        // The function shell: its id exists from the start of the body, so
        // every node compiled below is tagged with it and registered in its
        // scope — the template the apply clone walk recognizes by chain
        // membership.  `parent` is the enclosing function: a nested
        // closure's nodes then read as members of the enclosing template
        // too, while a sibling's do not (the mutual-recursion invariant).
        // The return and parameter slots are placeholders, filled once the
        // body is checked.
        let function = self.module.functions.insert(Function {
            nodes: Vec::new(),
            r#return: param,
            parameter: param,
            asserts: Vec::new(),
            // The lexical parent is the innermost enclosing function whose
            // definition depth is one less: a genuinely-nested closure joins
            // the enclosing template, while a same-depth sibling binding
            // (mutual recursion — compiled here because the body references
            // it) hangs under nothing.  Its closure then stays outside this
            // template, referenced in place, so the recursion re-applies the
            // sibling's never-bound template instead of a bound instance.
            parent: self
                .function_stack
                .iter()
                .rposition(|&(_, d)| d + 1 == depth)
                .map(|i| self.function_stack[i].0),
            block: return_block,
        });
        self.module.blocks[return_block].functions.push(function);
        // The nodes above predate the shell: the allocation helper
        // tagged them with (and registered them in) the *enclosing*
        // function's scope.  They are this function's own — move them into
        // its scope and retag them, exactly where the helper would have put
        // them had the shell existed first.
        let mut param_parts = vec![value_cell, type_cell];
        if let Some(attr) = attr_cell {
            param_parts.push(attr);
        }
        // The parameter pair itself (`param`) belongs to this function's
        // template scope as well — the lowlevel [`Function`] contract and the
        // apply clone walk both require `parameter` to be registered in
        // `nodes`, so the debug gate in `function_apply` holds.
        param_parts.push(param);
        if let Some(enclosing) = self.current_function() {
            self.module.functions[enclosing]
                .nodes
                .retain(|&n| !param_parts.contains(&n));
        }
        for &node in &param_parts {
            self.module.nodes[node].function = Some(function);
        }
        self.module.functions[function].nodes = param_parts;
        self.function_stack.push((function, depth));
        self.term[parameter] = Some(param);
        self.ty[parameter] = Some(type_cell);
        // A self- or mutually-recursive binding (`fib = n => e`): the IR is a
        // cycle — the body references the function's own `ExprId`.  Register
        // the function's pair *before* the body compiles, so the reference
        // resolves to the pre-registered pair (whose value node is the
        // function's own) instead of re-entering this check.  Every lambda
        // takes this path (a non-referencing lambda's pre-registration is
        // overwritten identically below); the pair's type slot is a cell,
        // bound to the arrow below once the return type is known.
        let func_node = self.alloc_node(return_block, None, None);
        let ty_cell = self.fresh_cell();
        let pair = self.array_node(return_block, &[func_node, ty_cell]);
        self.term[e] = Some(pair);
        self.val[e] = Some(func_node);
        self.ty[e] = Some(ty_cell);
        self.scopes.push(HashMap::from([(
            parameter,
            Binding {
                term: param,
                ty: type_cell,
            },
        )]));
        // The annotated parameter's type is compiled in scope — it may
        // reference the parameter itself (`x : x -> Int`) — and unified
        // against the parameter's type slot *before* the body compiles, so
        // in-body readers see the annotated kind statically (an array
        // annotation's length, a function annotation's arrow) and the
        // generated constraints fire at normalize.  At each apply the
        // argument still checks against the very same slot — the unify
        // differs from the outer `(x => e) : (T -> _)` annotation only in
        // when it happens, not in what it binds.
        if let Some(parameter_type) = parameter_type {
            self.check_expr(parameter_type);
            let type_pair =
                self.term[parameter_type].expect("the type expression must compile to a pair");
            self.check_unify(
                type_cell,
                type_pair,
                self.loc(parameter_type, 1),
                DiagKind::Annotation,
            );
        }
        // The annotated parameter's attribute `x # n`, compiled in body scope
        // like the type so `n` may reference the parameter itself.  The
        // declared perspective is a *template* constraint: the apply's check
        // compares each argument's attribute against this declared value node
        // (via [`Checker::function_param_attr`], then the attribute's
        // [`AttrExt::unify_slots`]).  The live attribute cell in the parameter
        // pair is deliberately left **unbound** — binding it to the declared
        // value here would let the deep pass *bake* it (it is a concrete
        // value), so the per-apply clone would reference the template's cell
        // instead of resetting it, and the lowlevel apply's positional unify
        // would then enforce the declared perspective (equality) against the
        // argument, defeating the attribute's subtype relaxation.  Kept
        // unbound, it is a fresh per-apply clone that binds the argument's
        // actual perspective, exactly like the value/type cells — so the
        // body's return reads the caller's perspective and `f (5 # 4)` yields
        // `5 # 4`.  The declared value itself stays only in
        // [`Checker::function_param_attr`].
        if let Some(parameter_attribute) = parameter_attribute {
            self.check_expr(parameter_attribute);
            let declared = self.value_of(parameter_attribute);
            // The parameter's schema tail[0] names the attribute; the apply's
            // check resolves its `AttrExt` from this marker.
            let marker = self.ir.schema(parameter).tail[0];
            self.function_param_attr.insert(e, (marker, declared));
        }
        let ret = self.check_expr(r#return);
        self.scopes.pop();
        self.current_block = saved;
        // The shell fills in: the return and parameter entry points.  The
        // body's asserts were registered into [`Function::asserts`] as they
        // were compiled, and the scope grew node by node.  The value node
        // pre-exists (the self-reference applied it during the body); it
        // fills in now with the function id.
        self.module.functions[function].r#return = ret;
        self.module.functions[function].parameter = param;
        // The return may live in another function's scope — a body ending
        // in a variable reference to a nested closure's pair (owned by that
        // closure, its chain reaching here through the parent link).  The
        // entry point still belongs to this function's scope: the apply
        // walk starts from it.
        if !self.module.functions[function].nodes.contains(&ret) {
            self.module.functions[function].nodes.push(ret);
        }
        self.module.write_node_value(
            func_node,
            Some(P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(
                function,
            )))),
        );
        self.recursive_func_nodes.push(func_node);
        // The function's own type: the arrow shape `[parameter type, return
        // type]` kinded as a function — `[[in, out], [FunctionType, Type]]`.
        // Built while the current function is still the shell, so these
        // nodes join its scope like the rest of the body.
        let shape = self.array_node(return_block, &[type_cell, self.ty[r#return].unwrap()]);
        self.arrows.insert(shape);
        let kind = self.kind_expr(return_block, self.function_type_marker);
        let arrow = self.array_node(return_block, &[shape, kind]);
        // The self-reference's type cell now carries the arrow, so the
        // in-body applications see the function's real type.
        self.module.unify(ty_cell, arrow);
        let pair = self.array_node(return_block, &[func_node, arrow]);
        self.function_stack.pop();
        self.term[e] = Some(pair);
        self.val[e] = Some(func_node);
        self.ty[e] = Some(arrow);
        pair
    }

    fn check_app(&mut self, e: ExprId, function: ExprId, argument: ExprId) -> NodeId {
        self.check_expr(function);
        self.check_expr(argument);
        // The function slot is the function's *value* (the runtime apply
        // needs a `HighProgramValue::Function`, not the pair); the argument slot is the
        // full pair, so the apply's unify compares type cell to type cell.
        let function_value = self.value_of(function);
        // The apply's argument operand is normalized to the function's
        // declared *parameter* arity, slot-aligned: value@0, type@1, and — for
        // a parameter carrying the attribute attribute — the argument's
        // attribute@2 (read `missing()` = `0` when absent).  The lowlevel
        // apply's positional unify then compares value-to-value and
        // type-to-type, matching the parameter pair's shape.  The attribute
        // equality is checked separately below (the per-apply parameter clone
        // resets its own attribute cell, so it cannot enforce the template's
        // declared value).
        let param_persp = self.function_param_attr.get(&function).copied();
        let argument_value = self.value_of(argument);
        let argument_type = self.ty[argument].unwrap();
        let argument_persp = self.attr_or_missing(argument);
        let argument_pair = match param_persp {
            Some(_) => self.array_node(
                self.current_block,
                &[argument_value, argument_type, argument_persp],
            ),
            None => self.array_node(self.current_block, &[argument_value, argument_type]),
        };
        // Function-ness guard: catch *concretely* non-function types
        // statically (applying a literal is an error, not a runtime panic).
        // Concrete function types and unbound types (parameters, lambdas,
        // call results) are left to the runtime apply — unifying the shared
        // cell here would chain the type cells of every use of a polymorphic
        // value.  A failed unify never merges classes, so this cannot chain
        // either.
        let function_ty = self.ty[function].unwrap();
        let concrete = self
            .module
            .node_value(AnyNodeId::Dynamic(function_ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            });
        if concrete && !self.is_function_type(function_ty) {
            let d = self.fresh_cell();
            let c = self.fresh_cell();
            let shape = self.array_node(self.current_block, &[d, c]);
            let kind = self.kind_expr(self.current_block, self.function_type_marker);
            let fn_ty = self.array_node(self.current_block, &[shape, kind]);
            self.check_unify(function_ty, fn_ty, self.loc(e, 1), DiagKind::Guard);
        }
        // The apply's attribute equality check: the function's declared
        // parameter attribute (or its `missing` for an unannotated parameter)
        // against the argument's attribute (or `missing`).  This is what
        // rejects `id (5 # 4)` (declared missing vs `4`) and `f 5` for
        // `f = x # 4 => x` (declared `4` vs missing).  Routed through the
        // attribute's `AttrExt::unify_slots`; a program with no attribute
        // extension reaches neither branch (no schema carries an attribute).
        if let Some((param_marker, param_slot)) = param_persp {
            let ext = (self.attr_ext)(&param_marker);
            let arg_missing = self.attr_or_missing(argument);
            let loc2 = self.loc(e, 2);
            ext.unify_slots(self, arg_missing, param_slot, loc2);
        } else if self.attr[argument].is_some() {
            let marker = &self.ir.schema(argument).tail[0];
            let ext = (self.attr_ext)(marker);
            let found_attr = self.attr[argument].unwrap();
            let zero_marker = self.zero_marker;
            let loc2 = self.loc(e, 2);
            ext.unify_slots(self, found_attr, zero_marker, loc2);
        }
        // The result's type cell: unbound unless the apply's evaluation
        // syncs it.  The cell rides in the apply's operand; the runtime
        // apply unifies the return pair with the apply node — the apply
        // node *is* the return pair — and binds the cell to the return
        // type: a concrete result syncs its type, a polymorphic template's
        // lazy result leaves it unbound.
        let c = self.fresh_cell();
        let operands = self.array_node(self.current_block, &[function_value, argument_pair, c]);
        let node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Apply),
            Some(operands),
        );
        // Record the argument edge: the checker is the only place that knows
        // this application's argument structure (its expression's source
        // span), and the *edge* (this apply op node -> the argument) is unique
        // per application even when the argument node itself is shared — so a
        // runtime parameter-check failure can be attributed to the argument's
        // span regardless of node sharing.  The lowlevel records only the
        // apply node on failure; the diagnostics read this edge.
        self.apply_edges.insert(
            node,
            ApplyEdge {
                argument_expr: argument,
                apply_expr: e,
            },
        );
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
        self.check_expr(left);
        self.check_expr(right);
        self.check_unify(
            self.ty[left].unwrap(),
            self.int_type,
            self.loc(left, 1),
            DiagKind::BinOp,
        );
        self.check_unify(
            self.ty[right].unwrap(),
            self.int_type,
            self.loc(right, 1),
            DiagKind::BinOp,
        );
        let operator = match operator {
            BinOp::Add => P::Operator::from(TypeOperator::Add),
            BinOp::Sub => P::Operator::from(TypeOperator::Sub),
            BinOp::Leq => P::Operator::from(TypeOperator::Leq),
            BinOp::Eq => P::Operator::from(TypeOperator::Eq),
        };
        let left = self.value_of(left);
        let right = self.value_of(right);
        let operands = self.array_node(self.current_block, &[left, right]);
        let value = self.op_node(self.current_block, operator, Some(operands));
        let pair = self.pair_of(value, self.int_type);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(self.int_type);
        pair
    }

    /// An array-element read `a[i]`.  The container's type is *pinned* to a
    /// fresh array type — the same pin [`Self::check_binop`] applies to its
    /// operands and [`Self::check_table_find`] to its container — so a
    /// concretely non-array container (a tuple, a struct, a function, a
    /// table) fails here with a diagnostic, and an unbound container (a
    /// parameter, a call result) resolves at the call site's argument
    /// unify: only an array can flow in.  Tuple and struct slots are read
    /// with the dedicated positional form `a(k)` ([`Self::check_field`]),
    /// table entries with `t{k}` — the operator and the type extraction
    /// are chosen by syntax, never by a runtime kind dispatch.
    ///
    /// The value is the structural `Index` over the array's value; the type
    /// is the pinned shape's element cell, which lands in the concrete
    /// element type's class when the container type binds.  The read also
    /// registers a *bounds constraint* on the module's assert worklist:
    /// `i < length` as `length <= i == 0`, the comparison ops the value
    /// language already has.  A concrete out-of-range index fails the
    /// assert pass; a lazy length (an annotated parameter) keeps the assert
    /// pending, and the apply clone re-checks it against each argument's
    /// actual array.
    fn check_index(&mut self, e: ExprId, array: ExprId, index: ExprId) -> NodeId {
        self.check_expr(array);
        self.check_expr(index);
        let elem_cell = self.fresh_cell();
        let len_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[elem_cell, len_cell]);
        let kind = self.kind_expr(self.current_block, self.array_type_marker);
        let array_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.ty[array].unwrap(),
            array_ty,
            self.loc(array, 1),
            DiagKind::Guard,
        );
        let array_value = self.value_of(array);
        let index_value = self.value_of(index);
        self.node_edges.insert(index_value, self.loc(index, 0));
        let value_ops = self.array_node(self.current_block, &[array_value, index_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let zero =
            self.alloc_node(self.current_block, None, Some(P::Value::from(LowValue::USize(0))));
        let beyond_ops = self.array_node(self.current_block, &[len_cell, index_value]);
        let beyond = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Leq),
            Some(beyond_ops),
        );
        let in_range_ops = self.array_node(self.current_block, &[beyond, zero]);
        let in_range = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Eq),
            Some(in_range_ops),
        );
        self.register_assert(in_range, self.loc(e, 0), false);
        let pair = self.pair_of(value_node, elem_cell);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(elem_cell);
        pair
    }

    /// A positional slot read `a(k)` — a tuple element or a struct field
    /// (both type shapes are positional lists; the nominal struct id lives
    /// in the kind slot, so the extraction is the same for both).  The
    /// frontend emits this form for the adjacent single-expression paren —
    /// `a(1)` — the syntactic distinction from struct instantiation
    /// (`a(1,)`, `a(1,1)`, and the two zero-field spellings `a()` / `a(,)`,
    /// mirroring the tuple grammar's `()` unit vs `(,)` empty tuple) and
    /// from function application (a spaced paren), so no runtime kind
    /// dispatch decides the read.
    ///
    /// The value is the structural `Index` over the container's value; the
    /// type is `Index(shape, k)` over the container type's shape — read
    /// structurally from the type pair itself, so a concrete tuple/struct
    /// resolves at check time and an unbound container (a parameter, a call
    /// result) resolves when the call binds it.  A *concretely*
    /// non-positional container — an array (`a[i]` is its read), a table
    /// (`t{k}`), a function, an atomic type — is the guard's error below,
    /// not a runtime panic (mirroring the apply guard).
    fn check_field(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let container_ty = self.ty[container].unwrap();
        let concrete = self
            .module
            .node_value(AnyNodeId::Dynamic(container_ty))
            .is_some_and(|value| {
                matches!(
                    value.as_enum(),
                    None | Some(LowValue::USize(_)) | Some(LowValue::Array(_))
                )
            });
        if concrete && !self.is_positional_type(container_ty) {
            let error_index = self.module.unify_errors.len();
            self.module.unify_errors.push(UnifyError {
                root_a: container_ty,
                root_b: container_ty,
                steps: Vec::new(),
                a: container_ty,
                b: container_ty,
                value_a: self.module.node_value(AnyNodeId::Dynamic(container_ty)),
                value_b: self.module.node_value(AnyNodeId::Dynamic(container_ty)),
            });
            self.diary.push(DiaryEntry {
                error_index,
                a: container_ty,
                b: container_ty,
                loc: self.loc(container, 1),
                kind: DiagKind::IndexTarget,
            });
        }
        let zero =
            self.alloc_node(self.current_block, None, Some(P::Value::from(LowValue::USize(0))));
        let container_value = self.value_of(container);
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        let value_ops = self.array_node(self.current_block, &[container_value, key_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(value_ops),
        );
        let shape_ops = self.array_node(self.current_block, &[container_ty, zero]);
        let shape = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(shape_ops),
        );
        let ty_ops = self.array_node(self.current_block, &[shape, key_value]);
        let ty_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_ops),
        );
        let pair = self.pair_of(value_node, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A table lookup `t{k}`: the lowlevel `TableGet` reads the entry whose
    /// stored key is deep-content-equal to `k`.  The frontend emits this
    /// form for the *adjacent* brace — the syntactic distinction from
    /// positional [`Self::check_index`] — so the operator is chosen by
    /// syntax, never by a runtime kind dispatch.
    ///
    /// The container's type is *pinned* to a fresh table type — the same
    /// pin [`Self::check_binop`] applies to its operands — so a concretely
    /// non-table container fails here with a diagnostic, and an unbound
    /// container (a parameter, a call result) resolves at the call site's
    /// argument unify: only a table can flow in.  The value is the
    /// `TableGet` op node itself; the type is the pinned shape's value-type
    /// cell, which lands in the concrete value type's class when the
    /// container type binds.
    fn check_table_find(&mut self, e: ExprId, container: ExprId, key: ExprId) -> NodeId {
        self.check_expr(container);
        self.check_expr(key);
        let key_cell = self.fresh_cell();
        let value_cell = self.fresh_cell();
        let shape = self.array_node(self.current_block, &[key_cell, value_cell]);
        let kind = self.kind_expr(self.current_block, self.table_type_marker);
        let table_ty = self.array_node(self.current_block, &[shape, kind]);
        self.check_unify(
            self.ty[container].unwrap(),
            table_ty,
            self.loc(container, 1),
            DiagKind::Guard,
        );
        let container_value = self.value_of(container);
        let key_value = self.value_of(key);
        self.node_edges.insert(key_value, self.loc(key, 0));
        let ops = self.array_node(self.current_block, &[container_value, key_value]);
        let value_node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::TableGet),
            Some(ops),
        );
        let pair = self.pair_of(value_node, value_cell);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(value_cell);
        pair
    }

    fn check_ann(
        &mut self,
        e: ExprId,
        value: ExprId,
        r#type: Option<ExprId>,
        attribute: Option<ExprId>,
    ) -> NodeId {
        self.check_expr(value);
        // `: T` — the value expression's type must unify with the type
        // expression itself; both sides are pairs in the recursive encoding.
        // The type slot is the annotation's own type expression (shared), or
        // the value's own type when only `# p` is present.  (Struct
        // instantiation is not an annotation — it is the dedicated
        // [`ExprKind::Instantiate`].)
        let type_pair = match r#type {
            Some(type_expr) => {
                self.check_expr(type_expr);
                let type_pair = self.term[type_expr].unwrap();
                self.check_unify(
                    self.ty[value].unwrap(),
                    type_pair,
                    self.loc(value, 1),
                    DiagKind::Annotation,
                );
                type_pair
            }
            None => self.ty[value].unwrap(),
        };
        let value_node = self.value_of(value);
        // `# p` — the attribute slot.  A leaf's slot is `p` itself; a
        // compound's is the attribute's meet over its direct sub-expressions'
        // attribute slots (absent → the attribute's `missing_value`), then
        // `# p` unifies that slot with `p`.  `# p` also stamps this node's
        // schema with the attribute tail, so the pair is built one slot
        // wider.  The checker asks the registry for the attribute's
        // `AttrExt` — it never names a concrete attribute.
        let pair = match attribute {
            Some(p) => {
                self.check_expr(p);
                let attr_val = self.value_of(p);
                let marker = &self.ir.schema(e).tail[0];
                let ext = (self.attr_ext)(marker);
                let children = self.persp_combine_children(value);
                let slot = if children.is_empty() {
                    attr_val
                } else {
                    let child_attrs: Vec<NodeId> =
                        children.iter().map(|&c| self.attr_or_missing(c)).collect();
                    let combined = ext.combine(self, &child_attrs);
                    let loc2 = self.loc(e, 2);
                    ext.unify_slots(self, combined, attr_val, loc2);
                    combined
                };
                self.attr[e] = Some(slot);
                self.array_node(self.current_block, &[value_node, type_pair, slot])
            }
            None => {
                self.attr[e] = None;
                self.pair_of(value_node, type_pair)
            }
        };
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
        self.check_expr(type_expr);
        self.check_expr(value);
        let type_pair = self.term[type_expr].unwrap();
        let value_ty = self.ty[value].unwrap();
        // The value's shape: the element-type list of a tuple type, or the
        // type itself for anything else (which then fails the list check).
        let value_shape = match self.module.array_items(value_ty) {
            Some(items) if items.len() == 2 => {
                // Materialize static refs so the shape can participate in
                // dynamic array construction below.
                let shape = self.module.as_dynamic(items[0].node, self.current_block);
                let kind = self.module.as_dynamic(items[1].node, self.current_block);
                if self.kind_marker_is(kind, P::Value::tuple_type_marker()) {
                    shape
                } else {
                    value_ty
                }
            }
            _ => value_ty,
        };
        // The struct pair's shape *is* the positional field-type list (the
        // nominal id lives in the kind slot).  During a recursive struct's
        // own descent the shape is still an unbound cell (the bindings are
        // mutually recursive), so defer the field-list check through a probe
        // cell.
        let shape = self.module.as_dynamic(
            self.module.array_items(type_pair).unwrap()[0].node,
            self.current_block,
        );
        let field_list = match self.module.array_items(shape) {
            Some(_) => shape,
            // The struct's shape cell is not resolved yet (mid-recursion):
            // bind it to a [field-list] probe and check the value against the
            // probe slot.  When the descent completes the cell unifies with
            // the real shape, closing the deferred check.
            _ => {
                let fields_cell = self.fresh_cell();
                self.module.unify(shape, fields_cell);
                fields_cell
            }
        };
        self.check_unify(
            value_shape,
            field_list,
            self.loc(e, 1),
            DiagKind::Annotation,
        );
        let value_node = self.value_of(value);
        let pair = self.pair_of(value_node, type_pair);
        self.term[e] = Some(pair);
        self.val[e] = Some(value_node);
        self.ty[e] = Some(type_pair);
        pair
    }

    /// `assert(condition)` — an explicit constraint, not a unify: the
    /// condition's *value* node is registered as an assert.  The
    /// lowlevel's [`Module::check_asserts`] then force-evaluates every
    /// assert (ignoring laziness) after the definition pass and requires
    /// `USize(1)` — an unbound condition is not bound to `1`, it stays
    /// untriggered, and the apply clone re-checks the instantiated
    /// condition per call.  The expression compiles to the condition
    /// itself: an assert checks its subject, it does not replace it.
    fn check_assert(&mut self, e: ExprId, condition: ExprId) -> NodeId {
        self.check_expr(condition);
        // The checked thing is a `USize`, so the assert names the value
        // node — element 0 of the pair — not the pair itself.
        let value = self.value_of(condition);
        self.register_assert(value, self.loc(e, 0), true);
        let pair = self.term[condition].unwrap();
        self.term[e] = Some(pair);
        self.val[e] = self.val[condition];
        self.ty[e] = self.ty[condition];
        pair
    }

    /// Registers an assert condition: the module worklist entry plus, when
    /// a function body is being compiled, the current function's own list —
    /// the function owns it, so an apply clones it and re-checks the
    /// instantiated condition against each call's argument.  `loc` is
    /// recorded as the runtime attribution edge for the condition node;
    /// `user_facing` marks an explicit `assert` (rendered as a diagnostic) as
    /// opposed to a generated guard (the array-bounds check, which duplicates
    /// the index eval error and is not rendered).  The location is source-blind
    /// (an [`ExprId`]-based [`Loc`]), so no span reaches the lowlevel module.
    fn register_assert(&mut self, condition: NodeId, loc: Loc, user_facing: bool) {
        self.node_edges.insert(condition, loc);
        if user_facing {
            self.user_asserts.insert(condition);
            self.module.add_user_assert(condition);
        } else {
            self.module.add_assert(condition);
        }
        if let Some(function) = self.current_function() {
            self.module.functions[function].asserts.push(condition);
        }
    }

    fn check_tuple_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        for &el in &elements {
            self.check_expr(el);
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
            tys.push(self.check_type_element(el));
        }
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.tuple_type_marker);
        let pair = self.array_node(self.current_block, &[shape, kind]);
        self.term[e] = Some(pair);
        self.val[e] = Some(shape);
        self.ty[e] = Some(kind);
        pair
    }

    /// The type an expression contributes in a type position — a struct
    /// field, a tuple-type element, a function-type side.  There is no
    /// term/type distinction: the expression is used as-is, its pair being
    /// the type it denotes.  A genuine type (a value whose own type is a
    /// kind, or an unbound cell) contributes its pair directly; a *term*
    /// put in a type position contributes its own value pair too, and the
    /// subsequent unification fails (a term's value pair does not unify
    /// with its own type) — `struct<Int, b>` with `b : B` fails, while
    /// `struct<Int, B>` works.
    fn check_type_element(&mut self, el: ExprId) -> NodeId {
        self.check_expr(el);
        self.term[el].unwrap()
    }

    /// A struct type expression.  A struct is *polymorphic*: the nominal
    /// type id identifies its constructor, and instantiating it with field
    /// types produces a concrete type.  So the id belongs in the kind slot
    /// (with `TypeStruct` as an inner tag), not in the value shape, which is
    /// just the field-type list:
    ///
    /// ```text
    /// pair = [ shape, kind ]
    /// shape = [ field types… ]
    /// kind  = [ type_id, [ TypeStruct, K ] ]
    /// ```
    ///
    /// The id is a per-compilation [`P::Operator::from(TypeOperator::Fresh)`] call, so
    /// two occurrences keep distinct nominal ids.  Fields are positional
    /// (no names in v1).
    fn check_type_struct(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let mut tys = Vec::new();
        for &el in &elements {
            tys.push(self.check_type_element(el));
        }
        let id = self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Fresh),
            None,
        );
        let shape = self.array_node(self.current_block, &tys);
        let inner_kind = self.kind_expr(self.current_block, self.type_struct_marker);
        let kind = self.array_node(self.current_block, &[id, inner_kind]);
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
            self.check_expr(el);
            vals.push(self.value_of(el));
            // Found = this element's type, expected = the shared cell: the
            // first element binds the cell, a later one that differs
            // conflicts against it.
            self.check_unify(
                self.ty[el].unwrap(),
                element_ty,
                self.loc(el, 1),
                DiagKind::ArrayElement,
            );
        }
        let value = self.array_node(self.current_block, &vals);
        let length = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::USize(vals.len()))),
        );
        let shape = self.array_node(self.current_block, &[element_ty, length]);
        let kind = self.kind_expr(self.current_block, self.array_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A constant table literal `table { k1 :: v1, k2 :: v2, … }` — the
    /// entries (interleaved key/value ids in the children arena) are checked
    /// like an array's elements, against a shared key-type cell and a shared
    /// value-type cell, and the value is built eagerly by the lowlevel
    /// [`Module::build_table`]: every key is force-evaluated and
    /// deep-content-hashed, an entry whose key is not concrete is dropped
    /// with a recorded [`EvalError::TableKeyUnbound`], and the survivors are
    /// stored sorted by hash.  The type is the kinded pair
    /// `[[key type, value type], [TypeTable, Type]]`.
    fn check_table_term(&mut self, e: ExprId) -> NodeId {
        let entries = self.range_children(e);
        let key_ty = self.fresh_cell();
        let value_ty = self.fresh_cell();
        let mut pairs = Vec::with_capacity(entries.len() / 2);
        for chunk in entries.chunks(2) {
            let (key, value) = (chunk[0], chunk[1]);
            self.check_expr(key);
            self.check_expr(value);
            let key_node = self.value_of(key);
            self.node_edges.insert(key_node, self.loc(key, 0));
            pairs.push((AnyNodeId::Dynamic(key_node), AnyNodeId::Dynamic(self.value_of(value))));
            self.check_unify(
                self.ty[key].unwrap(),
                key_ty,
                self.loc(key, 1),
                DiagKind::TableKey,
            );
            self.check_unify(
                self.ty[value].unwrap(),
                value_ty,
                self.loc(value, 1),
                DiagKind::TableValue,
            );
        }
        let table = self.module.build_table(&pairs, self.current_block);
        let value = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Table(table))),
        );
        let shape = self.array_node(self.current_block, &[key_ty, value_ty]);
        let kind = self.kind_expr(self.current_block, self.table_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// A shallow array `[v1, ~ v2, ~2 v3]` — typed like a tuple (per-element
    /// type slots: a homogeneous `Array` type would reject `[x, ~ f(x+1)]`
    /// with an `Int` head and a `Stream` tail).  The value array carries the
    /// per-position mask: a bare-`~` position's whole subtree stays lazy in
    /// the deep pass (a read forces the single element on demand), and a
    /// `~n` position is wrapped so the value slot at each of the first `n`
    /// levels of its type spine stays shallow.
    fn check_shallow_array_term(&mut self, e: ExprId) -> NodeId {
        let elements = self.range_children(e);
        let depths = self.range_depths(e);
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        let mut mask = Vec::new();
        for (i, &el) in elements.iter().enumerate() {
            self.check_expr(el);
            match depths[i] {
                0 => {
                    vals.push(self.value_of(el));
                    tys.push(self.ty[el].unwrap());
                }
                usize::MAX => {
                    vals.push(self.value_of(el));
                    tys.push(self.ty[el].unwrap());
                }
                n => {
                    // The wrapped term is a lazy region: its value is the
                    // pair chain `[s, [s, … [s, d]]]`, whose structure does
                    // not match the element's own type.  Its reads are
                    // therefore underdetermined — a fresh cell — never a
                    // concrete type that would silently mismatch the
                    // wrapped value.
                    vals.push(self.wrap_shallow(el, n));
                    tys.push(self.fresh_cell());
                }
            }
            mask.push(depths[i] == usize::MAX);
        }
        // `[values, [[element types], [TupleType, Type]]]` — the same shape
        // as a tuple, so reads dispatch on the tuple kind and select the
        // per-element type slot.
        let value = self.array_node_masked(self.current_block, &vals, &mask);
        let shape = self.array_node(self.current_block, &tys);
        let kind = self.kind_expr(self.current_block, self.tuple_type_marker);
        let ty_node = self.array_node(self.current_block, &[shape, kind]);
        let pair = self.pair_of(value, ty_node);
        self.term[e] = Some(pair);
        self.val[e] = Some(value);
        self.ty[e] = Some(ty_node);
        pair
    }

    /// Wrap `e`'s checked term so the value slot at each of the first
    /// `depth` levels of its type spine stays shallow.  Each level is a
    /// fresh pair `[slot0, slot1]` carrying the `shallow=[true, false]`
    /// mask: slot 0 (the value slot) is marked, slot 1 is the next level
    /// down the type spine (the pair's own type slot).  The descent follows
    /// position 1 while the spine is a concrete pair at check time; an
    /// unbound slot ends the descent (the wraps above the stop still apply).
    /// The layers are fresh nodes, so a shared subexpression (a kind
    /// expression reused by every occurrence) is never itself marked.
    fn wrap_shallow(&mut self, e: ExprId, depth: usize) -> NodeId {
        // Level 1 is the element's own `[value, type]` pair.  For a static
        // element that is its stored term; for a call the term is the apply
        // operation node (its pair is virtual), so level 1 is built from
        // the extracted value and type.  Each deeper level is the previous
        // level's type slot's own `[shape, kind]` pair.
        let mut levels: Vec<(NodeId, NodeId)> = Vec::new();
        let mut current = self.term[e].unwrap();
        if self.module.nodes[current].operation.is_some() {
            levels.push((self.value_of(e), self.ty[e].unwrap()));
        } else {
            while levels.len() < depth {
                let Some(items) = self.module.array_items(current) else {
                    break;
                };
                if items.len() != 2 {
                    break;
                }
                let slot0 = self.module.as_dynamic(items[0].node, self.current_block);
                let slot1 = self.module.as_dynamic(items[1].node, self.current_block);
                let descend = self
                    .module
                    .array_items(slot1)
                    .is_some_and(|next| next.len() == 2);
                levels.push((slot0, slot1));
                if !descend {
                    break;
                }
                current = slot1;
            }
        }
        // Rebuild from the innermost level out: each level is a fresh pair
        // [value slot, next] with the shallow mask [true, false].
        let mut wrapped = None;
        for &(slot0, slot1) in levels.iter().rev() {
            let next = wrapped.unwrap_or(slot1);
            wrapped =
                Some(self.array_node_masked(self.current_block, &[slot0, next], &[true, false]));
        }
        wrapped.unwrap_or(self.term[e].unwrap())
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
        self.check_expr(element_type);
        self.check_expr(length);
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

    /// A source-blind location naming `slot` of expression `e` (0 = value,
    /// 1 = type, 2+ = the attribute tail).  The recursive descent beyond the
    /// leading slot is filled by [`Checker::check_unify`] from the lowlevel's
    /// unify `steps`.
    fn loc(&self, e: ExprId, slot: usize) -> Loc {
        let step = match slot {
            0 => LocStep::Value,
            1 => LocStep::Type,
            n => LocStep::Attr(n - 2),
        };
        Loc {
            expr: e,
            path: vec![step],
        }
    }

    fn lookup(&self, target: ExprId) -> Binding {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&target).copied())
            .expect("unresolved parameter (frontend bug)")
    }
}

/// The full-parse walk: append a [`LocStep`] for each unify `step`,
/// classifying the current node as an expression's `[value, type]` pair
/// (→ `Value`/`Type`/`Attr`) or a tuple/array/struct shape (→ `Shape` then
/// `Elem`).
///
/// `start` is the unify's `b`-side top operand (`root_b`); the walk tracks the
/// `step.b` child as it descends.  Both sides of a unify are structurally
/// parallel (unification only descends where both are arrays), so the shape
/// tags are identical whichever side is tracked — the `b` side is just the one
/// the source-blind location is anchored to.
pub(crate) fn tag_descent<P: lichen_lowlevel::Program>(
    module: &Module<P>,
    mut path: Vec<LocStep>,
    start: NodeId,
    steps: &[lichen_lowlevel::UnifyStep],
) -> Vec<LocStep> {
    let mut cur = start;
    let mut in_shape = false;
    for step in steps {
        let tag = if in_shape {
            in_shape = false;
            LocStep::Elem(step.index)
        } else if step.index == 0 {
            if slot0_is_shape(module, cur) {
                in_shape = true;
                LocStep::Shape
            } else {
                LocStep::Value
            }
        } else if step.index == 1 {
            LocStep::Type
        } else {
            LocStep::Attr(step.index - 2)
        };
        path.push(tag);
        cur = step.b;
    }
    path
}

/// Whether `node`'s element 0 is a list — a tuple/array/struct structure (a
/// "shape") rather than an expression's `[value, type]` pair.
fn slot0_is_shape<P: lichen_lowlevel::Program>(module: &Module<P>, node: NodeId) -> bool {
    let Some(items) = module.array_items(node) else {
        return false;
    };
    if items.is_empty() {
        return false;
    }
    match items[0].node {
        AnyNodeId::Dynamic(child) => module.array_items(child).is_some(),
        // A static element is a leaf (a package export); it is never a
        // tuple/array/struct shape we descend into.
        AnyNodeId::Static(_) => false,
    }
}

impl<P: HighProgram> Ctx<P> for Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// The value node for a raw value: a built-in type marker reuses the
    /// checker's installed shared marker node, so the canonical type
    /// expressions are reached in place; anything else allocates a node.
    fn value_node(&mut self, value: P::Value) -> NodeId {
        if value == P::Value::int_marker() {
            self.int_marker
        } else if value == P::Value::function_type_marker() {
            self.function_type_marker
        } else if value == P::Value::tuple_type_marker() {
            self.tuple_type_marker
        } else if value == P::Value::array_type_marker() {
            self.array_type_marker
        } else if value == P::Value::type_struct_marker() {
            self.type_struct_marker
        } else if value == P::Value::table_type_marker() {
            self.table_type_marker
        } else {
            self.alloc_node(self.current_block, None, Some(value))
        }
    }

    fn array_node(&mut self, ids: &[NodeId]) -> NodeId {
        Checker::array_node(self, self.current_block, ids)
    }

    fn op_node(&mut self, op: P::Operator, operand: Option<NodeId>) -> NodeId {
        Checker::op_node(self, self.current_block, op, operand)
    }

    fn pair(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        Checker::pair_of(self, value, ty)
    }

    fn kind_expr(&mut self, marker: NodeId) -> NodeId {
        Checker::kind_expr(self, self.current_block, marker)
    }

    fn fresh(&mut self) -> NodeId {
        Checker::fresh_cell(self)
    }

    fn universe(&self) -> NodeId {
        self.type_expr
    }

    fn int_type(&self) -> NodeId {
        self.int_type
    }

    fn int_marker_node(&self) -> NodeId {
        self.int_marker
    }

    fn type_marker_node(&self) -> NodeId {
        self.type_marker
    }

    fn function_type_marker_node(&self) -> NodeId {
        self.function_type_marker
    }

    fn tuple_type_marker_node(&self) -> NodeId {
        self.tuple_type_marker
    }

    fn array_type_marker_node(&self) -> NodeId {
        self.array_type_marker
    }

    fn type_struct_marker_node(&self) -> NodeId {
        self.type_struct_marker
    }

    fn table_type_marker_node(&self) -> NodeId {
        self.table_type_marker
    }

    fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) {
        Checker::check_unify(self, a, b, loc, kind)
    }

    fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    ) {
        Checker::check_unify_relaxed(self, a, b, loc, kind, is_subtype)
    }

    fn class_value(&self, node: NodeId) -> Option<P::Value> {
        Checker::class_value(self, node)
    }
}

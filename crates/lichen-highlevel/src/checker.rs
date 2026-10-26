//! The checker compiles an [`IR`] into a lowlevel [`Module`].
//! See docs/language-spec.md.
//!
//! # Invariant
//! Every expression compiles to the recursive expr `[value, type]` whose type
//! slot is itself such an expr, and every type spine bottoms out at the
//! self-referential universe `K = [Type, ↺]`. A parameter is such an expr, so
//! the apply-time unify is the function-parameter check.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

use lichen_lowlevel::{
    AnyNodeId, ArrayItem, ArrayValue, BlockId, BudgetExhausted, FunctionId, LowOperator, LowValue,
    Module, NodeId, Operation, Registry,
};
use stacksafe::stacksafe;

use crate::attr::{AttrExtRegistry, AttrSet};
use crate::diagnostic::{AssertSpelling, DiagKind, DiaryEntry};
use crate::ir::{BinOp, ChildRange, ExprId, ExprKind, IR, Loc, LocPath, PrefixOp};
use crate::native::{NativeArg, NativeOps, no_native_ops};

/// What a build does after checking: run the program, or decide only types.
///
/// # Invariant
/// **`Check` evaluates the type slot of every expression and never the value slot.** Types are
/// values in this language and can participate in computation, so `Check` still evaluates — it
/// is the *value* slot that is never entered, which is what keeps a divergent `@loop` out of
/// the checking path. `Evaluate` is the full build the compiler and the tests run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Check, then run every body and every top-level statement (the definition pass).
    Evaluate,
    /// Check, then decide every expression's type slot; no body runs.
    Check,
}
use crate::program::{Ctx, HighProgram, LiteralExt, TypeOperator, ValueType};
use crate::shape::{EXPR_ATTR_BASE, for_each_kind_marker, slot_width};

mod annotations;
mod asserts;
mod diagnostics;
mod gc;
mod indexing;
mod lambda;
mod native_call;
mod operators;
mod structs;
mod templates;
mod tuples;
mod values;

// These macros expand the kind-marker registry list into marker plumbing.

/// The [`Markers`] struct: the field set is the kind-marker list.
macro_rules! define_markers {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        /// The installed shared kind-marker nodes, one per marker, in registry
        /// order and never rebuilt.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub struct Markers {
            $(
                #[doc = concat!("The installed shared `", $display, "` marker node.")]
                pub $marker_fn: NodeId,
            )*
        }
    };
}
for_each_kind_marker!(define_markers);

/// The `install_constants` allocation: one shared marker node, in registry
/// order.
macro_rules! define_install_markers {
    ([ $this:ident, $root:ident ] $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $( $this.markers.$marker_fn =
            $this.alloc_node($root, None, Some(ValueType::$marker_fn())); )*
    };
}

/// The `Ctx` marker-accessor impls: each returns its marker's shared node.
macro_rules! define_ctx_marker_accessor_impls {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            fn $node_fn(&self) -> NodeId {
                self.markers.$marker_fn
            }
        )*
    };
}

/// The `Ctx::value_node` dispatch: a built-in type marker reuses the installed
/// shared node; anything else allocates one.
macro_rules! define_value_node_dispatch {
    ([ $this:ident, $value:ident ] $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $( if $value == ValueType::$marker_fn() {
            return $this.markers.$marker_fn;
        } )*
    };
}

/// A parameter in scope: its `[value, type]` expr and its type cell.
///
/// # Invariant
/// A use references the expr, so the apply's clone always includes it — the
/// lowlevel runs the argument unify only when the parameter was cloned.
#[derive(Clone, Copy)]
struct Binding {
    term: NodeId,
    ty: NodeId,
}

pub struct Checker<P: HighProgram + 'static>
where
    P::Value: ValueType,
{
    /// The frontend's IR, shared and read-only.
    ///
    /// # Invariant
    /// The `Arc` makes that compiler-checked: `self.ir.set_kind` is E0596.
    /// `Arc::get_mut` is the one route left open, deliberately, because closing
    /// it would thread an `&IR` lifetime through every consumer.
    ir: Arc<IR<P::Attr, P::Literal>>,
    module: Module<P>,
    pub current_block: BlockId,
    /// The attribute extension registry: an attribute marker to its lowering
    /// behaviour, or `None` without one.
    ///
    /// # Invariant
    /// The checker names no concrete attribute, and a marker it cannot resolve
    /// is a reported guard failure, never a panic.
    attr_ext: Option<AttrExtRegistry<P, P::Attr>>,
    /// Whether [`Checker::no_attr_ext_guard`] already reported this build's
    /// missing attribute extension.
    no_attr_ext_reported: bool,
    /// The native-operator registry (see [`crate::native`]).
    ///
    /// # Invariant
    /// Only the checker resolves a `$name` against this slice, so an
    /// unregistered name is a reported guard failure at the call's span.
    native_ops: NativeOps<P>,
    scopes: Vec<HashMap<ExprId, Binding>>,
    /// The lexical function stack: one entry per lambda body being compiled,
    /// innermost last.
    ///
    /// # Invariant
    /// The innermost entry is the current function, and every node the checker
    /// allocates is tagged with it; which function is a lambda's parent is the
    /// frontend's decision, never this stack's.
    function_stack: Vec<(FunctionId, Option<ExprId>)>,
    /// Every lambda's allocated [`FunctionId`], keyed by its IR expression.
    function_of: HashMap<ExprId, FunctionId>,
    /// The enclosing lambdas' parameters, innermost last: an attribute naming
    /// one of their fields is a **reference**.
    current_parameters: Vec<ExprId>,
    /// Placeholder callees for which constructor recovery succeeded.
    placeholder_constructors: std::collections::HashSet<ExprId>,
    /// The `@gc` expressions this build compiled.
    ///
    /// # Invariant
    ///
    /// A non-empty list is what makes [`Checker::prune_released`] run: a
    /// collected scope releases the nodes its root does not reach, and every
    /// table here may name one. Empty skips the pass.
    gc_scopes: Vec<ExprId>,
    /// The per-expression checker state — one entry per IR expression, see
    /// [`ExprState`].  Indexed by [`ExprId`].
    state: Vec<ExprState>,
    /// The checker's own per-annotation merged attribute tails.
    ///
    /// # Invariant
    /// The IR stays the frontend's: only this table carries the merged tail.
    merged_tails: HashMap<ExprId, Vec<P::Attr>>,
    /// The attributes a body requires of its own parameter, replayed at each apply.
    ///
    /// # Invariant
    /// The requirement is consumed by the read that states it, so a caller's slot is
    /// only compared with it where the apply supplies one
    /// (`docs/notes/array-attribute-view.md`).
    deferred_requirements: HashMap<FunctionId, Vec<(P::Attr, NodeId)>>,
    /// The attribute each expr's **attribute slot node** belongs to.
    ///
    /// # Invariant
    /// Recorded where the expr is built, so a failed unify names its own attribute and
    /// the order consulted for it is that attribute's and no other
    /// (`docs/notes/array-attribute-view.md`).
    attribute_nodes: HashMap<NodeId, P::Attr>,
    /// The checker's own check sequence, attributed for diagnostics.
    ///
    /// # Invariant
    /// `diary` is in [`Self::check_seq`] order, so the output order comes from
    /// [`DiaryEntry::seq`] rather than from the vec's position.
    diary: Vec<DiaryEntry>,
    /// The monotonic counter behind [`DiaryEntry::seq`].
    check_seq: usize,
    /// Arrow nodes the type printer reads as `in -> out`.
    ///
    /// # Invariant
    ///
    /// Empty by construction: the language has no arrow type, so no node is
    /// registered here. The printer's bare `[in, out]` arrow arm therefore never
    /// fires, and no reading depends on this being non-empty.
    arrows: HashSet<NodeId>,
    /// The apply edges, keyed by apply op node.
    apply_edges: HashMap<NodeId, ApplyEdge>,
    /// Each apply's **type-only** application, keyed by the apply's expression.
    type_applies: HashMap<ExprId, NodeId>,
    /// Each marked position's `(type application, tail cell, inner type)`.
    ///
    /// # Invariant
    /// The type application is the marked call's own, and it is `None` for an
    /// element that is not a call. The tail cell is the position's declared
    /// type, and the inner type is the element's own: the tail closes on the
    /// inner one once the application has decided it.
    shallow_types: Vec<(Option<NodeId>, NodeId, NodeId)>,
    /// The wrapper nodes this checker builds for `@shallow(n)` elements.
    ///
    /// # Invariant
    /// The set is a construction fact, like [`Self::arrows`]: a reader asks whether a
    /// node *is* a wrapper without reading a value, so the answer is the same before
    /// and after the values are decided.
    shallow_regions: HashSet<NodeId>,
    /// A node a runtime failure references, mapped to its location.
    ///
    /// # Invariant
    /// No span is stored on a node, which is what lets the graph be shared.
    node_edges: HashMap<NodeId, Loc>,
    /// An array read's internal `i + 1` subscript, mapped to the source `i`.
    array_subscripts: HashMap<NodeId, NodeId>,
    /// The read whose derived nodes are being built.
    ///
    /// # Invariant
    /// Set by a read's check before it builds a node no expression slot owns, so
    /// [`Self::name_node`] and the read's slot nodes are attributed to that read.
    read_site: Option<Loc>,
    /// The user-facing assert condition nodes: the explicit `assert` expressions.
    ///
    /// # Invariant
    /// Only these render as `DiagKind::Assert`; the bounds guard has its own
    /// error.
    user_asserts: HashSet<NodeId>,
    /// How a failed assert of each registered condition should read.
    assert_spellings: HashMap<NodeId, AssertSpelling>,

    /// The value node of every lambda in the program.
    ///
    /// # Invariant
    /// The write is unconditional: a non-recursive lambda contributes too.
    lambda_value_nodes: Vec<NodeId>,
    /// The top-level statements the definition pass found non-terminating.
    ///
    /// # Invariant
    /// Each carries a source-blind [`Loc`] and the budget the guard recorded:
    /// the build reports such a statement as an error, never a panic.
    nonterminating: Vec<NonTerminating>,
    // The installed shared kind-marker nodes, one per marker.
    markers: Markers,
    /// The shared `[int, Type]` type expression every literal's expr carries.
    int_type: NodeId,
    /// The shared `[float, Type]` type expression, paired into every `Float`
    /// literal.
    float_type: NodeId,
    /// The shared `[string, Type]` type expression, paired into every `Str`
    /// literal.
    string_type: NodeId,
    /// The shared **`Tuple` template** — the `[TupleType, Type]` expression every
    /// tuple value's type is.
    ///
    /// # Invariant
    /// One node for the whole program: a tuple value's type is this node, so a
    /// read can require the tuple layout by unifying with it, and two tuple
    /// values are one type.
    tuple_template: NodeId,
    /// The shared **`Array` template** — the `[ArrayType, Type]` expression every
    /// array value's type is.
    ///
    /// # Invariant
    /// One node, as [`Self::tuple_template`]'s is.
    array_template: NodeId,
    /// The shared **`Struct` template** — the self-referential `[StructType, ↺]`
    /// expression every struct value's type is.
    ///
    /// # Invariant
    /// Its type slot is itself: the struct layout closes on itself, so the
    /// template needs no second node and no payload.
    struct_template: NodeId,
    /// The shared wildcard value node: a compound type's unconstrained positions.
    ///
    /// # Invariant
    /// One node is enough: unification neither merges nor writes it, so no class
    /// can bind it and no use is distinguishable from another.
    any_value: NodeId,
    /// The canonical universe `[Type, ↺]`.
    type_expr: NodeId,
    // The two shared index constants.
    //
    // Invariant: allocated before any function exists, so untagged.
    /// The shared `USize(0)` node — element 0 of an expression's expr.
    zero_value: NodeId,
    /// The shared `USize(1)` node — element 1 of an expression's expr.
    one_value: NodeId,
    /// One shared absent-occurrence slot per attribute that opted in, indexed by
    /// its position in [`AttrSet::ORDER`].
    ///
    /// # Invariant
    /// The attribute decides whether its own missing value may be shared (see
    /// [`crate::attr::AttrExt::share_missing_slot`]): `None` here is the
    /// attribute declining.
    missing_slots: Vec<Option<NodeId>>,
    /// The interned field-name nodes — one per unique field name, so every
    /// occurrence of `.a` reads the same key node.
    name_nodes: HashMap<&'static str, NodeId>,
}

/// One application's argument edge, recorded when the apply is wired.
///
/// # Invariant
/// The edge carries the attribution, so a runtime parameter-check failure names
/// the argument's own source span even when its node is reused.
#[derive(Clone, Copy, Debug)]
pub struct ApplyEdge {
    /// The argument's IR expression — the caret target.
    pub argument_expr: ExprId,
    /// The whole apply's IR expression (the call site) — context.
    pub apply_expr: ExprId,
}

/// One non-terminating statement the definition pass abandoned: where it is,
/// and the budget the VM's guard refused on.
#[derive(Clone, Debug)]
pub struct NonTerminating {
    /// Where the abandoned binding is.
    pub loc: Loc,
    /// The budget that ran out, recorded by the lowlevel instead of panicking so
    /// the diagnostic can name it.
    pub budget: Option<BudgetExhausted>,
}

/// One IR expression's checker state, in one entry.
///
/// # Invariant
/// Every field is recorded at the vector's ordinal for [`ExprId`], so no field
/// can name a different expression than the rest.
#[derive(Clone, Copy, Debug, Default)]
pub struct ExprState {
    /// The compiled expr node `[value, type]`.
    pub term: Option<NodeId>,
    /// Element 0 of the expr (the value).
    ///
    /// # Invariant
    /// It is stored for a static node and built lazily as `Index(expr, 0)` for a
    /// call result.
    pub val: Option<NodeId>,
    /// Element 1 of the expr (the type).
    pub ty: Option<NodeId>,
    /// The constraint attribute slot, which the apply-time check reads.
    ///
    /// # Invariant
    /// It is `None` without a `# p` constraint; a label slot (e.g. `Doc`) is
    /// metadata and never appears here.
    pub attr: Option<NodeId>,
}

impl std::ops::Index<ExprId> for Vec<ExprState> {
    type Output = ExprState;
    fn index(&self, id: ExprId) -> &ExprState {
        &self[id.0 as usize]
    }
}

impl std::ops::IndexMut<ExprId> for Vec<ExprState> {
    fn index_mut(&mut self, id: ExprId) -> &mut ExprState {
        &mut self[id.0 as usize]
    }
}

/// The result of one [`Checker::build`]: the module the lowlevel built and
/// the attributed per-expression nodes.
pub struct Build<P: HighProgram>
where
    P::Value: ValueType,
{
    /// The IR this build checked, shared with the [`Checker`] that read it.
    pub ir: Arc<IR<P::Attr, P::Literal>>,
    pub module: Module<P>,
    /// The per-expression checker state, indexed by [`ExprId`].
    pub state: Vec<ExprState>,
    pub root_term: NodeId,
    pub root_val: NodeId,
    pub root_ty: NodeId,
    /// The root expression's attribute tail, as the checker computed it ([`Checker::schema_tail`]).
    ///
    /// # Invariant
    /// It is index-aligned with the runtime expr's attribute slots
    /// (`build.root_term`), which are built at the merged width — not with the
    /// IR's own stamp, which holds only what the source spelled.
    pub root_schema_tail: Vec<P::Attr>,
    /// The installed shared kind-marker nodes.
    pub markers: Markers,
    // Three markers in their pre-Phase-1 flat spelling, kept for existing
    // readers; `markers` carries the full set.
    pub int_marker: NodeId,
    pub string_marker: NodeId,
    pub type_marker: NodeId,
    /// The shared `[int, Type]` type expression.
    pub int_type: NodeId,
    /// The shared `[float, Type]` type expression.
    pub float_type: NodeId,
    /// The shared `[string, Type]` type expression.
    pub string_type: NodeId,
    /// The canonical universe `[Type, ↺]`.
    pub type_expr: NodeId,
    /// The checker's attributed checks, one per check it issued.
    pub diary: Vec<DiaryEntry>,
    /// Arrow nodes; read by the diagnostics.
    pub arrows: HashSet<NodeId>,
    /// The apply edges keyed by apply op node (see [`ApplyEdge`]).
    pub apply_edges: HashMap<NodeId, ApplyEdge>,
    /// The runtime-attribution edges (see [`Checker::node_edges`]).
    pub node_edges: HashMap<NodeId, Loc>,
    /// An array read's internal subscript, mapped to its source subscript.
    pub array_subscripts: HashMap<NodeId, NodeId>,
    /// The user-facing assert condition nodes (see [`Checker::user_asserts`]).
    pub user_asserts: HashSet<NodeId>,
    /// How a failed assert of each registered condition should read (see
    /// [`AssertSpelling`]).
    pub assert_spellings: HashMap<NodeId, AssertSpelling>,

    /// The top-level statements the definition pass found non-terminating.
    pub nonterminating: Vec<NonTerminating>,
    pub ok: bool,
    /// The lowering map, built on the first read. See [`crate::lowering`].
    ///
    /// # Invariant
    /// A `OnceLock` rather than a built field: attribution is read only when a
    /// node has to be named — a diagnostic, a test — and paying an O(n) table
    /// on every build to answer a question nobody asked is the wrong trade.
    pub(crate) lowerings: OnceLock<HashMap<NodeId, Loc>>,
}

/// The compile work budget one [`Checker`] build runs under.
///
/// # Invariant
/// The budget is the caller's to set because a *terminating* program whose
/// definition pass exceeds it is reported as [`DiagKind::NonTerminating`],
/// exactly like an infinite loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkBudget {
    /// Nested applications permitted before the guard refuses.
    ///
    /// # Invariant
    /// Depth is the applied node's own, so an expansion reaches the bound at its trip
    /// count while a converted loop's iterations never do. A loop spends no count and is
    /// therefore unbounded: the guard is a stack bound, not a work bound.
    pub apply_depth_limit: usize,
}

impl Default for WorkBudget {
    fn default() -> Self {
        WorkBudget {
            apply_depth_limit: 500,
        }
    }
}

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// Compile an IR with a fresh private registry and no attribute extension,
    /// under the default [`WorkBudget`].
    pub fn build(ir: IR<P::Attr, P::Literal>) -> Build<P> {
        Self::build_with_budget(ir, WorkBudget::default())
    }

    /// [`Self::build_in_attr_native`] with an explicit [`Mode`].
    pub fn build_in_attr_native_with_mode(
        ir: IR<P::Attr, P::Literal>,
        registry: Arc<RwLock<Registry<P>>>,
        attr_ext: AttrExtRegistry<P, P::Attr>,
        native_ops: NativeOps<P>,
        mode: Mode,
    ) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(
            ir,
            module,
            Some(attr_ext),
            native_ops,
            WorkBudget::default(),
            mode,
        )
    }

    /// [`Self::build`] under a caller-supplied [`WorkBudget`].
    pub fn build_with_budget(ir: IR<P::Attr, P::Literal>, work_budget: WorkBudget) -> Build<P> {
        Self::build_with(
            ir,
            Module::new(),
            None,
            no_native_ops(),
            work_budget,
            Mode::Evaluate,
        )
    }

    /// Compile an IR whose module is bound to a caller-provided shared registry.
    pub fn build_in(ir: IR<P::Attr, P::Literal>, registry: Arc<RwLock<Registry<P>>>) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(
            ir,
            module,
            None,
            no_native_ops(),
            WorkBudget::default(),
            Mode::Evaluate,
        )
    }

    /// Compile an IR with a caller-supplied attribute extension registry.
    ///
    /// # Invariant
    /// `attr_ext` maps an attribute marker to its lowering behaviour; the
    /// checker never names a concrete attribute.
    pub fn build_in_attr(
        ir: IR<P::Attr, P::Literal>,
        registry: Arc<RwLock<Registry<P>>>,
        attr_ext: AttrExtRegistry<P, P::Attr>,
    ) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(
            ir,
            module,
            Some(attr_ext),
            no_native_ops(),
            WorkBudget::default(),
            Mode::Evaluate,
        )
    }

    /// [`Self::build_in_attr`] with a native-operator registry.
    ///
    /// # Invariant
    /// `native_ops` is that plugin's private registry, empty for an ordinary
    /// file.
    pub fn build_in_attr_native(
        ir: IR<P::Attr, P::Literal>,
        registry: Arc<RwLock<Registry<P>>>,
        attr_ext: AttrExtRegistry<P, P::Attr>,
        native_ops: NativeOps<P>,
    ) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(
            ir,
            module,
            Some(attr_ext),
            native_ops,
            WorkBudget::default(),
            Mode::Evaluate,
        )
    }

    /// The shared body of every entry point above: compile `ir` against
    /// `module`.
    ///
    /// # Invariant
    /// Normalization is a function's birth: every lambda's value node is deep-evaluated
    /// before the definition pass, so a new function's own cells settle once. A later
    /// read sees decided cells or reports the open one; an apply is not a normalize.
    fn build_with(
        ir: IR<P::Attr, P::Literal>,
        mut module: Module<P>,
        attr_ext: Option<AttrExtRegistry<P, P::Attr>>,
        native_ops: NativeOps<P>,
        work_budget: WorkBudget,
        mode: Mode,
    ) -> Build<P> {
        // order; checked once per program.
        debug_assert!(
            crate::attr::order_is_canonical::<P::Attr>(),
            "an attribute's canonical index must be its position in the set's order"
        );
        // A `Check` build reports the type side's parameter check; `Evaluate` lets the value side.
        module.set_type_side_reporting(mode == Mode::Check);

        // The definition pass runs the whole program, so the limits must be set
        // before it starts.
        module.apply_depth_limit = work_budget.apply_depth_limit;

        let root_block = module.add_block(None);
        let n = ir.expr.len();
        let mut checker = Checker {
            ir: Arc::new(ir),
            module,
            current_block: root_block,
            attr_ext,
            no_attr_ext_reported: false,
            native_ops,
            scopes: Vec::new(),
            function_stack: Vec::new(),
            function_of: HashMap::new(),
            current_parameters: Vec::new(),
            placeholder_constructors: std::collections::HashSet::new(),
            gc_scopes: Vec::new(),
            state: vec![ExprState::default(); n],
            merged_tails: HashMap::new(),
            deferred_requirements: HashMap::new(),
            attribute_nodes: HashMap::new(),
            diary: Vec::new(),
            check_seq: 0,
            arrows: HashSet::new(),
            apply_edges: HashMap::new(),
            type_applies: HashMap::new(),
            shallow_types: Vec::new(),
            shallow_regions: HashSet::new(),
            node_edges: HashMap::new(),
            array_subscripts: HashMap::new(),
            read_site: None,
            user_asserts: HashSet::new(),
            assert_spellings: HashMap::new(),

            lambda_value_nodes: Vec::new(),
            nonterminating: Vec::new(),
            markers: Markers::default(),
            int_type: NodeId::default(),
            float_type: NodeId::default(),
            string_type: NodeId::default(),
            tuple_template: NodeId::default(),
            array_template: NodeId::default(),
            struct_template: NodeId::default(),
            any_value: NodeId::default(),
            type_expr: NodeId::default(),
            zero_value: NodeId::default(),
            one_value: NodeId::default(),
            name_nodes: HashMap::new(),
            missing_slots: vec![None; P::Attr::ORDER.len()],
        };
        checker.install_constants();
        // Prove the canonical structures concrete before the definition pass, so
        // the clone walk references them in place.
        checker.module.evaluate_node_deep(checker.type_expr, None);
        checker.module.evaluate_node_deep(checker.int_type, None);
        checker.module.evaluate_node_deep(checker.float_type, None);
        checker.module.evaluate_node_deep(checker.string_type, None);
        // The compound templates are concrete before the definition pass.
        checker
            .module
            .evaluate_node_deep(checker.tuple_template, None);
        checker
            .module
            .evaluate_node_deep(checker.array_template, None);
        checker
            .module
            .evaluate_node_deep(checker.struct_template, None);
        let root = checker.ir.root;
        // Type-check every user-written top-level statement, then the final root.
        let stmt_roots = checker.ir.stmt_roots.clone();
        let mut evaluated_lambdas = 0;
        for &s in &stmt_roots {
            checker.check_expr(s);
            // Later statements read the shallow types their predecessors decide.
            for &func_node in &checker.lambda_value_nodes[evaluated_lambdas..] {
                checker.module.evaluate_node_deep(func_node, None);
            }
            evaluated_lambdas = checker.lambda_value_nodes.len();
            checker.resolve_shallow_types();
        }
        let root_term = checker.check_expr(root);
        let root_ty = checker.state[root]
            .ty
            .expect("the root expression must have a type");
        // Deep-evaluate every lambda's value node before the definition pass, so a
        // recursive reference stays in place.
        for &func_node in &checker.lambda_value_nodes {
            checker.module.evaluate_node_deep(func_node, None);
        }
        // A declared type is a fact about the template: every written type term is
        // decided here, before any body runs.
        for index in 0..checker.ir.expr.len() {
            let expression = ExprId(index as u32);
            let is_type = matches!(
                checker.ir[expression].kind,
                ExprKind::TypeSet(_) | ExprKind::TypeTable(_)
            );
            if is_type && let Some(term) = checker.state[expression].term {
                checker.module.evaluate_node_deep(term, None);
            }
        }
        // A `@shallow` mark is on the value; its element's type is applied here, and an open tail then closes.
        checker.resolve_shallow_types();
        // The definition pass: run the program so the apply-time checks fire.
        if mode == Mode::Evaluate && !checker.check_failed() {
            // Its order is user-visible, so functions are sorted by the IR
            // expression they were compiled from.
            let expression_of: HashMap<FunctionId, ExprId> = checker
                .function_of
                .iter()
                .map(|(&expression, &function)| (function, expression))
                .collect();
            let mut functions: Vec<FunctionId> = checker.module.functions.keys().collect();
            functions.sort_by_key(|function| {
                expression_of
                    .get(function)
                    .map_or(u32::MAX, |expression| expression.0)
            });
            for function in functions {
                let (ret, asserts) = {
                    let function = &checker.module.functions[function];
                    (function.r#return, function.asserts.clone())
                };
                // Every body runs once, so its checks fire even unapplied.
                //
                // Invariant: the deep pass pins what the clone copies.
                checker.module.evaluate_node_deep(ret, None);
                // The return's proof extends to the body's asserts, each its own
                // reachability entry point.
                for &condition in &asserts {
                    checker.module.evaluate_node_deep(condition, None);
                }
            }
        }
        // A marked site's "not emitted" verdict is the reader's to report.
        // See docs/notes/loop-conversion.md.

        // Evaluate every user-written top-level statement, so a
        // non-terminating one is a diagnostic, not an aborted build.
        if mode == Mode::Evaluate && !checker.check_failed() {
            let mut fatal = false;
            for &s in &stmt_roots {
                let Some(term) = checker.state[s].term else {
                    continue;
                };
                checker.module.evaluate_node_deep(term, None);
                // Copied out: the borrow of `checker.module` must end before
                // the `nonterminating` push below.
                let Some(budget) = checker.module.budget_exhausted else {
                    continue;
                };
                checker.nonterminating.push(NonTerminating {
                    loc: Loc {
                        expr: s,
                        path: LocPath::Empty,
                    },
                    budget: Some(budget),
                });
                fatal = true;
                break;
            }
            if !fatal {
                checker.module.evaluate_node_deep(root_term, None);
                if let Some(budget) = checker.module.budget_exhausted {
                    checker.nonterminating.push(NonTerminating {
                        loc: Loc {
                            expr: root,
                            path: LocPath::Empty,
                        },
                        budget: Some(budget),
                    });
                }
            }
        }
        // `Check`: every expression's type slot is decided; no value slot is entered.
        if mode == Mode::Check && !checker.check_failed() {
            for index in 0..checker.ir.expr.len() {
                let expression = ExprId(index as u32);
                if let Some(ty) = checker.state[expression].ty {
                    checker.module.evaluate_node_deep(ty, None);
                }
            }
        }

        // The assert pass: drain the constraint worklist, requiring `USize(1)` of
        // each deep-evaluated condition.
        if !checker.check_failed() && checker.nonterminating.is_empty() {
            checker.module.check_asserts();
            // A depth-budget stop in the drain is a diagnostic, not a silent pass.
            if let Some(budget) = checker.module.budget_exhausted
                && checker.nonterminating.is_empty()
            {
                checker.nonterminating.push(NonTerminating {
                    loc: Loc {
                        expr: root,
                        path: LocPath::Empty,
                    },
                    budget: Some(budget),
                });
            }
        }
        // A released scope takes the edges into it.
        if !checker.gc_scopes.is_empty() {
            checker.prune_released();
        }
        // **Every failed unify asks its attribute's order** (`docs/notes/array-attribute-view.md`).
        let mut pardoned = Vec::new();
        for index in 0..checker.module.unify_errors.len() {
            let error = &checker.module.unify_errors[index];
            let attribute = checker
                .attribute_nodes
                .get(&error.root_a)
                .or_else(|| checker.attribute_nodes.get(&error.root_b))
                .copied();
            let Some(marker) = attribute else {
                continue;
            };
            let admitted = checker
                .attribute_extension(&marker)
                .is_some_and(|ext| ext.is_subtype(&checker, error.root_a, error.root_b));
            if admitted {
                pardoned.push(index);
            }
        }
        if !pardoned.is_empty() {
            let errors = std::mem::take(&mut checker.module.unify_errors);
            checker.module.unify_errors = errors
                .into_iter()
                .enumerate()
                .filter(|(index, _error)| !pardoned.contains(index))
                .map(|(_index, error)| error)
                .collect();
        }
        // A build is not ok when a unification failed or a guard was recorded.
        let ok = !checker.check_failed()
            && checker.module.eval_errors.is_empty()
            && checker.module.assert_errors.is_empty()
            && !checker.module.has_extension_failures()
            && checker.nonterminating.is_empty();
        let root_val = checker.value_of(root);
        let root_schema_tail = checker.schema_tail(root).to_vec();
        // The definition pass syncs an apply's result cell with the return
        // expr, so an unannotated call takes the return type.
        Build {
            ir: checker.ir,
            module: checker.module,
            state: checker.state,
            root_term,
            root_val,
            root_ty,
            root_schema_tail,
            markers: checker.markers,
            int_marker: checker.markers.int_marker,
            string_marker: checker.markers.string_marker,
            type_marker: checker.markers.type_marker,
            int_type: checker.int_type,
            float_type: checker.float_type,
            string_type: checker.string_type,
            type_expr: checker.type_expr,
            diary: checker.diary,
            arrows: checker.arrows,
            apply_edges: checker.apply_edges,
            node_edges: checker.node_edges,
            array_subscripts: checker.array_subscripts,
            user_asserts: checker.user_asserts,
            assert_spellings: checker.assert_spellings,

            nonterminating: checker.nonterminating,
            ok,
            lowerings: OnceLock::new(),
        }
    }

    /// Drop every checker entry that names a node a `@gc` scope released.
    ///
    /// # Invariant
    ///
    /// The scope is a boundary: what it released is not part of this build's
    /// graph any more, so an expression inside one reads as having no compiled
    /// slot rather than naming a released node. A surviving scope's root and
    /// every node reachable from it are still live, so only its unreachable
    /// interior is dropped.
    fn prune_released(&mut self) {
        let live = |node: NodeId| self.module.nodes.contains_key(node);
        for state in &mut self.state {
            state.term = state.term.filter(|&node| live(node));
            state.val = state.val.filter(|&node| live(node));
            state.ty = state.ty.filter(|&node| live(node));
            state.attr = state.attr.filter(|&node| live(node));
        }
        self.arrows.retain(|&node| live(node));
        self.node_edges.retain(|&node, _| live(node));
        self.array_subscripts
            .retain(|&node, source| live(node) && live(*source));
        self.apply_edges.retain(|&node, _| live(node));
        self.user_asserts.retain(|&node| live(node));
        self.assert_spellings.retain(|&node, _| live(node));
    }

    /// Whether any check already failed: a failed unification **or** a recorded
    /// guard failure.
    /// # Invariant
    /// A guard entry is exactly one whose [`DiaryEntry::errors`] is `None`: the
    /// kind of check it was is a recorded fact, never an inference from an
    /// empty range.
    fn check_failed(&self) -> bool {
        !self.module.unify_errors.is_empty()
            || self.diary.iter().any(|entry| entry.errors.is_none())
    }

    /// An expression's attribute tail: the annotation's merged tail when
    /// recorded, else the frontend's own IR stamp.
    ///
    /// # Invariant
    /// Only this table holds merged tails; every other tail lives in the IR,
    /// which the fallback reaches.
    fn schema_tail(&self, e: ExprId) -> &[P::Attr] {
        self.merged_tails
            .get(&e)
            .map_or_else(|| self.ir.schema(e).tail.as_slice(), |tail| tail.as_slice())
    }

    /// The type constants as plain nodes in the root block.
    ///
    /// # Invariant
    /// The universe `K = [Type, ↺]`, whose type slot is itself, closes every
    /// type spine, and `[int, K]` is the shared int type expression. Proving
    /// these concrete before the definition pass pins them in place: a clone
    /// of the universe would build a fresh self-loop no unify can equate with.
    fn install_constants(&mut self) {
        let root = self.current_block;
        // The kind markers, one shared node each, in registry order.
        for_each_kind_marker!(define_install_markers[self, root]);
        // The wildcard: one node, because unification neither merges nor writes it.
        self.any_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::Any)));
        // `K = [Type, K]`: point its type slot at itself; the lowlevel deep
        // pass cuts the self-loop it reaches.
        let universe = self.alloc_node(root, None, None);
        let mut items = vec![
            ArrayItem::new(AnyNodeId::Dynamic(self.markers.type_marker)),
            ArrayItem::new(AnyNodeId::Dynamic(universe)),
        ];
        items.extend(std::iter::repeat_n(
            ArrayItem::new(AnyNodeId::Dynamic(self.any_value)),
            P::Attr::ORDER.len(),
        ));
        let layout = self.full_width_layout();
        self.module.write_node_value(
            universe,
            Some(P::Value::from(LowValue::Array(ArrayValue::with_layout(
                self.module.alloc_array(&items, root),
                layout,
            )))),
        );
        self.type_expr = universe;
        self.int_type = self.kind_expr(root, self.markers.int_marker);
        self.float_type = self.kind_expr(root, self.markers.float_marker);
        self.string_type = self.kind_expr(root, self.markers.string_marker);
        // The compound **templates**: the shared layout expressions a
        // compound value's type is.
        self.tuple_template = self.kind_expr(root, self.markers.tuple_type_marker);
        self.array_template = self.kind_expr(root, self.markers.array_type_marker);
        self.struct_template = self.self_referential_template(root);
        // The positional-read constants, the subscripts of `Index(expr, 0)` and
        // `Index(expr, 1)`, shared with the markers above.
        self.zero_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(0))));
        self.one_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(1))));
        // The default attribute array: a hole reads a wildcard
        // (`docs/notes/array-attribute-view.md`).
        let cell = ArrayItem::new(AnyNodeId::Dynamic(self.any_value));
        let defaults = vec![cell; crate::shape::EXPR_ATTR_BASE + P::Attr::ORDER.len()];
        let defaults = self.module.alloc_array(&defaults, root);
        self.module.set_default_attributes(defaults);
        // The full width, for the operators that build an expression of their own.
        let full_width = self.full_width_layout();
        self.module.record_full_width_layout(full_width);
    }

    /// The shared `USize(0)` node — the `Index(expr, 0)` subscript every value
    /// read and expr-slot descent uses.
    pub(super) fn zero(&self) -> NodeId {
        self.zero_value
    }
    /// The shared **`Tuple` template** — the `[TupleType, Type]` expression.
    pub(super) fn tuple_template(&self) -> NodeId {
        self.tuple_template
    }

    /// The shared **`Array` template** — the `[ArrayType, Type]` expression.
    pub(super) fn array_template(&self) -> NodeId {
        self.array_template
    }

    /// The shared **`Struct` template** — the `[StructType, ↺]` expression.
    pub(super) fn struct_template(&self) -> NodeId {
        self.struct_template
    }

    /// A **self-referential** template expression `[marker, ↺]`: its type slot
    /// is itself.
    ///
    /// # Invariant
    /// The type slot is the node itself, so the layout closes on itself and no
    /// second node carries it — the universe is built the same way.
    fn self_referential_template(&mut self, block: BlockId) -> NodeId {
        let template = self.alloc_node(block, None, None);
        let mut items = vec![
            ArrayItem::new(AnyNodeId::Dynamic(self.markers.type_struct_marker)),
            ArrayItem::new(AnyNodeId::Dynamic(template)),
        ];
        items.extend(std::iter::repeat_n(
            ArrayItem::new(AnyNodeId::Dynamic(self.any_value)),
            P::Attr::ORDER.len(),
        ));
        let layout = self.full_width_layout();
        self.module.write_node_value(
            template,
            Some(P::Value::from(LowValue::Array(ArrayValue::with_layout(
                self.module.alloc_array(&items, block),
                layout,
            )))),
        );
        template
    }
    /// The shared `USize(1)` node — the `Index(expr, 1)` subscript every type
    /// read uses.
    pub(super) fn one(&self) -> NodeId {
        self.one_value
    }

    /// The shared wildcard value node — the `@any` spelling's value.
    pub(super) fn any(&self) -> NodeId {
        self.any_value
    }

    /// A constant `USize` node — a subscript or table key a rule states rather
    /// than computes; `0` and `1` are shared.
    pub(super) fn usize_node(&mut self, n: usize) -> NodeId {
        match n {
            0 => self.zero(),
            1 => self.one(),
            _ => self.alloc_node(
                self.current_block,
                None,
                Some(P::Value::from(LowValue::USize(n))),
            ),
        }
    }

    /// The interned `Str(name)` key node for a field name, shared across every
    /// read of it.
    ///
    /// # Invariant
    /// One node per name is enough: a `TableGet` compares content only, and the
    /// frontend already interns the string side.
    pub(super) fn name_node(&mut self, name: &'static str) -> NodeId {
        if let Some(&node) = self.name_nodes.get(name) {
            self.stamp_read_node(node);
            return node;
        }
        let node = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Str(name))),
        );
        self.name_nodes.insert(name, node);
        self.stamp_read_node(node);
        node
    }

    /// Attribute a node the read being built created, when there is one.
    pub(super) fn stamp_read_node(&mut self, node: NodeId) {
        if let Some(site) = self.read_site.clone() {
            self.node_edges.insert(node, site);
        }
    }

    // --- allocation ------------------------------------------------------

    /// The innermost function whose body is being compiled, or [`None`].
    fn current_function(&self) -> Option<FunctionId> {
        self.function_stack.last().map(|&(function, _)| function)
    }

    /// Whether a read's container is the current `@loop` function's state
    /// parameter, or a read rooted at it.
    ///
    /// # Invariant
    /// The loop conversion derives the state tuple's element types from the
    /// recursive call and unifies them after the body, so a state read is
    /// deferred to that conversion; every other undecided container is refused.
    fn is_loop_state(&self, e: ExprId) -> bool {
        let Some(&(function, Some(lambda))) = self.function_stack.last() else {
            return false;
        };
        if !self.module.function_is_looping(function) {
            return false;
        }
        let ExprKind::Function { parameter, .. } = self.ir[lambda].kind else {
            return false;
        };
        self.loop_state_rooted(e, parameter)
    }

    /// The rooted test behind [`Self::is_loop_state`]: the state parameter
    /// itself, or a read whose container is one.
    fn loop_state_rooted(&self, e: ExprId, parameter: ExprId) -> bool {
        if e == parameter {
            return true;
        }
        let container = match self.ir[e].kind {
            ExprKind::Field { container, .. } | ExprKind::NamedField { container, .. } => {
                Some(container)
            }
            ExprKind::Index { array, .. } => Some(array),
            _ => None,
        };
        container.is_some_and(|container| self.loop_state_rooted(container, parameter))
    }

    /// A node owned by the current function — the checker's only node creation
    /// point.
    ///
    /// # Invariant
    /// The node is tagged with [`Checker::current_function`] and registered in
    /// its scope, so the apply clone walk recognizes it as template; a top-level
    /// node is untagged and belongs to no template.
    fn alloc_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.module.add_node(block, operation, value);
        if let Some(function) = self.current_function() {
            self.module.register_in_function(function, node);
        }
        node
    }

    /// A fresh, undecided type cell: an empty node slot, which is undecided's
    /// only in-VM representation.
    pub fn fresh_cell(&mut self) -> NodeId {
        self.alloc_node(self.current_block, None, None)
    }

    /// A plain value node in the current block — how a native operator builds a
    /// custom value without an operation.
    pub fn value_node(&mut self, value: P::Value) -> NodeId {
        self.alloc_node(self.current_block, None, Some(value))
    }

    pub fn array_node(&mut self, block: BlockId, ids: &[NodeId]) -> NodeId {
        self.array_node_with_layout(block, ids, 0)
    }

    /// An array instance's value: shallow flags are per item; slot 0 is the template.
    ///
    /// # Invariant
    /// The flags are the items' own, so a marked position is the item a deep
    /// pass skips: the mask is on the item, not on the node that holds it.
    /// The first slot holds the array's **element template** — the layout, not one of
    /// the user elements — so it carries the template marker: a key comparison reads
    /// that slot's presence and never its value.
    pub(super) fn array_instance_node(
        &mut self,
        block: BlockId,
        ids: &[NodeId],
        mask: &[bool],
    ) -> NodeId {
        let items: Vec<ArrayItem> = ids
            .iter()
            .zip(mask.iter())
            .enumerate()
            .map(|(index, (&node, &shallow))| ArrayItem {
                node: AnyNodeId::Dynamic(node),
                shallow,
                template: index == 0,
            })
            .collect();
        self.array_value_node(block, items, 0)
    }

    /// [`Self::array_node`] with the array's **attribute layout**.
    ///
    /// # Invariant
    /// `layout` is a registered id whose `i`-th canonical index names the attribute
    /// the array's `EXPR_ATTR_BASE + i`-th slot carries; id `0` is the empty layout
    /// ([`Self::array_node`]). See `docs/notes/array-attribute-view.md`.
    pub fn array_node_with_layout(
        &mut self,
        block: BlockId,
        ids: &[NodeId],
        layout: u32,
    ) -> NodeId {
        let items: Vec<ArrayItem> = ids
            .iter()
            .map(|&node| ArrayItem::new(AnyNodeId::Dynamic(node)))
            .collect();
        self.array_value_node(block, items, layout)
    }

    /// The array **value** node over `items`, with the array's attribute
    /// layout.
    fn array_value_node(&mut self, block: BlockId, items: Vec<ArrayItem>, layout: u32) -> NodeId {
        self.alloc_node(
            block,
            None,
            Some(P::Value::from(LowValue::Array(ArrayValue::with_layout(
                self.module.alloc_array(&items, block),
                layout,
            )))),
        )
    }

    /// The registered layout of a schema tail: slot `i` carries the canonical
    /// attribute `tail[i]` names.
    ///
    /// # Invariant
    /// The tail is in canonical order (the frontend emits it so), which is what
    /// [`Module::register_attribute_layout`] requires.
    pub(super) fn layout_for_tail(&mut self, tail: &[P::Attr]) -> u32 {
        let indices: Vec<u32> = tail
            .iter()
            .map(|marker| marker.order_index() as u32)
            .collect();
        self.module.register_attribute_layout(&indices)
    }

    /// The full-width layout: slot `i` carries canonical attribute `i`.
    pub(super) fn full_width_layout(&mut self) -> u32 {
        self.layout_for_tail(&P::Attr::ORDER)
    }

    /// Canonical attribute `canonical` of `node`, through its layout and the default.
    pub(super) fn slot_attribute(&self, node: AnyNodeId, canonical: u32) -> Option<AnyNodeId> {
        let value = self.module.node_value(node)?;
        let value = lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value)?;
        self.module.array_attribute(&value, canonical)
    }

    /// The attribute slot a read states, when it states one.
    ///
    /// # Invariant
    /// The slot is the element's **attribute expr**, not the element: a reader of
    /// the slot compares its value position, so returning the element would put the
    /// term's value where a perspective belongs.
    pub(super) fn stated_element_slot(&self, slot: NodeId) -> Option<NodeId> {
        P::Attr::ORDER.iter().find_map(|marker| {
            let node =
                self.slot_attribute(AnyNodeId::Dynamic(slot), marker.order_index() as u32)?;
            let stated = self
                .module
                .node_value(node)
                .and_then(|value| lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value))
                .is_none_or(|value| value != LowValue::Any);
            if !stated {
                return None;
            }
            node.dynamic()
        })
    }

    /// A layout-carrying masked array: the shallow wrapper.
    ///
    /// # Invariant
    /// The width is [`shape::slot_width`], and the first item carries the mask.

    fn array_node_masked_slot(&mut self, block: BlockId, ids: &[NodeId], mask: &[bool]) -> NodeId {
        let mut items: Vec<ArrayItem> = ids
            .iter()
            .zip(mask.iter())
            .map(|(&node, &shallow)| ArrayItem {
                node: AnyNodeId::Dynamic(node),
                shallow,
                template: false,
            })
            .collect();
        let any = self.any();
        items.extend(std::iter::repeat_n(
            ArrayItem::new(AnyNodeId::Dynamic(any)),
            P::Attr::ORDER.len(),
        ));
        let layout = self.full_width_layout();
        let array = self.module.alloc_array(&items, block);
        let node = self.alloc_node(
            block,
            None,
            Some(P::Value::from(LowValue::Array(ArrayValue::with_layout(
                array, layout,
            )))),
        );
        self.shallow_regions.insert(node);
        node
    }

    /// Whether a node holds a shallow mark's expr: a wrapper this check built,
    /// or a clone of one.
    ///
    /// # Invariant
    /// A clone instantiates the template's wrapper, so the origin chain answers
    /// for it; the set holds the wrappers the check built.
    pub(super) fn is_shallow_region(&self, node: NodeId) -> bool {
        let mut current = Some(node);
        while let Some(node) = current {
            if self.shallow_regions.contains(&node) {
                return true;
            }
            current = self.module.node_origin(node);
        }
        false
    }

    pub fn op_node(
        &mut self,
        block: BlockId,
        operator: P::Operator,
        operand: Option<NodeId>,
    ) -> NodeId {
        self.alloc_node(block, Some(Operation { operator, operand }), None)
    }

    /// An expr node `[value, type]` built around two already-compiled nodes.
    fn expr_of(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        self.slot_expr(self.current_block, value, ty)
    }

    /// A **expr** of the recursive encoding at full attribute width: `[first,
    /// second, wildcards…]`.
    ///
    /// # Invariant
    /// Every expr the checker mints comes from here, so a reader finds its two
    /// meaningful slots at [`crate::shape::EXPR_VALUE_SLOT`]/[`crate::shape::EXPR_TYPE_SLOT`] and
    /// every other slot is the wildcard an unstated attribute is.
    fn slot_expr(&mut self, block: BlockId, first: NodeId, second: NodeId) -> NodeId {
        let any = self.any();
        let mut items = Vec::with_capacity(slot_width::<P>());
        items.push(first);
        items.push(second);
        items.extend(std::iter::repeat_n(any, P::Attr::ORDER.len()));
        let layout = self.full_width_layout();
        self.array_node_with_layout(block, &items, layout)
    }

    /// An expr with one attribute slot at its canonical position — a parameter's,
    /// a signature's or an argument's.
    fn attribute_expr(
        &mut self,
        block: BlockId,
        value: NodeId,
        ty: NodeId,
        marker: &P::Attr,
        slot: NodeId,
    ) -> NodeId {
        let any = self.any();
        let mut items = vec![value, ty];
        for index in 0..P::Attr::ORDER.len() {
            items.push(if index == marker.order_index() {
                slot
            } else {
                any
            });
        }
        let layout = self.full_width_layout();
        self.attribute_nodes.insert(slot, *marker);
        self.array_node_with_layout(block, &items, layout)
    }

    /// Whether an element's value, through its origin chain, has no position for
    /// the attribute slot `index`.
    ///
    /// # Invariant
    /// A clone states no value at check time; the walk follows [`Module::node_origin`]
    /// back to the template that holds the expr.
    fn narrow_element(&self, element: NodeId, index: usize) -> bool {
        let mut current = Some(element);
        while let Some(node) = current {
            if let Some(value) = self.module.node_value(AnyNodeId::Dynamic(node)) {
                let Some(value) = lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value) else {
                    return true;
                };
                // SAFETY: the array is the value of a live node of this module.
                let LowValue::Array(array) = value else {
                    return true;
                };
                return unsafe { array.items() }.len() <= crate::shape::attr_slot(index);
            }
            current = self.module.node_origin(node);
        }
        false
    }

    /// An expr carrying the element slot's attributes at their canonical positions.
    fn element_expr(&mut self, value: NodeId, ty: NodeId, element: NodeId) -> NodeId {
        let mut items = vec![value, ty];
        for index in 0..P::Attr::ORDER.len() {
            // An undecided element's attribute is a lazy read of it.
            let slot = match self
                .slot_attribute(AnyNodeId::Dynamic(element), index as u32)
                .and_then(|node| node.dynamic())
            {
                Some(slot) => slot,
                // A narrow element states no attribute: the slot is the wildcard.
                None if self.narrow_element(element, index) => self.any(),
                None => {
                    let attribute = self.alloc_node(
                        self.current_block,
                        None,
                        Some(P::Value::from(LowValue::USize(crate::shape::attr_slot(
                            index,
                        )))),
                    );
                    let ops = self.array_node(self.current_block, &[element, attribute]);
                    self.op_node(
                        self.current_block,
                        P::Operator::from(LowOperator::Index),
                        Some(ops),
                    )
                }
            };
            items.push(slot);
        }
        let layout = self.full_width_layout();
        self.array_node_with_layout(self.current_block, &items, layout)
    }

    /// The kind expression of a compound type: `[marker, Type]`, where `marker`
    /// is a kind marker (see [`crate::shape`]).
    fn kind_expr(&mut self, block: BlockId, marker: NodeId) -> NodeId {
        self.slot_expr(block, marker, self.type_expr)
    }

    /// The function type expression `[[domain, codomain], [FunctionType, K]]`
    /// — the one construction point for arrows.
    ///
    /// # Invariant
    /// It is deliberately not registered in [`Checker::arrows`]: that set feeds
    /// the type printer, whose members render as `T -> U`, so it holds only
    /// source-level arrows. The caller decides membership.
    pub(super) fn arrow(&mut self, block: BlockId, domain: NodeId, codomain: NodeId) -> NodeId {
        self.arrow_parts(block, domain, codomain).2
    }

    /// [`Self::arrow`] returning all three nodes `(shape, kind, expr)` for a
    /// caller that also needs the halves.
    fn arrow_parts(
        &mut self,
        block: BlockId,
        domain: NodeId,
        codomain: NodeId,
    ) -> (NodeId, NodeId, NodeId) {
        let shape = self.array_node(block, &[domain, codomain]);
        let kind = self.kind_expr(block, self.markers.function_type_marker);
        let expr = self.slot_expr(block, shape, kind);
        (shape, kind, expr)
    }

    /// A struct type's nominal id node: one value per written struct type
    /// occurrence, minted while the graph is built.
    ///
    /// # Invariant
    /// The id **is** the written occurrence, so one occurrence has one id however
    /// far the type flows and two occurrences are two declarations by
    /// construction. It is written into the node rather than computed by an
    /// operator, so a clone copies the identity instead of re-minting it and the
    /// build phase never evaluates. See docs/notes/function-type-merge.md.
    fn fresh_nominal_id(&mut self, occurrence: ExprId) -> NodeId {
        self.value_node(P::Value::type_id_value(occurrence.0 as usize))
    }

    /// The children of a variadic expression kind.
    ///
    /// # Invariant
    /// The non-variadic kinds are named rather than caught by a wildcard: a new
    /// kind storing children as a [`ChildRange`] must join the range arm, or it
    /// would compile here and panic at run time.
    fn range_children(&self, e: ExprId) -> Vec<ExprId> {
        let range = match self.ir[e].kind {
            ExprKind::Tuple(range)
            | ExprKind::TypeSet(range)
            | ExprKind::TypeTable(range)
            | ExprKind::Set(range)
            | ExprKind::Table(range) => range,
            ExprKind::Struct { fields, .. } => fields,
            ExprKind::ArrayInst { elements, .. } => elements,
            ExprKind::NativeCall { args, .. } => args,
            ExprKind::Prefix { operands, .. } => operands,
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Function { .. }
            | ExprKind::Apply { .. }
            | ExprKind::BinOp { .. }
            | ExprKind::Instantiate { .. }
            | ExprKind::Record { .. }
            | ExprKind::Assert { .. }
            | ExprKind::Gc { .. }
            | ExprKind::Unify { .. }
            | ExprKind::Index { .. }
            | ExprKind::Field { .. }
            | ExprKind::NamedField { .. }
            | ExprKind::Find { .. }
            | ExprKind::Annotation { .. }
            | ExprKind::Array { .. }
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => {
                unreachable!("expected a variadic expression kind")
            }
        };
        self.ir.children[range.start as usize..range.end as usize].to_vec()
    }

    /// The value of an expression: element 0 of its expr.
    ///
    /// # Invariant
    /// It is stored for a static node and built lazily as `Index(expr, 0)` for a
    /// call result.
    fn value_of(&mut self, e: ExprId) -> NodeId {
        if let Some(value) = self.state[e].val {
            return value;
        }
        let expr = self.state[e].term.expect("expression must be compiled");
        let operands = self.array_node(self.current_block, &[expr, self.zero()]);
        let index = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        self.state[e].val = Some(index);
        index
    }

    /// The value `node` holds, or `None` when it holds nothing. Read-only.
    pub fn node_value(&self, node: NodeId) -> Option<P::Value> {
        self.module.node_value(AnyNodeId::Dynamic(node))
    }

    // --- the check -------------------------------------------------------

    fn check_expr(&mut self, e: ExprId) -> NodeId {
        // The variant decides the compilation: a `Tuple` is a value, a `TypeTuple` a
        // type; the unifies decide correctness.
        self.check_term(e)
    }

    // The encoding accessors and shape predicates live in [`crate::shape`], the
    // single authority for the expr/type encoding.

    /// The checker's recursion: one frame per nested expression, so a hostile
    /// program may nest past the native stack.
    ///
    /// # Safety
    /// Nothing bounds the expression grammar here — the budget guards bound the
    /// runtime, not the check — so the recursion grows the stack instead:
    /// `#[stacksafe]` is what keeps a deeply nested program from overflowing the
    /// process, as the lowlevel's own recursive entry points do.
    #[stacksafe]
    fn check_term(&mut self, e: ExprId) -> NodeId {
        // The IR is a graph, not a tree: one expression may have several parents.
        // Compile each once, or fresh state duplicates.
        if let Some(expr) = self.state[e].term {
            return expr;
        }
        // A cycle forms only through a block-wide placeholder.
        //
        // Invariant: gate on `block_roots` alone, never on the kind.
        let skeleton = if self.ir.block_roots.contains(&e) {
            let vc = self.fresh_cell();
            let tc = self.fresh_cell();
            let skel = self.array_node(self.current_block, &[vc, tc]);
            self.state[e].term = Some(skel);
            self.state[e].val = Some(vc);
            self.state[e].ty = Some(tc);
            Some((vc, tc))
        } else {
            None
        };
        let expr = match self.ir[e].kind {
            ExprKind::Literal(lit) => {
                // A literal builds its expr through `LiteralExt::build`.
                //
                // Invariant: a type-constant literal builds the universe.
                let built = lit.build(self);
                // A value literal decides its own attributes: each is its extension's
                // `[value, type]` expr, else the wildcard.
                let expr = if lit.decides_attributes() {
                    let mut items = vec![built.value, built.ty];
                    for marker in P::Attr::ORDER {
                        let slot = match self
                            .attribute_extension(&marker)
                            .and_then(|ext| ext.literal_value())
                        {
                            Some(value) => {
                                let value = self.value_node(P::Value::from(value));
                                self.expr(value, self.int_type)
                            }
                            None => self.any(),
                        };
                        items.push(slot);
                    }
                    let layout = self.full_width_layout();
                    self.array_node_with_layout(self.current_block, &items, layout)
                } else {
                    built.expr
                };
                self.state[e].term = Some(expr);
                self.state[e].val = Some(built.value);
                self.state[e].ty = Some(built.ty);
                expr
            }
            ExprKind::Parameter => {
                // The function compiled its parameter expr before the body.
                //
                // Invariant: this use resolves to that expr.
                let binding = self.lookup(e);
                self.state[e].term = Some(binding.term);
                self.state[e].ty = Some(binding.ty);
                binding.term
            }
            ExprKind::Function {
                parameter,
                parameter_type,
                parameter_attribute,
                r#return,
                parent,
                looping,
            } => self.check_lam(
                e,
                parent,
                parameter_type,
                parameter_attribute,
                parameter,
                r#return,
                looping,
            ),
            ExprKind::Apply { function, argument } => self.check_app(e, function, argument),
            ExprKind::BinOp {
                operator,
                left,
                right,
            } => self.check_binop(e, operator, left, right),
            ExprKind::Prefix { operator, operands } => self.check_prefix(e, operator, operands),
            ExprKind::Instantiate {
                template,
                value,
                names,
            } => {
                let arg_names: Vec<Option<&'static str>> =
                    self.ir.struct_names[names.start as usize..names.end as usize].to_vec();
                self.check_instantiate(e, template, value, &arg_names)
            }
            ExprKind::Record { value, names } => {
                let field_names: Vec<Option<&'static str>> =
                    self.ir.struct_names[names.start as usize..names.end as usize].to_vec();
                self.check_record(e, value, &field_names)
            }
            ExprKind::Assert { condition } => self.check_assert(e, condition),
            ExprKind::Index { array, index } => self.check_index(e, array, index),
            ExprKind::Field { container, key } => self.check_field(e, container, key),
            ExprKind::NamedField { container, name } => self.check_named_field(e, container, name),
            ExprKind::Find { container, key } => self.check_table_find(e, container, key),
            ExprKind::Annotation { value, r#type, .. } => self.check_ann(e, value, r#type),
            ExprKind::Tuple(_) => self.check_tuple_term(e),
            ExprKind::Struct { fields, names } => self.check_struct_value(e, fields, names),
            ExprKind::Array { template, length } => self.check_array_template(e, template, length),
            ExprKind::ArrayInst {
                template,
                elements,
                depths,
            } => self.check_array_inst(e, template, elements, depths),
            ExprKind::Set(_) => self.check_set_term(e),
            ExprKind::Table(_) => self.check_table_term(e),
            ExprKind::Unify { actual, expected } => self.check_unify_expr(e, actual, expected),
            ExprKind::ErrorBlock => {
                // A recovered-error region: an opaque leaf whose cells are never
                // unified, so nothing inside reports a type error.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                let expr = self.expr_of(val, ty_cell);
                self.state[e].term = Some(expr);
                self.state[e].val = Some(val);
                self.state[e].ty = Some(ty_cell);
                expr
            }
            ExprKind::Placeholder => {
                // `_` is an inference hole in either position: two fresh cells.
                //
                // Invariant: the kind slot is a cell, not the universe.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                // `_` is undecided in every attribute: each attribute slot is a hole of
                // its own, not the wildcard (spec §1).
                let block = self.current_block;
                let mut items = vec![val, ty_cell];
                for _ in 0..P::Attr::ORDER.len() {
                    let slot_value = self.fresh_cell();
                    let slot_type = self.fresh_cell();
                    items.push(self.slot_expr(block, slot_value, slot_type));
                }
                let layout = self.full_width_layout();
                let expr = self.array_node_with_layout(block, &items, layout);
                self.state[e].term = Some(expr);
                self.state[e].val = Some(val);
                self.state[e].ty = Some(ty_cell);
                expr
            }
            ExprKind::TypeSet(_) => self.check_set_type(e),
            ExprKind::TypeTable(_) => self.check_table_type(e),
            ExprKind::Static { export } => {
                // Imported package export: the package's final `[value, type]` expr.
                let expr = self.module.materialize_leaf(export, self.current_block);
                // The importer needs a *checked* contract: the expr's two items, so a
                // violated one is an honest guard, not a panic.

                // SAFETY: `expr` was materialized into the current block, so its
                // arena is alive.
                let Some(items) =
                    (unsafe { self.module.array_items(expr) }).filter(|items| items.len() >= 2)
                else {
                    let expr = self.refused_expr(e);
                    self.record_guard(expr, expr, self.loc(e, 0), DiagKind::ImportExport, None);
                    return expr;
                };
                let value_node = self.module.as_dynamic(items[0].node, self.current_block);
                let ty_node = self.module.as_dynamic(items[1].node, self.current_block);
                self.state[e].term = Some(expr);
                self.state[e].val = Some(value_node);
                self.state[e].ty = Some(ty_node);
                expr
            }
            ExprKind::NativeCall { op, args } => self.check_native_call(e, op, args),
            ExprKind::Gc { inner } => self.check_gc(e, inner),
        };
        // **The anchor handoff**: the one place a mark's position crosses
        // into the lowlevel as data.
        self.anchor_marks(e);
        // Bind the skeleton's cells to the real value and type, so a
        // reference resolved to it equals the finished expression.
        if let Some((vc, tc)) = skeleton {
            let value = self.value_of(e);
            let ty = self.state[e].ty.expect("a compound kind sets a type");
            self.module.unify(vc, value);
            self.module.unify(tc, ty);
        }
        expr
    }

    /// Pin the marks `e` carries to the node its reader reaches them by.
    ///
    /// # Invariant
    /// The one place a mark's *position* crosses into the lowlevel: the stored fact is text
    /// and nothing else — no slot, no tail, no value. Every mark the frontend recorded is
    /// pinned, because a mark is a primitive and not a claim's property: the claim decides
    /// which marks are *projections*, not which are marks.
    fn anchor_marks(&mut self, e: ExprId) {
        // Copied out: the table read and the anchor node are two borrows.
        let Some(payloads) = self.ir.marks.get(&e).cloned() else {
            return;
        };
        let Some(anchor) = self.anchor_node(e) else {
            return;
        };
        self.module.stamp_marks(anchor, payloads);
    }

    /// The node a reader reaches `e`'s marks by.
    ///
    /// # Invariant
    /// **The node the reader walks, never a synthesized one.** A value expression anchors to
    /// its own value node, or — for the kinds decided only at the apply — to the node that
    /// computes it. A **function** is the one other shape: its value node is re-minted per
    /// closure, so a claim on it anchors to the template's parameter, which every clone's
    /// origin chain reaches.
    fn anchor_node(&mut self, e: ExprId) -> Option<NodeId> {
        if let ExprKind::Function { .. } = self.ir[e].kind
            && let Some(function) = self.function_of.get(&e).copied()
        {
            return Some(self.module.functions[function].parameter);
        }
        self.state[e].val.or(self.state[e].term)
    }

    fn lookup(&self, target: ExprId) -> Binding {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(&target).copied())
            .expect("unresolved parameter (frontend bug)")
    }

    /// The attributes an expr states, one node per canonical slot.
    fn stated_attributes(&self, expr: NodeId) -> Vec<(P::Attr, NodeId)> {
        let Some(LowValue::Array(array)) = self
            .module
            .node_value(AnyNodeId::Dynamic(expr))
            .and_then(|value| lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value))
        else {
            return Vec::new();
        };
        let layout = self.module.attribute_layout(array.layout());
        // SAFETY: `array` is the payload of a value of the module being checked.
        let items = unsafe { array.items() };
        layout
            .iter()
            .enumerate()
            .filter_map(|(offset, canonical)| {
                // The wildcard states nothing: it is the hole an expr carries for an
                // attribute no one named.
                let wildcard = items
                    .get(EXPR_ATTR_BASE + offset)?
                    .node
                    .dynamic()
                    .and_then(|node| self.module.node_value(AnyNodeId::Dynamic(node)))
                    .and_then(|value| lichen_utils::extend::AsEnum::<LowValue>::as_enum(&value));
                if matches!(wildcard, Some(LowValue::Any)) {
                    return None;
                }
                let value = items.get(EXPR_ATTR_BASE + offset)?.node.dynamic()?;
                let attribute = P::Attr::ORDER
                    .iter()
                    .find(|attribute| attribute.order_index() as u32 == *canonical)?;
                Some((*attribute, value))
            })
            .collect()
    }
}

impl<P: HighProgram> Ctx<P> for Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    fn full_width_layout(&mut self) -> u32 {
        Checker::full_width_layout(self)
    }

    fn record_full_width_layout(&mut self, layout: u32) {
        self.module.record_full_width_layout(layout);
    }

    /// A raw value's node: a built-in type marker reuses the installed shared
    /// marker node; anything else allocates one.
    fn value_node(&mut self, value: P::Value) -> NodeId {
        for_each_kind_marker!(define_value_node_dispatch[self, value]);
        self.alloc_node(self.current_block, None, Some(value))
    }

    fn array_node(&mut self, ids: &[NodeId]) -> NodeId {
        Checker::array_node(self, self.current_block, ids)
    }

    fn op_node(&mut self, op: P::Operator, operand: Option<NodeId>) -> NodeId {
        Checker::op_node(self, self.current_block, op, operand)
    }

    fn expr(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        Checker::expr_of(self, value, ty)
    }

    fn kind_expr(&mut self, marker: NodeId) -> NodeId {
        Checker::kind_expr(self, self.current_block, marker)
    }

    fn arrow(&mut self, domain: NodeId, codomain: NodeId) -> NodeId {
        Checker::arrow(self, self.current_block, domain, codomain)
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

    fn float_type(&self) -> NodeId {
        self.float_type
    }

    fn string_type(&self) -> NodeId {
        self.string_type
    }

    fn tuple_template(&self) -> NodeId {
        Checker::tuple_template(self)
    }

    fn array_template(&self) -> NodeId {
        Checker::array_template(self)
    }

    fn struct_template(&self) -> NodeId {
        Checker::struct_template(self)
    }

    // The 9 marker-node accessors — registry-derived.
    for_each_kind_marker!(define_ctx_marker_accessor_impls);

    fn check_unify(&mut self, a: NodeId, b: NodeId, loc: Loc, kind: DiagKind) {
        // The seam's contract is the check itself.
        Checker::check_unify(self, a, b, loc, kind);
    }

    fn check_unify_relaxed(
        &mut self,
        a: NodeId,
        b: NodeId,
        loc: Loc,
        kind: DiagKind,
        is_subtype: &dyn Fn(&dyn Ctx<P>, NodeId, NodeId) -> bool,
    ) {
        Checker::check_unify_relaxed(self, a, b, loc, kind, is_subtype);
    }

    fn node_value(&self, node: NodeId) -> Option<P::Value> {
        Checker::node_value(self, node)
    }
}

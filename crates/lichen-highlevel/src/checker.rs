//! The checker compiles an [`IR`] into a lowlevel [`Module`].
//! See docs/language-spec.md.
//!
//! # Invariant
//! Every expression compiles to the recursive pair `[value, type]` whose type
//! slot is itself such a pair, and every type spine bottoms out at the
//! self-referential universe `K = [Type, ↺]`. A parameter is such a pair, so
//! the apply-time unify is the function-parameter check.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use lichen_lowlevel::{
    AnyNodeId, ArrayItem, BlockId, BudgetExhausted, FunctionId, LowOperator, LowValue, Module,
    NodeId, Operation, Registry,
};
use stacksafe::stacksafe;

use crate::attr::{AttrExtRegistry, AttrSet};
use crate::diagnostic::{AssertSpelling, DiagKind, DiaryEntry};
use crate::ir::{BinOp, ChildRange, ConvOp, ExprId, ExprKind, IR, Loc};
use crate::native::{NativeArg, NativeOps, no_native_ops};
use crate::program::{Ctx, HighProgram, LiteralExt, TypeOperator, ValueType};
use crate::shape::for_each_kind_marker;

mod annotations;
mod asserts;
mod diagnostics;
mod indexing;
mod lambda;
mod native_call;
mod operators;
mod structs;
mod tuples;

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

/// A parameter in scope: its `[value, type]` pair and its type cell.
///
/// # Invariant
/// A use references the pair, so the apply's clone always includes it — the
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
    /// The per-expression checker state — one entry per IR expression, see
    /// [`ExprState`].  Indexed by [`ExprId`].
    state: Vec<ExprState>,
    /// The parameter-attribute slot of each function whose parameter is
    /// annotated `x # n`.
    function_param_attr: HashMap<ExprId, (P::Attr, NodeId)>,
    /// The checker's own per-annotation merged attribute tails.
    ///
    /// # Invariant
    /// The IR stays the frontend's: only this table carries the merged tail.
    merged_tails: HashMap<ExprId, Vec<P::Attr>>,
    /// The checker's own check sequence, attributed for diagnostics.
    ///
    /// # Invariant
    /// `diary` is in [`Self::check_seq`] order, so the output order comes from
    /// [`DiaryEntry::seq`] rather than from the vec's position.
    diary: Vec<DiaryEntry>,
    /// The monotonic counter behind [`DiaryEntry::seq`].
    check_seq: usize,
    /// The arrow nodes built by [`Checker::check_lam`]; the type printer renders
    /// them as `param → body`.
    arrows: HashSet<NodeId>,
    /// The apply edges, keyed by apply op node.
    apply_edges: HashMap<NodeId, ApplyEdge>,
    /// A node a runtime failure references, mapped to its location.
    ///
    /// # Invariant
    /// No span is stored on a node, which is what lets the graph be shared.
    node_edges: HashMap<NodeId, Loc>,
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
    /// A forced callee evaluation already hit the VM's apply/depth guard once.
    ///
    /// # Invariant
    /// Never force again: a refused walk leaves the graph partly evaluated, so
    /// the build's statement pass reports the non-termination.
    force_failed: bool,
    // The installed shared kind-marker nodes, one per marker.
    markers: Markers,
    /// The shared `[int, Type]` type expression every literal's pair carries.
    int_type: NodeId,
    /// The shared `[float, Type]` type expression, paired into every `Float`
    /// literal.
    float_type: NodeId,
    /// The shared `[string, Type]` type expression, paired into every `Str`
    /// literal.
    string_type: NodeId,
    /// The canonical universe `[Type, ↺]`.
    type_expr: NodeId,
    // The two shared index constants.
    //
    // Invariant: allocated before any function exists, so untagged.
    /// The shared `USize(0)` node — element 0 of an expression's pair.
    zero_value: NodeId,
    /// The shared `USize(1)` node — element 1 of an expression's pair.
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
    /// The compiled pair node `[value, type]`.
    pub term: Option<NodeId>,
    /// Element 0 of the pair (the value).
    ///
    /// # Invariant
    /// It is stored for a static node and built lazily as `Index(pair, 0)` for a
    /// call result.
    pub val: Option<NodeId>,
    /// Element 1 of the pair (the type).
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
    /// It is index-aligned with the runtime pair's attribute slots
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
    /// The user-facing assert condition nodes (see [`Checker::user_asserts`]).
    pub user_asserts: HashSet<NodeId>,
    /// How a failed assert of each registered condition should read (see
    /// [`AssertSpelling`]).
    pub assert_spellings: HashMap<NodeId, AssertSpelling>,
    /// The top-level statements the definition pass found non-terminating.
    pub nonterminating: Vec<NonTerminating>,
    pub ok: bool,
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
    pub apply_depth_limit: usize,
    /// Cumulative applications permitted before the guard refuses.
    ///
    /// # Invariant
    /// A converted `@loop` iteration is one application, so this bound covers
    /// loops and expansions alike: raising it is what a large trip count asks
    /// for, since a loop never spends the nesting bound.
    pub apply_total_limit: usize,
}

impl Default for WorkBudget {
    /// The checker's tuned pair.
    ///
    /// # Invariant
    /// The two numbers are matched, so an unmarked recursion reaches the total
    /// bound around the count it would have reached the nesting one.
    fn default() -> Self {
        WorkBudget {
            apply_depth_limit: 500,
            apply_total_limit: 2_000,
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

    /// [`Self::build`] under a caller-supplied [`WorkBudget`].
    pub fn build_with_budget(ir: IR<P::Attr, P::Literal>, work_budget: WorkBudget) -> Build<P> {
        Self::build_with(ir, Module::new(), None, no_native_ops(), work_budget)
    }

    /// Compile an IR whose module is bound to a caller-provided shared registry.
    pub fn build_in(ir: IR<P::Attr, P::Literal>, registry: Arc<RwLock<Registry<P>>>) -> Build<P> {
        let module = Registry::new_module(&registry);
        Self::build_with(ir, module, None, no_native_ops(), WorkBudget::default())
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
        )
    }

    /// The shared body of every entry point above: compile `ir` against
    /// `module`.
    fn build_with(
        ir: IR<P::Attr, P::Literal>,
        mut module: Module<P>,
        attr_ext: Option<AttrExtRegistry<P, P::Attr>>,
        native_ops: NativeOps<P>,
        work_budget: WorkBudget,
    ) -> Build<P> {
        // Invariant: an attribute's canonical index is its position in the set's
        // order; checked once per program.
        debug_assert!(
            crate::attr::order_is_canonical::<P::Attr>(),
            "an attribute's canonical index must be its position in the set's order"
        );
        // The definition pass runs the whole program, so the limits must be set
        // before it starts.
        module.apply_depth_limit = work_budget.apply_depth_limit;
        module.apply_total_limit = work_budget.apply_total_limit;

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
            state: vec![ExprState::default(); n],
            function_param_attr: HashMap::new(),
            merged_tails: HashMap::new(),
            diary: Vec::new(),
            check_seq: 0,
            arrows: HashSet::new(),
            apply_edges: HashMap::new(),
            node_edges: HashMap::new(),
            user_asserts: HashSet::new(),
            assert_spellings: HashMap::new(),
            lambda_value_nodes: Vec::new(),
            nonterminating: Vec::new(),
            force_failed: false,
            markers: Markers::default(),
            int_type: NodeId::default(),
            float_type: NodeId::default(),
            string_type: NodeId::default(),
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
        let root = checker.ir.root;
        // Type-check every user-written top-level statement, then the final root.
        let stmt_roots = checker.ir.stmt_roots.clone();
        for &s in &stmt_roots {
            checker.check_expr(s);
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
        // The definition pass: run the program so the apply-time checks fire.
        if !checker.check_failed() {
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
        if !checker.check_failed() {
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
                        path: Vec::new(),
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
                            path: Vec::new(),
                        },
                        budget: Some(budget),
                    });
                }
            }
        }
        // The assert pass: drain the constraint worklist, requiring `USize(1)` of
        // each deep-evaluated condition.
        if !checker.check_failed() && checker.nonterminating.is_empty() {
            checker.module.check_asserts();
        }
        // A build is not ok when a unification failed or a guard was recorded.
        let ok = !checker.check_failed()
            && checker.module.eval_errors.is_empty()
            && checker.module.assert_errors.is_empty()
            && checker.nonterminating.is_empty();
        let root_val = checker.value_of(root);
        let root_schema_tail = checker.schema_tail(root).to_vec();
        // The definition pass syncs an apply's result cell with the return
        // pair, so an unannotated call takes the return type.
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
            user_asserts: checker.user_asserts,
            assert_spellings: checker.assert_spellings,
            nonterminating: checker.nonterminating,
            ok,
        }
    }

    /// Whether any check already failed: a failed unification **or** a recorded
    /// guard failure.
    ///
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
        // `K = [Type, K]`: point its type slot at itself; the lowlevel deep
        // pass cuts the self-loop it reaches.
        let universe = self.alloc_node(root, None, None);
        let items = [
            ArrayItem::new(AnyNodeId::Dynamic(self.markers.type_marker)),
            ArrayItem::new(AnyNodeId::Dynamic(universe)),
        ];
        self.module.write_node_value(
            universe,
            Some(P::Value::from(LowValue::Array(
                self.module.alloc_array(&items, root),
            ))),
        );
        self.type_expr = universe;
        self.int_type = self.array_node(root, &[self.markers.int_marker, self.type_expr]);
        self.float_type = self.array_node(root, &[self.markers.float_marker, self.type_expr]);
        self.string_type = self.array_node(root, &[self.markers.string_marker, self.type_expr]);
        // The positional-read constants, the subscripts of `Index(pair, 0)` and
        // `Index(pair, 1)`, shared with the markers above.
        self.zero_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(0))));
        self.one_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(1))));
    }

    /// The shared `USize(0)` node — the `Index(pair, 0)` subscript every value
    /// read and pair-slot descent uses.
    pub(super) fn zero(&self) -> NodeId {
        self.zero_value
    }

    /// The shared `USize(1)` node — the `Index(pair, 1)` subscript every type
    /// read uses.
    pub(super) fn one(&self) -> NodeId {
        self.one_value
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
            return node;
        }
        let node = self.alloc_node(
            self.current_block,
            None,
            Some(P::Value::from(LowValue::Str(name))),
        );
        self.name_nodes.insert(name, node);
        node
    }

    // --- allocation ------------------------------------------------------

    /// The innermost function whose body is being compiled, or [`None`].
    fn current_function(&self) -> Option<FunctionId> {
        self.function_stack.last().map(|&(function, _)| function)
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

    /// [`Self::array_node`] with the per-position `~` markers of a shallow
    /// array; an all-`false` set is the unmarked form.
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

    /// The kind expression of a compound type: `[marker, Type]`, where `marker`
    /// is a kind marker (see [`crate::shape`]).
    fn kind_expr(&mut self, block: BlockId, marker: NodeId) -> NodeId {
        self.array_node(block, &[marker, self.type_expr])
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

    /// [`Self::arrow`] returning all three nodes `(shape, kind, pair)` for a
    /// caller that also needs the halves.
    fn arrow_parts(
        &mut self,
        block: BlockId,
        domain: NodeId,
        codomain: NodeId,
    ) -> (NodeId, NodeId, NodeId) {
        let shape = self.array_node(block, &[domain, codomain]);
        let kind = self.kind_expr(block, self.markers.function_type_marker);
        let pair = self.array_node(block, &[shape, kind]);
        (shape, kind, pair)
    }

    /// The struct marker node `[payload, TypeStruct]`, the type slot holding the
    /// `TypeStruct` atom.
    ///
    /// # Invariant
    /// The tag is a fact the graph carries, never a shape a reader guesses; this
    /// is the one construction point, including the deferred instantiation's
    /// pin, whose payload cells all stay open.
    fn struct_marker_node(&mut self, id: NodeId, names: NodeId, names_in_order: NodeId) -> NodeId {
        let payload = self.array_node(self.current_block, &[id, names, names_in_order]);
        let tag = self.type_struct_marker_node();
        self.array_node(self.current_block, &[payload, tag])
    }

    /// The struct-kind requirement a decided-tier read states: `[[payload,
    /// TypeStruct], K]`, with the payload left open.
    ///
    /// # Invariant
    /// It names the tag, never the payload's shape, so it checks `TypeStruct`
    /// rather than guessing that any marker-shaped array is a struct; it is the
    /// struct analogue of the tuple read's `[TypeTuple, K]` unify.
    fn struct_kind_requirement(&mut self) -> NodeId {
        let payload = self.fresh_cell();
        let marker = self.array_node(
            self.current_block,
            &[payload, self.type_struct_marker_node()],
        );
        self.kind_expr(self.current_block, marker)
    }

    /// A struct type's nominal id node: the [`TypeOperator::Fresh`] call one
    /// written struct type occurrence makes.
    ///
    /// # Invariant
    /// The `Fresh` call per written occurrence is what makes one written
    /// occurrence one nominal type. See docs/notes/function-type-merge.md.
    fn fresh_nominal_id(&mut self) -> NodeId {
        self.op_node(
            self.current_block,
            P::Operator::from(TypeOperator::Fresh),
            None,
        )
    }

    /// The struct type's encoding — the one construction point for the layout
    /// [`crate::shape`] documents but never builds.
    ///
    /// # Invariant
    /// The identity is decided once per written occurrence, and deep-evaluating
    /// the marker is what pins it: a clone would otherwise re-run `Fresh` or copy
    /// the name arena into a different table, so one type applied twice would not
    /// unify. The field types stay out of the identity.
    /// See docs/notes/function-type-merge.md.
    fn struct_type_type(
        &mut self,
        id: NodeId,
        field_tys: &[NodeId],
        field_names: &[Option<&'static str>],
    ) -> (NodeId, NodeId, NodeId) {
        let shape = self.array_node(self.current_block, field_tys);
        let names = self.build_struct_names(field_names);
        let names_in_order = self.build_struct_names_in_order(field_names);
        let marker = self.struct_marker_node(id, names, names_in_order);
        self.module.evaluate_node_deep(marker, None);
        let kind = self.kind_expr(self.current_block, marker);
        let wrapper = self.array_node(self.current_block, &[shape, kind]);
        (shape, kind, wrapper)
    }

    /// A lazy structural read down a constant index `path`: the nested `Index`
    /// chain that resolves when `base` binds.
    fn lazy_index_path(&mut self, base: NodeId, path: &[usize]) -> NodeId {
        let mut node = base;
        for &slot in path {
            let index = match slot {
                0 => self.zero(),
                1 => self.one(),
                _ => self.usize_node(slot),
            };
            let ops = self.array_node(self.current_block, &[node, index]);
            node = self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(ops),
            );
        }
        node
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
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::Set(range)
            | ExprKind::ShallowArray { range, .. }
            | ExprKind::Table(range) => range,
            ExprKind::TypeStruct { fields, .. } => fields,
            ExprKind::NativeCall { args, .. } => args,
            ExprKind::Literal(_)
            | ExprKind::Parameter
            | ExprKind::Function { .. }
            | ExprKind::Apply { .. }
            | ExprKind::BinOp { .. }
            | ExprKind::Convert { .. }
            | ExprKind::Instantiate { .. }
            | ExprKind::Record { .. }
            | ExprKind::Assert { .. }
            | ExprKind::Index { .. }
            | ExprKind::RawIndex { .. }
            | ExprKind::Field { .. }
            | ExprKind::NamedField { .. }
            | ExprKind::RawNamedField { .. }
            | ExprKind::Find { .. }
            | ExprKind::Annotation { .. }
            | ExprKind::TypeFunction { .. }
            | ExprKind::TypeArray { .. }
            | ExprKind::Placeholder
            | ExprKind::ErrorBlock
            | ExprKind::Static { .. } => {
                unreachable!("expected a variadic expression kind")
            }
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

    /// The value of an expression: element 0 of its pair.
    ///
    /// # Invariant
    /// It is stored for a static node and built lazily as `Index(pair, 0)` for a
    /// call result.
    fn value_of(&mut self, e: ExprId) -> NodeId {
        if let Some(value) = self.state[e].val {
            return value;
        }
        let pair = self.state[e].term.expect("expression must be compiled");
        let operands = self.array_node(self.current_block, &[pair, self.zero()]);
        let index = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        self.state[e].val = Some(index);
        index
    }

    /// The value currently held by `node`'s equality class, or `None` when the
    /// class is undecided. Read-only.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        self.module.class_value(node)
    }

    // --- the check -------------------------------------------------------

    fn check_expr(&mut self, e: ExprId) -> NodeId {
        // The variant decides the compilation: a `Tuple` is a value, a `TypeTuple` a
        // type; the unifies decide correctness.
        self.check_term(e)
    }

    // The encoding accessors and shape predicates live in [`crate::shape`], the
    // single authority for the pair/type encoding.

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
        if let Some(pair) = self.state[e].term {
            return pair;
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
        let pair = match self.ir[e].kind {
            ExprKind::Literal(lit) => {
                // A literal builds its pair through `LiteralExt::build`.
                //
                // Invariant: a type-constant literal builds the universe.
                let built = lit.build(self);
                self.state[e].term = Some(built.pair);
                self.state[e].val = Some(built.value);
                self.state[e].ty = Some(built.ty);
                built.pair
            }
            ExprKind::Parameter => {
                // The function compiled its parameter pair before the body.
                //
                // Invariant: this use resolves to that pair.
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
            ExprKind::Convert { operator, value } => self.check_convert(e, operator, value),
            ExprKind::Instantiate {
                type_expr,
                value,
                names,
            } => {
                let arg_names: Vec<Option<&'static str>> =
                    self.ir.struct_names[names.start as usize..names.end as usize].to_vec();
                self.check_instantiate(e, type_expr, value, &arg_names)
            }
            ExprKind::Record { value, names } => {
                let field_names: Vec<Option<&'static str>> =
                    self.ir.struct_names[names.start as usize..names.end as usize].to_vec();
                self.check_record(e, value, &field_names)
            }
            ExprKind::Assert { condition } => self.check_assert(e, condition),
            ExprKind::Index { array, index } => self.check_index(e, array, index),
            ExprKind::RawIndex { container, index } => self.check_raw_index(e, container, index),
            ExprKind::Field { container, key } => self.check_field(e, container, key),
            ExprKind::NamedField { container, name } => self.check_named_field(e, container, name),
            ExprKind::RawNamedField { container, name } => {
                self.check_raw_named_field(e, container, name)
            }
            ExprKind::Find { container, key } => self.check_table_find(e, container, key),
            ExprKind::Annotation { value, r#type, .. } => self.check_ann(e, value, r#type),
            ExprKind::TypeFunction {
                parameter,
                r#return,
            } => {
                // Compile both sides in the enclosing scope, ahead of the
                // signature's shell.
                let parameter_ty = self.check_type_element(parameter);
                let return_ty = self.check_type_element(r#return);
                self.check_signature(e, parameter_ty, return_ty)
            }
            ExprKind::Tuple(_) => self.check_tuple_term(e),
            ExprKind::TypeTuple(_) => self.check_tuple_type(e),
            ExprKind::TypeStruct { .. } => self.check_type_struct(e),
            ExprKind::Array(_) => self.check_array_term(e),
            ExprKind::Set(_) => self.check_set_term(e),
            ExprKind::Table(_) => self.check_table_term(e),
            ExprKind::ShallowArray { .. } => self.check_shallow_array_term(e),
            ExprKind::ErrorBlock => {
                // A recovered-error region: an opaque leaf whose cells are never
                // unified, so nothing inside reports a type error.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                let pair = self.pair_of(val, ty_cell);
                self.state[e].term = Some(pair);
                self.state[e].val = Some(val);
                self.state[e].ty = Some(ty_cell);
                pair
            }
            ExprKind::Placeholder => {
                // `_` is an inference hole in either position: two fresh cells.
                //
                // Invariant: the kind slot is a cell, not the universe.
                let val = self.fresh_cell();
                let ty_cell = self.fresh_cell();
                let pair = self.pair_of(val, ty_cell);
                self.state[e].term = Some(pair);
                self.state[e].val = Some(val);
                self.state[e].ty = Some(ty_cell);
                pair
            }
            ExprKind::TypeArray {
                element_type,
                length,
            } => self.check_array_type(e, element_type, length),
            ExprKind::Static { export } => {
                // Imported package export: the package's final `[value, type]` pair.
                let pair = self.module.materialize_leaf(export, self.current_block);
                // The importer needs a *checked* contract: the pair's two items, so a
                // violated one is an honest guard, not a panic.

                // SAFETY: `pair` was materialized into the current block, so its
                // arena is alive.
                let Some(items) =
                    (unsafe { self.module.array_items(pair) }).filter(|items| items.len() == 2)
                else {
                    let pair = self.refused_pair(e);
                    self.record_guard(pair, pair, self.loc(e, 0), DiagKind::ImportExport, None);
                    return pair;
                };
                let value_node = self.module.as_dynamic(items[0].node, self.current_block);
                let ty_node = self.module.as_dynamic(items[1].node, self.current_block);
                self.state[e].term = Some(pair);
                self.state[e].val = Some(value_node);
                self.state[e].ty = Some(ty_node);
                pair
            }
            ExprKind::NativeCall { op, args } => self.check_native_call(e, op, args),
        };
        // Bind the skeleton's cells to the real value and type, so a
        // reference resolved to it equals the finished expression.
        if let Some((vc, tc)) = skeleton {
            let value = self.value_of(e);
            let ty = self.state[e].ty.expect("a compound kind sets a type");
            self.module.unify(vc, value);
            self.module.unify(tc, ty);
        }
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

impl<P: HighProgram> Ctx<P> for Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
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

    fn pair(&mut self, value: NodeId, ty: NodeId) -> NodeId {
        Checker::pair_of(self, value, ty)
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
        Checker::check_unify_relaxed(self, a, b, loc, kind, is_subtype)
    }

    fn class_value(&self, node: NodeId) -> Option<P::Value> {
        Checker::class_value(self, node)
    }
}

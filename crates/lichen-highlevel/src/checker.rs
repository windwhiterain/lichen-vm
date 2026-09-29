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

//!
//! The checking rules are grouped into the sibling modules: `lambda` (a
//! function and an application), `structs` (struct types, field reads,
//! instantiation), `indexing` (arrays, tables, the reads over them),
//! `annotations` (the attribute slots), and `diagnostics` (the attributed
//! unify and the reported-error helpers).  This root holds the checker itself:
//! its state, the node construction every check shares, the per-kind dispatch,
//! and the passes that drive them.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use lichen_lowlevel::{
    AnyNodeId, ArrayItem, BlockId, BudgetExhausted, FunctionId, LowOperator, LowValue, Module,
    NodeId, Operation, Registry,
};

use crate::attr::AttrExt;
use crate::diagnostic::{DiagKind, DiaryEntry};
use crate::ir::{BinOp, ChildRange, ExprId, ExprKind, IR, Loc};
use crate::native::{NativeArg, NativeOps, no_native_ops};
use crate::program::{Ctx, HighProgram, LiteralExt, TypeOperator, ValueType};
use crate::shape::for_each_kind_marker;

mod annotations;
mod diagnostics;
mod indexing;
mod lambda;
mod structs;

// The registry-derived consumer macros below expand the one kind-marker
// list ([`crate::shape::for_each_kind_marker`]) into the checker's marker
// plumbing: the [`Markers`] struct, the `install_constants` allocation, the
// `Ctx` accessor impls, and the `Ctx::value_node` dispatch.  Adding or
// removing a marker touches the registry list alone.

/// The whole [`Markers`] struct, generated from the registry: the field set
/// IS the kind-marker list.
macro_rules! define_markers {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        /// The installed shared kind-marker nodes — one per marker, allocated
        /// by [`Checker::install_constants`] in registry order and referenced
        /// (never rebuilt) wherever the marker value appears.  Registry-derived:
        /// adding or removing a kind marker touches only
        /// [`crate::shape::for_each_kind_marker`]'s list.
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

/// The `install_constants` allocation: one shared node per marker, in
/// registry order.  Call-site context (`self`, the root block) is passed in
/// through the args group — hygiene keeps the macro from seeing it.
macro_rules! define_install_markers {
    ([ $this:ident, $root:ident ] $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $( $this.markers.$marker_fn =
            $this.alloc_node($root, None, Some(ValueType::$marker_fn())); )*
    };
}

/// The `Ctx` marker-accessor impls: each returns the checker's installed
/// shared node for its marker.
macro_rules! define_ctx_marker_accessor_impls {
    ($( [ $($args:tt)* ] )? $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $(
            fn $node_fn(&self) -> NodeId {
                self.markers.$marker_fn
            }
        )*
    };
}

/// The `Ctx::value_node` dispatch: a built-in type marker reuses the
/// checker's installed shared marker node, so the canonical type
/// expressions are reached in place; anything else falls through to a fresh
/// allocation by the caller.
macro_rules! define_value_node_dispatch {
    ([ $this:ident, $value:ident ] $( $(#[$doc:meta])* $variant:ident { $tag:literal, $display:literal, $marker_fn:ident, $node_fn:ident } )*) => {
        $( if $value == ValueType::$marker_fn() {
            return $this.markers.$marker_fn;
        } )*
    };
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
    /// compiling module's plugin (see [`crate::native`]).  Nothing upstream of
    /// the checker can see it — the frontend compiles a `$name` call blindly,
    /// so the **checker** is what resolves the name against this slice, and an
    /// unregistered `$name` is a [`DiagKind::NativeOpUnresolved`] guard
    /// failure reported at the call's own span.  The empty default
    /// `no_native_ops` therefore resolves nothing; a plugin whose source is
    /// being compiled (e.g. `lichen-compute`'s `jit`/`launch`) supplies its own
    /// slice.
    native_ops: NativeOps<P>,
    scopes: Vec<HashMap<ExprId, Binding>>,
    /// The lexical function stack: one entry per enclosing lambda whose body
    /// is being compiled, innermost last — each entry carries the IR
    /// expression id it was compiled from, so a nested closure's
    /// [`ExprKind::Function::parent`] link resolves to a [`FunctionId`] via
    /// [`Self::function_of`].  The innermost entry is the
    /// current function ([`Checker::current_function`]): every node the
    /// checker allocates is tagged with it (its template membership
    /// back-pointer) and registered in its scope
    /// ([`Function::nodes`](lichen_lowlevel::Function::nodes)), and
    /// asserts registered in it join
    /// [`Function::asserts`](lichen_lowlevel::Function::asserts).
    /// [`Checker::check_lam`]
    /// pushes its shell while the body compiles and pops on exit — the old
    /// per-frame block bookkeeping is gone, and the lexical nesting lives in
    /// [`Function::parent`](lichen_lowlevel::Function::parent).  Which
    /// function *is* the parent is decided by the frontend when it compiled
    /// the lambda's syntax (see the language crate's `fn_parents`), not
    /// here: a lambda compiled while another lambda's body is being checked
    /// may be a sibling of it (mutual recursion), which hangs under nothing,
    /// never under the lambda being checked.
    function_stack: Vec<(FunctionId, Option<ExprId>)>,
    /// Every lambda's allocated [`FunctionId`], keyed by its IR expression id
    /// — the `ExprId` → `FunctionId` resolution behind a
    /// [`ExprKind::Function::parent`] link, filled in by
    /// [`Checker::check_lam`] as each function's shell is begun.
    function_of: HashMap<ExprId, FunctionId>,
    /// The compiled pair node `[value, type]` per expression.
    term: Vec<Option<NodeId>>,
    /// Element 0 of the pair (the value).  `None` for call results, whose
    /// value is only known at runtime — built lazily as
    /// `Index(pair, 0)` via [`Checker::value_of`] when the value is needed.
    val: Vec<Option<NodeId>>,
    /// Element 1 of the pair (the type).
    ty: Vec<Option<NodeId>>,
    /// The *constraint* attribute slot (element 2+ of the pair) — the slot the
    /// apply-time attribute check reads.  Indexed like [`Checker::ty`]: `None`
    /// for an expression whose schema carries only a label attribute (`? d`)
    /// or no attribute; the constraint slot for one carrying a constraint
    /// (a `# p` annotation's perspective).  A label slot (e.g. `Doc`) is
    /// metadata and lives only in the pair, never here.
    attr: Vec<Option<NodeId>>,
    /// The parameter-attribute slot of each function whose parameter is
    /// annotated `x # n` — keyed by the `Function` expression id, carrying the
    /// attribute marker (the parameter's schema tail) and the fresh attribute
    /// cell.  The apply uses it (or `missing` for an unannotated parameter) to
    /// run the attribute equality check.
    function_param_attr: HashMap<ExprId, (P::Attr, NodeId)>,
    /// The checker's own per-annotation **merged** attribute tails, keyed by
    /// the annotation expression id.  The IR is the frontend's (its stamped
    /// tail records only what the source spelled), while this tail merges the
    /// value's with the annotation's — a product of checking, so the checker
    /// owns it rather than writing it back into the IR.
    merged_tails: HashMap<ExprId, Vec<P::Attr>>,
    /// The checker's own check sequence, attributed for diagnostics: one entry
    /// per check the checker issued, recording the `unify_errors` entries that
    /// check owns (empty when it produced none) plus its span and check kind.
    ///
    /// Invariant: `diary` is in [`Self::check_seq`] order, which is why the
    /// output order comes from `DiaryEntry::seq` rather than from the vec's
    /// position — the same numbers would then agree.
    diary: Vec<DiaryEntry>,
    /// The monotonic recording counter behind [`DiaryEntry::seq`]: the
    /// checker assigns each recorded check the next value, so two checks are
    /// always comparable in recording order and the diagnostics come out in
    /// that order whatever each check contributed.
    check_seq: usize,
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
    /// The value node of **every** lambda in the program, collected by
    /// [`Checker::check_lam`].  [`Checker::build`] deep-evaluates all of
    /// them before the definition pass (proving them concrete), so a
    /// recursive reference stays in place instead of cloning the function per
    /// application.
    ///
    /// The write is deliberately unconditional: a lambda that does not
    /// reference itself contributes its value node too, and deep-evaluating
    /// it is idempotent, so narrowing the field to the recursive ones would
    /// change nothing about the pass.
    lambda_value_nodes: Vec<NodeId>,
    /// The top-level statements the definition pass found **non-terminating**
    /// (the VM's apply/depth guard refused the walk evaluating the statement's
    /// value).  Each carries a source-blind [`Loc`] so the diagnostics layer
    /// can point at the binding the user wrote, plus the budget the guard
    /// recorded, so the message can name it.  Option B: the build evaluates
    /// every user-written top-level statement and reports a non-terminating
    /// one as an error instead of panicking.
    nonterminating: Vec<NonTerminating>,
    /// A forced callee evaluation ([`Checker::check_instantiate`]) already hit
    /// the VM's apply/depth guard once.  A refused walk leaves the module's
    /// graph partly evaluated, so every later force could trip the guard
    /// again — never force again; the build's statement pass reports the
    /// non-termination.
    force_failed: bool,
    // The installed shared marker nodes — one per kind marker, allocated by
    // `install_constants`; registry-derived (see [`Markers`]).
    markers: Markers,
    /// The shared `[int, Type]` type expression every literal's pair carries.
    int_type: NodeId,
    /// The shared `[string, Type]` type expression every `Str` literal's pair
    /// (and the `string` type constant) carries.
    string_type: NodeId,
    /// The canonical universe `[Type, ↺]` — the self-referential `Type : Type`.
    type_expr: NodeId,
    // The two shared index constants, allocated once by
    // `install_constants` and referenced wherever a pair is read by position.
    //
    // Invariant: they are allocated **before any function exists**, so they
    // carry no function tag and belong to no template.  That is what makes
    // sharing them safe: the apply clone walk's membership test (see
    // `Function::nodes` / `Node::function`) leaves an untagged node
    // referenced in place rather than cloning it, which is exactly the old
    // per-occurrence behaviour observed through one node instead of several.
    // The kind markers already rely on the same property; see the Phase 1
    // notes in `docs/notes/type-system-cleanup-plan.md`.
    /// The shared `USize(0)` node — element 0 of an expression's pair.
    zero_value: NodeId,
    /// The shared `USize(1)` node — element 1 of an expression's pair.
    one_value: NodeId,
    /// The interned field-name nodes — one per unique field name, so every
    /// occurrence of `.a` reads the same key node (see
    /// [`Self::name_node`]).
    name_nodes: HashMap<&'static str, NodeId>,
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

/// One non-terminating statement the definition pass abandoned: where it is,
/// and the budget the VM's guard refused on.
#[derive(Clone, Debug)]
pub struct NonTerminating {
    /// Where the abandoned binding is.
    pub loc: Loc,
    /// The budget that ran out — recorded by the lowlevel instead of
    /// panicking, so the diagnostic can name what was exceeded and by how
    /// much it was bounded.  `None` only if the guard's own record is
    /// somehow absent, which the reporting sites below never allow.
    pub budget: Option<BudgetExhausted>,
}

/// The result of one [`Checker::build`]: the module the lowlevel built, the
/// per-expression nodes the checker attributed, and everything the layers
/// above need to render, trace, or freeze it — plus the pass records that let
/// [`Build::diagnostics`](crate::diagnostic::Build::diagnostics) attribute a
/// lowlevel failure back to the expression that produced it.
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
    /// The root expression's attribute tail — its own tail merged with every
    /// annotation's, as the checker computed it ([`Checker::schema_tail`]).
    /// Index-aligned with the *runtime* pair's attribute slots
    /// (`build.root_term`), which the checker already built at the merged
    /// width, so a renderer walks it rather than the IR's own stamp (which
    /// holds only what the source spelled).
    pub root_schema_tail: Vec<P::Attr>,
    /// The installed shared kind-marker nodes — the complete registry set
    /// (see [`Markers`]).
    pub markers: Markers,
    // The pre-Phase-1 flat spelling of three of the markers, kept so existing
    // readers keep working; `markers` carries the full set.
    pub int_marker: NodeId,
    pub string_marker: NodeId,
    pub type_marker: NodeId,
    /// The shared `[int, Type]` type expression.
    pub int_type: NodeId,
    /// The shared `[string, Type]` type expression.
    pub string_type: NodeId,
    /// The canonical universe `[Type, ↺]`.
    pub type_expr: NodeId,
    /// The checker's attributed checks (see [`DiaryEntry`]) — one per check it
    /// issued, each owning the `unify_errors` range it produced.
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
    /// The top-level statements the definition pass found non-terminating (see
    /// [`Checker::nonterminating`]) — the source-blind locations of the
    /// user-written bindings the build reports as non-termination errors.
    pub nonterminating: Vec<NonTerminating>,
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
        // The attribute set's canonical order is the pair layout; a generated
        // set proves it at build time, and a hand-written one is checked here
        // (once per program) so no two attributes can claim the same slot.
        debug_assert!(
            crate::attr::order_is_canonical::<P::Attr>(),
            "an attribute's canonical index must be its position in the set's order"
        );
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
            function_of: HashMap::new(),
            term: vec![None; n],
            val: vec![None; n],
            ty: vec![None; n],
            attr: vec![None; n],
            function_param_attr: HashMap::new(),
            merged_tails: HashMap::new(),
            diary: Vec::new(),
            check_seq: 0,
            arrows: HashSet::new(),
            apply_edges: HashMap::new(),
            node_edges: HashMap::new(),
            user_asserts: HashSet::new(),
            lambda_value_nodes: Vec::new(),
            nonterminating: Vec::new(),
            force_failed: false,
            markers: Markers::default(),
            int_type: NodeId::default(),
            string_type: NodeId::default(),
            type_expr: NodeId::default(),
            zero_value: NodeId::default(),
            one_value: NodeId::default(),
            name_nodes: HashMap::new(),
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
        checker.module.evaluate_node_deep(checker.string_type, None);
        let root = checker.ir.root;
        // Option B: the top-level statements are the "stack of user-written
        // expressions" — type-check each one (so its term/value/type are
        // built), then the final root.  This replaces the tuple cascade; the
        // build drives the checker over every statement the user wrote.
        let stmt_roots = checker.ir.stmt_roots.clone();
        for &s in &stmt_roots {
            checker.check_expr(s);
        }
        let root_term = checker.check_expr(root);
        let root_ty = checker.ty[root].expect("the root expression must have a type");
        // Prove every lambda's function value concrete before the definition
        // pass: a recursive reference (the apply in the body) then stays in
        // place, so every recursion level re-applies the
        // template — whose own parameter is never deep-evaluated and is
        // cloned fresh per level.  Without this, the deep pass evaluates the
        // parameter clone of the first application, a second application
        // reuses the already-bound clone instead of cloning it fresh, and
        // the argument unify conflicts (the recursion cannot descend).
        for &func_node in &checker.lambda_value_nodes {
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
        if !checker.check_failed() {
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
        // Option B: evaluate every user-written top-level statement, so each
        // one's value is computed — and a non-terminating one, which the VM
        // refused instead of panicking (see `Module::budget_exhausted`), is
        // reported as a diagnostic rather than aborting the build.
        if !checker.check_failed() {
            let mut fatal = false;
            for &s in &stmt_roots {
                let Some(term) = checker.term[s] else {
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
        // The assert pass: drain the module's constraint worklist — the
        // originals and the clones the definition pass's applies produced —
        // force-evaluating each condition (ignoring laziness) and requiring
        // `USize(1)`.  Decided points are consumed; an assert whose condition
        // stays lazy is *not triggered* and stays pending on the worklist:
        // an in-body assert whose parameter was never bound (the function was
        // never applied) is deferred instead of failing, and the clone
        // re-checks it per call.  Skipped when the definition pass was (the
        // graph may be broken enough to panic on the forced evaluation), or
        // when a statement was already found non-terminating (the module state
        // is inconsistent after the caught guard).
        if !checker.check_failed() && checker.nonterminating.is_empty() {
            checker.module.check_asserts();
        }
        // A failed unification or a recorded guard failure is what makes a
        // build not ok — a guard records a diagnostic without touching the
        // lowlevel's error vec, so the vec alone no longer decides `ok`.
        let ok = !checker.check_failed()
            && checker.module.eval_errors.is_empty()
            && checker.module.assert_errors.is_empty()
            && checker.nonterminating.is_empty();
        let root_val = checker.value_of(root);
        let root_schema_tail = checker.schema_tail(root).to_vec();
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
            root_schema_tail,
            markers: checker.markers,
            int_marker: checker.markers.int_marker,
            string_marker: checker.markers.string_marker,
            type_marker: checker.markers.type_marker,
            int_type: checker.int_type,
            string_type: checker.string_type,
            type_expr: checker.type_expr,
            diary: checker.diary,
            arrows: checker.arrows,
            apply_edges: checker.apply_edges,
            node_edges: checker.node_edges,
            user_asserts: checker.user_asserts,
            nonterminating: checker.nonterminating,
            ok,
        }
    }

    /// Whether any check the checker issued already failed — a failed
    /// unification **or** a recorded guard failure.  A guard failure is a
    /// check-time refusal that never reached the lowlevel, so it leaves the
    /// error vec empty and the vec alone would now miss it; this is why the
    /// definition pass skips when it holds (the graph may then hit a
    /// non-function apply, which the runtime panics on).
    fn check_failed(&self) -> bool {
        !self.module.unify_errors.is_empty() || self.diary.iter().any(|e| e.errors.is_empty())
    }

    /// The attribute tail a checked expression carries: an annotation's
    /// **merged** tail (the value's tail merged with the annotation's spelled
    /// slots) when the checker recorded one, else the frontend's own IR stamp.
    /// The fallback is why readers need no knowledge of which expressions the
    /// table covers: every other tail (a parameter's `x # n`, a transplanted
    /// binding's, the frontend's own stamps) lives only in the IR.
    fn schema_tail(&self, e: ExprId) -> &[P::Attr] {
        self.merged_tails
            .get(&e)
            .map_or_else(|| self.ir.schema(e).tail.as_slice(), |tail| tail.as_slice())
    }

    /// The type constants as plain nodes in the root block (types are
    /// first-class values — they live in the runtime graph), plus the two
    /// canonical structures: the universe `K = [Type, ↺]` whose type slot is
    /// itself (`Type : Type` closes every type spine) and the shared int
    /// type expression `[int, K]` every literal's pair carries.
    fn install_constants(&mut self) {
        let root = self.current_block;
        // The 8 kind markers, one shared node each, registry order.
        for_each_kind_marker!(define_install_markers[self, root]);
        // `K = [Type, K]`: allocate the node, then point its type slot at
        // itself.  The self-loop is cut by the lowlevel deep-evaluation
        // cycle guard whenever the definition pass reaches it.
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
        self.string_type = self.array_node(root, &[self.markers.string_marker, self.type_expr]);
        // The two positional-read constants, `Index(pair, 0)` and
        // `Index(pair, 1)`'s subscripts.  Allocated here, before any
        // function exists, so they belong to no template (see the fields'
        // invariant) and are shared like the markers above.
        self.zero_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(0))));
        self.one_value = self.alloc_node(root, None, Some(P::Value::from(LowValue::USize(1))));
    }

    /// The shared `USize(0)` node — the `Index(pair, 0)` subscript every
    /// value read and every pair-slot descent uses.
    pub(super) fn zero(&self) -> NodeId {
        self.zero_value
    }

    /// The shared `USize(1)` node — the `Index(pair, 1)` subscript every
    /// type read uses.
    pub(super) fn one(&self) -> NodeId {
        self.one_value
    }

    /// The interned `Str(name)` key node for a field name — one node per
    /// unique name, shared across every read of it (a `TableGet(names, name)`
    /// only compares content, and the frontend already interns the string
    /// side, so one node per name is enough).  Allocated in the root block
    /// the first time the name is asked for, hence outside every function's
    /// template like the constants above.
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

    /// The innermost function whose body is being compiled — [`None`] at
    /// top level.
    fn current_function(&self) -> Option<FunctionId> {
        self.function_stack.last().map(|&(function, _)| function)
    }

    /// A node owned by the current function — the checker's only node
    /// creation point.  The node is tagged with [`Checker::current_function`]
    /// and registered in its scope
    /// ([`Function::nodes`](lichen_lowlevel::Function::nodes)), so the apply clone
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

    /// The function type expression `[[domain, codomain], [FunctionType,
    /// K]]` — the single construction point for the arrow encoding.  The
    /// block is explicit because the two lambda-site arrows are built into
    /// the shell's own block, not the current one.
    ///
    /// Deliberately **not** registered in [`Checker::arrows`]: that set feeds
    /// the type printer (a member renders as `T -> U`), so it holds only
    /// source-level arrows.  The caller decides membership — see
    /// [`crate::program::Ctx::arrow`].
    pub(super) fn arrow(&mut self, block: BlockId, domain: NodeId, codomain: NodeId) -> NodeId {
        self.arrow_parts(block, domain, codomain).2
    }

    /// [`Self::arrow`] returning all three nodes — `(shape, kind, pair)` —
    /// for a caller that also needs the halves (an arrow *is* a `[shape,
    /// kind]` type expression, so its own value and type are those two).
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

    /// The struct marker node `[TypeId, names]` — the two-field kind marker
    /// of a struct type (layout:
    /// [`shape::STRUCT_MARKER_ID_SLOT`](crate::shape::STRUCT_MARKER_ID_SLOT) /
    /// [`shape::STRUCT_MARKER_NAMES_SLOT`](crate::shape::STRUCT_MARKER_NAMES_SLOT)).
    /// The single construction point both `struct<…>` types and
    /// struct-returning blocks use.
    fn struct_marker_node(&mut self, id: NodeId, names: NodeId) -> NodeId {
        self.array_node(self.current_block, &[id, names])
    }

    /// The struct type's full encoding — the field-type `shape`, the `kind`
    /// `[TypeStruct{id, names}, K]`, and the `[shape, kind]` wrapper pair —
    /// built from the caller's nominal `id` node, the field types and the
    /// field names:
    ///
    /// ```text
    /// wrapper = [ shape, kind ]
    /// shape   = [ field types… ]
    /// kind    = [ TypeStruct{id, names}, K ]
    /// ```
    ///
    /// The single construction point for the layout [`shape`](crate::shape)
    /// documents but deliberately never builds.  The `id` stays the caller's
    /// because its per-occurrence freshness is a policy of the emitting rule,
    /// not part of the encoding.
    fn struct_type_type(
        &mut self,
        id: NodeId,
        field_tys: &[NodeId],
        field_names: &[Option<&'static str>],
    ) -> (NodeId, NodeId, NodeId) {
        let shape = self.array_node(self.current_block, field_tys);
        let names = self.build_struct_names(field_names);
        let marker = self.struct_marker_node(id, names);
        let kind = self.kind_expr(self.current_block, marker);
        let wrapper = self.array_node(self.current_block, &[shape, kind]);
        (shape, kind, wrapper)
    }

    /// A lazy structural read down a constant index `path` from `base`: the
    /// nested `Index` op chain `Index(…Index(base, path[0])…, path[n])` that
    /// resolves when `base` binds — the runtime form of a constant encoding
    /// offset, walked for both struct name-table paths
    /// ([`shape::STRUCT_TYPE_NAMES_PATH`](crate::shape::STRUCT_TYPE_NAMES_PATH),
    /// [`shape::STRUCT_KIND_NAMES_PATH`](crate::shape::STRUCT_KIND_NAMES_PATH)).
    /// Each step's subscript is the shared positional constant.
    fn lazy_index_path(&mut self, base: NodeId, path: &[usize]) -> NodeId {
        // Both name-table paths descend 2-element structures only.
        debug_assert!(path.iter().all(|&slot| slot <= 1));
        let mut node = base;
        for &slot in path {
            let index = if slot == 0 { self.zero() } else { self.one() };
            let ops = self.array_node(self.current_block, &[node, index]);
            node = self.op_node(
                self.current_block,
                P::Operator::from(LowOperator::Index),
                Some(ops),
            );
        }
        node
    }

    /// The children of a variadic expression (`Tuple`, `TypeTuple`,
    /// `Array`, `TypeStruct`, `ShallowArray`).
    fn range_children(&self, e: ExprId) -> Vec<ExprId> {
        let range = match self.ir[e].kind {
            ExprKind::Tuple(range)
            | ExprKind::TypeTuple(range)
            | ExprKind::Array(range)
            | ExprKind::ShallowArray { range, .. }
            | ExprKind::Table(range) => range,
            ExprKind::TypeStruct { fields, .. } => fields,
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

    /// The value of an expression: element 0 of its pair.  For expressions
    /// whose pair is a static node (literals, variables, lambdas) this is
    /// stored; for call results it is extracted at runtime with
    /// `Index(pair, 0)` and memoized.
    fn value_of(&mut self, e: ExprId) -> NodeId {
        if let Some(value) = self.val[e] {
            return value;
        }
        let pair = self.term[e].expect("expression must be compiled");
        let operands = self.array_node(self.current_block, &[pair, self.zero()]);
        let index = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        self.val[e] = Some(index);
        index
    }

    /// `type_of e` — the operand's type expression, read lazily: the term
    /// IS element 1 of the operand's `[value, type]` pair, extracted with
    /// the raw lowlevel `Index` op (the mirror of [`Self::value_of`],
    /// element 0).  The expression's own halves are the type expression's —
    /// the value is its shape (element 0, left to the lazy `value_of` memo
    /// like a parameter use) and the type its kind (element 1) — so
    /// `type_of e` in a type position is exactly the operand's type, and
    /// `e : type_of e` unifies elementwise like `e : T`.  Nothing is forced
    /// here: a read over an unbound parameter resolves at the apply (the
    /// clone rewrites the parameter pair to the argument's).
    fn check_type_of(&mut self, e: ExprId, value: ExprId) -> NodeId {
        self.check_expr(value);
        let operands =
            self.array_node(self.current_block, &[self.term[value].unwrap(), self.one()]);
        let pair = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        let ty_operands = self.array_node(self.current_block, &[pair, self.one()]);
        let ty = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(ty_operands),
        );
        self.term[e] = Some(pair);
        self.val[e] = None;
        self.ty[e] = Some(ty);
        pair
    }

    /// The value currently held by `node`'s equality class — the
    /// representative's value — or `None` when the class is unbound.
    /// Read-only (a parent-pointer walk, no path compression).  An attribute's
    /// [`AttrExt::is_subtype`] uses this to compare two slot values after a
    /// failed equality unify.
    pub fn class_value(&self, node: NodeId) -> Option<P::Value> {
        self.module.class_value(node)
    }

    // --- the check -------------------------------------------------------

    fn check_expr(&mut self, e: ExprId) -> NodeId {
        // The variant decides the compilation (a `Tuple` is a value, a
        // `TypeTuple` a type expression — the frontend picks per syntactic
        // role).  There is no term/type distinction in the checker: every
        // expression compiles to its pair, and correctness is decided by the
        // unifications the surrounding construct issues.
        self.check_term(e)
    }

    // The encoding accessors and shape predicates this checker was built
    // around (`shape_of`/`kind_of`, the universe check, the `is_*` family)
    // live in [`crate::shape`] — the single authority for the pair/type
    // encoding — and are called from here with `self.type_expr` as the
    // canonical universe node.

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
        // the actual type).  Gate the skeleton on `block_roots` membership
        // alone, never on the expression kind: the frontend transplants the
        // binding value's kind into the placeholder, so a block root may be
        // *any* kind — a hand-maintained kind list silently misses a variant
        // (a self-reference through an unlisted kind re-enters this check
        // forever, a stack overflow).  For a childless kind the skeleton is
        // inert — nothing re-enters during its descent — and the epilogue
        // binds it away; a `Function` block root pre-registers its own pair
        // in `check_lam` before its body compiles, overwriting the skeleton.
        let skeleton = if self.ir.block_roots.contains(&e) {
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
                parent,
                ..
            } => self.check_lam(
                e,
                parent,
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
                let parameter_ty = self.check_type_element(parameter);
                let return_ty = self.check_type_element(r#return);
                let (shape, kind, pair) =
                    self.arrow_parts(self.current_block, parameter_ty, return_ty);
                // A source `T -> U` prints as an arrow, so its shape joins
                // `arrows`; a *pattern* arrow (the apply's function-ness
                // guard) deliberately does not — see [`Self::arrow`].
                self.arrows.insert(shape);
                self.term[e] = Some(pair);
                self.val[e] = Some(shape);
                self.ty[e] = Some(kind);
                pair
            }
            ExprKind::Tuple(_) => self.check_tuple_term(e),
            ExprKind::TypeTuple(_) => self.check_tuple_type(e),
            ExprKind::TypeStruct { .. } => self.check_type_struct(e),
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
                // `_` — an inference placeholder hole in any position (type or
                // value): two fresh unbound cells, one for the value slot and
                // one for the kind slot, so whatever the context unifies them
                // with binds them.  The kind slot must be a cell too, not the
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
            ExprKind::TypeOf { value } => self.check_type_of(e, value),
            ExprKind::Static { export } => {
                // Imported package export: the static ref names the package's
                // final `[value, type]` pair.  Materialize that pair leaf,
                // then extract dynamic value/type leaves from its static
                // items; the payloads stay in the package's static arena.
                let pair = self.module.materialize_leaf(export, self.current_block);
                // A `RawIndex` root compiles to the raw read operation, not to
                // a pair: `[1, 2]<0>` exports an unevaluated op node, whose
                // items are unavailable until something evaluates it.  The
                // contract the importer needs is therefore a *checked* one, so
                // a violated contract is an honest guard about the import
                // rather than a panic inside the checker.
                let Some(items) = self.module.array_items(pair) else {
                    let cell = self.fresh_cell();
                    let pair = self.pair_of(cell, cell);
                    self.term[e] = Some(pair);
                    self.val[e] = Some(cell);
                    self.ty[e] = Some(cell);
                    self.record_guard(pair, pair, self.loc(e, 0), DiagKind::ImportExport, None);
                    return pair;
                };
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
    /// a diagnostic rather than a panic (the frontend compiles `$name`
    /// blind, so the checker is the first layer that can see the registry).
    fn check_native_call(&mut self, e: ExprId, op: &'static str, args: ChildRange) -> NodeId {
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
        let built = match ops
            .iter()
            .find(|(name, _)| *name == op)
            .map(|(_, operator)| operator)
        {
            Some(operator) => operator.build(self, e, &native_args, loc),
            None => {
                // An unregistered name is an ordinary check-time refusal, not
                // a broken invariant: only the checker can see the registry,
                // so this is the one place it can be reported.  The guard
                // leaves the expression uncompiled, which `check_failed` picks
                // up — the definition pass is skipped, so nothing ever
                // evaluates the hole.
                self.record_guard(
                    self.type_expr,
                    self.type_expr,
                    loc,
                    DiagKind::NativeOpUnresolved,
                    Some(op),
                );
                let pair = self.pair_of(self.type_expr, self.type_expr);
                self.term[e] = Some(pair);
                self.val[e] = None;
                self.ty[e] = Some(self.type_expr);
                return pair;
            }
        };
        self.term[e] = Some(built.node);
        self.val[e] = built.val;
        self.ty[e] = Some(built.ty);
        built.node
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
        match operator {
            // `==` compares two *same-typed* values and yields 0/1: the Int
            // equalities (`s.a == 1`, `x == y`) and the type-value equalities
            // (`S::a == Int`) — the operands' types must be equal, so a type
            // value (`: Type`) can be compared with a type constant.
            BinOp::Eq => self.check_unify(
                self.ty[left].unwrap(),
                self.ty[right].unwrap(),
                self.loc(left, 1),
                DiagKind::BinOp,
            ),
            BinOp::Add | BinOp::Sub | BinOp::Leq => {
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
            }
        }
        let operator = P::Operator::from(TypeOperator::from(operator));
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
        }
        self.module.add_assert(condition);
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
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
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
        let kind = self.kind_expr(self.current_block, self.markers.tuple_type_marker);
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
    /// The value node for a raw value: a built-in type marker reuses the
    /// checker's installed shared marker node (registry-derived dispatch), so
    /// the canonical type expressions are reached in place; anything else
    /// allocates a node.
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

    fn string_type(&self) -> NodeId {
        self.string_type
    }

    // The 8 marker-node accessors — registry-derived.
    for_each_kind_marker!(define_ctx_marker_accessor_impls);

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

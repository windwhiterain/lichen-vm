use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::RwLock;

use lichen_utils::disjoint::{self};
use lichen_utils::extend::AsEnum;

pub use crate::assert::AssertError;
pub use crate::equality::UnifyError;
pub use crate::evaluation::EvalError;
pub use crate::function::ApplyError;

mod apply;
mod assert;
mod equality;
mod evaluation;
mod function;
mod gc;
mod static_module;
mod table;
mod utils;

pub trait Program: Sized + Copy + Debug + PartialEq {
    /// The program's full value vocabulary: the program's own value union
    /// with the structural [`LowValue`] carried whole as one variant
    /// (composed by [`lichen_utils::enum_ext!`] — see [`LowValue`]).  The
    /// lowlevel reads and builds structural values through
    /// [`AsEnum::as_enum`] and [`From<LowValue>`]; the program's own
    /// variants are opaque to it.
    type Value: ValueExt + From<LowValue> + AsEnum<LowValue> + Clone;
    /// The program's full operator vocabulary: the program's own operator
    /// union with the structural [`LowOperator`] carried whole as one
    /// variant (composed the same way — see [`LowOperator`]).  The
    /// lowlevel dispatches structural operators through [`AsEnum::as_enum`];
    /// everything else falls through to [`OperatorExt::run`].
    type Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>;
    /// Program-global extension state, stored on [`Module`] and read or
    /// mutated by extension operators — the highlevel's fresh-type-id
    /// counter, for example.  A concrete `GlobalExt` is a host struct
    /// composed of component states via [`lichen_utils::compose_ext!`], each
    /// component reached through [`lichen_utils::compose::AsField`] and its
    /// own inherent methods; the lowlevel only requires the marker
    /// [`GlobalExt`] trait.
    type GlobalExt: GlobalExt;
    /// Per-registered-package metadata.  The lowlevel treats this as an
    /// opaque default-constructible slot, just like [`Self::GlobalExt`] is an
    /// opaque marker on modules.  Higher layers extend it with their own
    /// per-package state (for example highlevel package export refs) without
    /// putting that concept into the lowlevel.
    type PackageMeta: Default;
}

/// Program-global extension state — the marker trait that stances the
/// `Program::GlobalExt` bound.
///
/// The lowlevel only ever *initialises* this state ([`Module::new`] calls
/// `P::GlobalExt::default()`); it never copies, compares, or formats it, so a
/// `GlobalExt` needs nothing beyond [`Default`] — `Debug`/`Copy`/`PartialEq`
/// are not required, so the concrete state's components need not be.  The
/// concrete state explicitly implements this marker (opt-in, no blanket impl):
/// it is composed downstream from component states with
/// [`lichen_utils::compose_ext!`] (which generates
/// [`lichen_utils::compose::AsField`] accessors per component; a component's
/// behaviour lives as its own inherent methods), then `impl GlobalExt for ..`.
pub trait GlobalExt: Default {}

/// The structural values the lowlevel itself produces and consumes — the
/// non-extension subset of the former `Value<P>`.  A program's value type
/// composes this enum with [`lichen_utils::enum_ext!`] — `+ LowValue;`
/// carries it whole as one variant named `LowValue` and bakes the
/// `From<LowValue>`/`AsEnum<LowValue>` pair the [`Program::Value`] contract
/// requires — so the lowlevel can always inspect a value through
/// [`AsEnum::as_enum`] and build one through [`From<LowValue>`] without
/// naming the program's part.  A chain layer further up (the highlevel's
/// vocabulary, a language crate's) lists its whole ancestry in one
/// invocation: `+ HighProgramValue as HighProgramValue; + LowValue;` — the
/// root glue generates through the carried layer.
///
/// A structural array value: the element [`ArrayItem`]s behind a
/// [`Handle`] into the array's home block's arena.  An element's `shallow`
/// flag is inert metadata: structure and unification ignore it, but it
/// travels with the element through GC and apply clones, and
/// [`Module::evaluate_node_deep`] skips the subtree of a marked element, so
/// the element stays lazy until a read forces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LowValue {
    USize(usize),
    Array(AnyHandle<[ArrayItem]>),
    /// A constant table value: the entries behind a [`Handle`] into the
    /// table's home block's arena (or a static module's shared arena), each
    /// carrying its key node, value node, and the key's precomputed deep
    /// content hash.  The items are stored sorted by that hash — the
    /// table's "hashtable" — so a read binary-searches and verifies the
    /// equal-hash run with [`Module::key_eq`] (see `table.rs`).  Like an
    /// array, a table is immutable and built once; there is no set/remove.
    Table(AnyHandle<[TableItem]>),
    Function(AnyFunctionId),
    None,
    Parameterized,
}

/// One element of a structural array value: the element's node plus its
/// shallow marker.  `shallow` is inert metadata — structure and unification
/// ignore it — but it travels with the node through GC and apply clones, and
/// [`Module::evaluate_node_deep`] skips the subtree of a marked position, so
/// the element stays lazy until a read forces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrayItem {
    pub node: AnyNodeId,
    pub shallow: bool,
}

impl ArrayItem {
    /// An unmarked element (`shallow` is false).
    pub fn new(node: AnyNodeId) -> Self {
        ArrayItem {
            node,
            shallow: false,
        }
    }
}

/// One entry of a structural table value: the entry's key node, its value
/// node, and the key's precomputed deep-content hash.  `hash` is derived
/// from the key's forced content when the table is built ([`Module::build_table`]),
/// so it is stable for the table's whole life — a stored key is fully
/// concrete by construction.  `key`/`value` are plain node refs: a value is
/// a lazy reference, read (and forced) on demand by `TableGet`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableItem {
    pub key: AnyNodeId,
    pub value: AnyNodeId,
    pub hash: u64,
}

impl AnyHandle<[ArrayItem]> {
    /// The array's items — valid for as long as the handle's home storage is
    /// alive: the array's home block arena for a dynamic payload, or the
    /// static arena (pinned by every importer's [`Dependency`]) for a static
    /// payload — the caller's existing safety contract for arena payloads.
    pub fn items(&self) -> &'static [ArrayItem] {
        match self {
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            AnyHandle::Static(handle) => unsafe { &*handle.offset },
        }
    }
}

impl AnyHandle<[TableItem]> {
    /// The table's entries — same arena-lifetime contract as
    /// [`AnyHandle<[ArrayItem]>::items`].
    pub fn items(&self) -> &'static [TableItem] {
        match self {
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            AnyHandle::Static(handle) => unsafe { &*handle.offset },
        }
    }
}

/// The structural operators the lowlevel itself dispatches — the
/// non-extension subset of the former `Operator<P>`.  A program's operator
/// type composes this enum with [`lichen_utils::enum_ext!`] —
/// `+ LowOperator;` carries it whole as one variant named `LowOperator` and
/// bakes the `From<LowOperator>`/`AsEnum<LowOperator>` pair the
/// [`Program::Operator`] contract requires — so the lowlevel can always pick
/// its own operators out of a value through [`AsEnum::as_enum`]; everything
/// `as_enum` doesn't recognise is a program operator and runs through
/// [`OperatorExt::run`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LowOperator {
    /// - `operand[0]`: array.
    /// - `operand[1]`: index.
    Index,
    /// - `operand[0]`: function.
    /// - `operand[1]`: argument.
    Apply,
    /// - `operand[0]`: table.
    /// - `operand[1]`: key.
    /// A table read: the key is force-evaluated, deep-content-hashed, and
    /// matched against the table's sorted entries; a miss (or a key that is
    /// still unbound) records a [`EvalError`] and yields [`LowValue::None`].
    TableGet,
}

/// The cheap, structural equality a value vocabulary must provide —
/// marker/`USize` variants compare by their fields, handle payloads compare
/// by pointer identity ([`Handle`]'s [`PartialEq`]).  It decides the fast
/// checks (`is_unbound`, kind-marker lookups); the *full* equality
/// unification merges on is [`ValueExt::value_eq`], which compares handle
/// payloads by content.  Equality *through* arrays is not any `==`'s job —
/// unification recurses into them elementwise.
pub trait ValueExt: Debug + Copy + PartialEq {
    fn is_handle(&self) -> bool;
    /// Available if [`Self::is_handle()`].
    fn handle(&self) -> AnyHandle<[u8]> {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn set_handle(&mut self, _payload: AnyHandle<[u8]>) {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn alignment() -> usize {
        unreachable!()
    }
    /// Full equality of two values: handle payloads compare by content
    /// (same variant, byte-wise against the pointed-to allocation), every
    /// other pair is the derived [`PartialEq`].  This is the equality
    /// unification merges two concrete values on — [`PartialEq`] itself is
    /// only the cheap pointer-level form.  Not deep: an array is one
    /// allocation, so two arrays compare equal only when they share it;
    /// structural equality through arrays is unification's elementwise
    /// recursion.
    fn value_eq(&self, other: &Self) -> bool {
        if self.is_handle()
            && other.is_handle()
            && std::mem::discriminant(self) == std::mem::discriminant(other)
        {
            let (a, b) = (self.handle(), other.handle());
            return a.len() == b.len()
                && (std::ptr::eq(a.as_ptr(), b.as_ptr())
                    || unsafe {
                        std::slice::from_raw_parts(a.as_ptr(), a.len())
                            == std::slice::from_raw_parts(b.as_ptr(), b.len())
                    });
        }
        self == other
    }
}

pub trait OperatorExt<P: Program>: Debug + Copy {
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value;
}

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Program> {
    pub operator: P::Operator,
    pub operand: Option<NodeId>,
}

#[derive(Debug, Clone, Copy)]
pub struct StaticOperation<P: Program> {
    pub operator: P::Operator,
    pub operand: Option<LocalNodeId>,
}

/// A class is unbound while it carries no value or only the lazy marker.
/// The highlevel checker uses the same rule for its diagnostics.
pub fn is_unbound(value: Option<impl AsEnum<LowValue>>) -> bool {
    value.is_none_or(|value| value.as_enum() == Some(LowValue::Parameterized))
}

/// Pointer into a [`Block::arena`].
/// `PartialEq` is pointer identity: two handles are equal iff they point at
/// the same allocation (same address and length) — never a dereference.
/// Content equality of two handle payloads is a value-level question,
/// answered by [`Module::value_eq`].
#[derive(Debug)]
pub struct Handle<T: ?Sized>(pub *const T);
#[derive(Debug)]
pub struct StaticHandle<T: ?Sized> {
    /// The payload's home module — [`StaticModule::key`], global and
    /// position-independent, so the handle reads identically from any
    /// importer that plugged the module and identity is shared across them.
    pub module: ModuleKey,
    /// Not really pointing to anything, just offset encoded with possible slice length.
    pub offset: *const T,
}

#[derive(Debug)]
pub enum AnyHandle<T: ?Sized> {
    Dynamic(Handle<T>),
    Static(StaticHandle<T>),
}

impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Clone for StaticHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Clone for AnyHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for Handle<T> {}
impl<T: ?Sized> Copy for StaticHandle<T> {}
impl<T: ?Sized> Copy for AnyHandle<T> {}

impl<T: ?Sized> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl<T: ?Sized> PartialEq for StaticHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.module == other.module && std::ptr::eq(self.offset, other.offset)
    }
}

impl<T: ?Sized> PartialEq for AnyHandle<T> {
    /// Identity: both handles must name the same storage — the same kind,
    /// and (for static payloads) the same module key and offset.  Two
    /// importers of the same module therefore share identity: the payload
    /// is the same bytes of the same shared arena.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AnyHandle::Dynamic(a), AnyHandle::Dynamic(b)) => a == b,
            (AnyHandle::Static(a), AnyHandle::Static(b)) => a == b,
            _ => false,
        }
    }
}

impl Handle<[u8]> {
    pub fn len(&self) -> usize {
        let slice = unsafe { &*self.0 };
        slice.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl StaticHandle<[u8]> {
    pub fn len(&self) -> usize {
        let slice = unsafe { &*self.offset };
        slice.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl AnyHandle<[u8]> {
    pub fn len(&self) -> usize {
        match self {
            AnyHandle::Dynamic(handle) => handle.len(),
            AnyHandle::Static(handle) => handle.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// The payload's data pointer (thin).
    pub fn as_ptr(&self) -> *const u8 {
        match self {
            AnyHandle::Dynamic(handle) => handle.0 as *const u8,
            AnyHandle::Static(handle) => handle.offset as *const u8,
        }
    }
}

new_key_type! {pub struct NodeId;}

/// The device key of a static module — the module's compact index in the
/// device's [`Registry`].  A monotonically increasing index allocated by
/// the device registry (the persistent store that maps keys to artifact
/// content hashes), so the same module has the same key in every process
/// sharing the registry — keys are stable across processes and are
/// reclaimed (reused) when a module is removed from the registry, so the
/// key space stays bounded.  Refs (node, function, handle) carry the key
/// of their home module, so refs are absolute from birth: an importer
/// stores them verbatim and resolves the key through the shared registry —
/// no per-importer retarget, no re-based copies, and the same payload is
/// shared by every importer.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleKey(u64);

impl ModuleKey {
    /// The key's compact index value.
    pub const fn as_raw(self) -> u64 {
        self.0
    }
    /// Build a key from its compact index value — the device registry's
    /// allocation unit.
    pub const fn from_raw(index: u64) -> Self {
        ModuleKey(index)
    }
}

impl std::fmt::Debug for ModuleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleKey({})", self.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticNodeId {
    /// The target module — [`StaticModule::key`].  Refs are absolute from
    /// birth (the key is global), so the same ref reads identically from
    /// the module itself, an importer, or any static payload it was frozen
    /// into.
    pub module: ModuleKey,
    pub index: LocalNodeId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalNodeId {
    pub index: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnyNodeId {
    Dynamic(NodeId),
    Static(StaticNodeId),
}
new_key_type! {pub struct BlockId;}
new_key_type! {pub struct FunctionId;}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticFunctionId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticFunctionRef {
    /// The target module — [`StaticModule::key`], same rule as
    /// [`StaticNodeId::module`].
    pub module: ModuleKey,
    pub index: StaticFunctionId,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnyFunctionId {
    Dynamic(FunctionId),
    Static(StaticFunctionRef),
}

/// Garbage collection unit.
/// # Contract
/// - Only one node can be referenced from parent block
/// - Referencing a node whose block was released is a panic
#[derive(Debug)]
pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
    /// Functions homed in this block, registered like nodes so garbage
    /// collection ([`Module::garbage_collect`]) drops them (and their
    /// scopes) with it.
    pub functions: Vec<FunctionId>,
}

/// # Contract:
/// Only [`Self::nodes`] can reference [`Self::parameter`].
#[derive(Debug, Clone)]
pub struct Function {
    /// The template scope — the nodes owned by this function's body
    /// (including the `r#return` and [`Self::parameter`] entry points),
    /// registered as they are compiled.  The clone pass's membership test
    /// does not consult this list directly: a node belongs to the template
    /// iff its [`Node::function`] chain (through [`Self::parent`]) reaches
    /// the applied function.  The list is the *starting set* of a closure
    /// clone and the garbage-collection root set, so it is iterated, never
    /// queried.
    pub nodes: Vec<NodeId>,
    pub r#return: NodeId,
    pub parameter: NodeId,
    /// The lexical parent — the function in whose body this function is
    /// nested (or [`None`] at top level).  The chain of these links makes
    /// the template membership test: a nested closure's nodes belong to an
    /// enclosing function's template because their owner's chain reaches
    /// it.  The link is *not* a keep-alive edge: garbage collection drops
    /// functions with their home block, never through this field.
    pub parent: Option<FunctionId>,
    /// The body's assert conditions — the function's own entries in
    /// [`Module::asserts`].  An apply clones every condition the deep pass
    /// did not prove concrete and registers the clones, so a body's assert
    /// that could not resolve at normalize re-checks the instantiated
    /// condition against each call's argument; a proven-concrete condition
    /// is per-call invariant (decided at normalize), so it is referenced in
    /// place and not re-registered.  Garbage collection moves the listed
    /// conditions with the function like any other edge.
    pub asserts: Vec<NodeId>,
    /// Owner.
    pub block: BlockId,
}

pub struct StaticFunction {
    pub parameter: LocalNodeId,
    pub r#return: LocalNodeId,
    pub asserts: Vec<LocalNodeId>,
}

/// The outcome of the deep pass ([`Module::evaluate_node_deep`],
/// [`Module::evaluate_node_forced`]) on one node.  The deep pass records,
/// per node, whether it ran at all and, when it ran, whether the subtree it
/// covers is parameterized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvaluatedDeep {
    /// `true` when any node in self's reachable subtree has a
    /// [`LowValue::Parameterized`] — i.e. the deep pass could not prove the
    /// subtree concrete.
    pub parameterized: bool,
}

#[derive(Debug)]
pub struct Node<P: Program> {
    pub value: Option<P::Value>,
    pub operation: Option<Operation<P>>,
    /// The function whose body owns this node — the template membership
    /// back-pointer ([`None`] for top-level and runtime-created nodes whose
    /// owner is not a template).  The apply clone walk tests membership by
    /// walking this chain through [`Function::parent`]; clones carry the
    /// tag of the context that created them.
    pub function: Option<FunctionId>,
    /// Owner.
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Whether the deep pass ([`Module::evaluate_node_deep`],
    /// [`Module::evaluate_node_forced`]) has run on this node, and what it
    /// proved.  [`Some`] means the deep pass ran and
    /// [`EvaluatedDeep::parameterized`] records whether any node in self's
    /// reachable subtree has a [`LowValue::Parameterized`].  [`None`] means
    /// it never ran, so the node's concreteness is unknown.
    pub evaluated_deep: Option<EvaluatedDeep>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::equality_representative`].
    pub equality: disjoint::Meta<NodeId>,
}

pub struct StaticNode<P: Program> {
    pub value: Option<P::Value>,
    pub operation: Option<StaticOperation<P>>,
    pub equality: disjoint::Meta<LocalNodeId>,
    /// The solved concreteness flag, copied from the source's
    /// `evaluated_deep` by `StaticModule::from_module` (`true` when never
    /// deep-passed — conservative).  Not derivable from the root value: an
    /// array whose cached value is the array while an element is unresolved
    /// is parameterized.  The importer's deep pass reads this instead of
    /// descending — a static ref is a decided leaf.
    pub parameterized: bool,
}

pub struct Module<P: Program> {
    /// The device's registry — shared with every module executing in the
    /// process.  All static refs resolve through it; the module itself is
    /// never shared (`Arc<Module>` does not exist).
    pub registry: Arc<RwLock<Registry<P>>>,
    pub nodes: SlotMap<NodeId, Node<P>>,
    pub blocks: SlotMap<BlockId, Block>,
    pub functions: SlotMap<FunctionId, Function>,
    /// Nested-application guard: a run panics when function applications
    /// nest deeper than this (a non-terminating function applying itself
    /// directly, e.g. `f(x) = f(x)`).  Defaults to
    /// [`Self::MAX_APPLY_DEPTH`]; tests lower it to panic fast.
    pub apply_depth_limit: usize,
    /// Total-application guard: a run panics when the *cumulative* number
    /// of function applications exceeds this — the lazy graph flattens most
    /// recursion (an apply returns its result pair and the outer deep pass
    /// descends into it, so nested depth stays 1 even for an infinite loop
    /// behind a lazy branch, and a wide recursion like fib is never deep at
    /// all), so nested depth alone cannot bound the work.  The total count
    /// bounds both.  Defaults to [`Self::MAX_APPLY_TOTAL`]; tests lower it
    /// to panic fast.
    pub apply_total_limit: usize,
    /// Deep-evaluation guard: a run panics when [`Self::evaluate_node_deep`]
    /// nests deeper than this (deep-evaluating an infinitely growing value,
    /// e.g. `f(x) = [x, f(x)]`).  Defaults to [`Self::MAX_DEEP_DEPTH`],
    /// which sits above the legitimately ~200k-deep block chains exercised
    /// by the `#[stacksafe]` tests; tests lower it to panic fast.
    pub evaluate_depth_limit: usize,
    pub unify_errors: Vec<UnifyError<P>>,
    /// Runtime evaluation failures (an out-of-bounds [`LowOperator::Index`]),
    /// recorded instead of panicking — same append-only, never-cleared
    /// contract as [`Self::unify_errors`].
    pub eval_errors: Vec<EvalError>,
    /// The module's assert worklist — the condition nodes registered by
    /// [`Self::add_assert`].  Spawn and every apply clone register here;
    /// [`Self::check_asserts`] drains it, consuming decided entries and
    /// leaving exactly the not-yet-triggered ones.  Garbage collection
    /// prunes the entries of dropped blocks.
    pub asserts: Vec<NodeId>,
    /// Failed asserts: a condition that resolved to a concrete value other
    /// than `USize(1)`.  An assert whose condition stays lazy (an unbound
    /// parameter) is not triggered and records nothing.  Same append-only,
    /// never-cleared contract as [`Self::unify_errors`].
    pub assert_errors: Vec<AssertError<P>>,
    /// Failed apply-time parameter checks: the context of each one (the
    /// declared parameter type, the argument type, and the apply node) so the
    /// highlevel can attribute the matching [`Self::unify_errors`] entries to
    /// the call site instead of the deep conflict leaves.  One entry per
    /// failed `function_apply`; the raw [`UnifyError`] entries it produced
    /// stay in [`Self::unify_errors`] alongside it.
    pub apply_errors: Vec<ApplyError>,
    /// Program-global extension state — see [`Program::GlobalExt`].
    pub global_ext: P::GlobalExt,
    apply_depth: usize,
    apply_total: usize,
    deep_depth: usize,
}

/// One entry of the [`Registry`]: a registered static module plus the home
/// of any future per-key access state (user directive).  Since the registry
/// is shared by every module executing in the process, the entry is shared
/// too — it is per-*key* state, not per-importer.
pub struct Package<P: Program> {
    pub module: Arc<StaticModule<P>>,
    /// Opaque per-package metadata; see [`Program::PackageMeta`].
    pub meta: P::PackageMeta,
    /// The artifact's content hash — the module's stable identity on the
    /// device.  The registry maps device keys to these hashes and back, so
    /// a key reinserted after reclamation is recognized as a different
    /// artifact (a loaded module is never silently shadowed).
    pub hash: [u8; 32],
}

/// A fully-solved module frozen into an immutable, shareable form.  Every
/// node holds its final answer (`Parameterized` for a residual computation —
/// never re-run in place); values carry refs keyed by [`Self::key`] —
/// absolute from birth, so an importer reads and stores them verbatim.
pub struct StaticModule<P: Program> {
    /// The module's global name — allocated once at freeze, carried by
    /// every ref into this module, resolved by each importer through its
    /// own [`Module::dependencies`].
    pub key: ModuleKey,
    pub nodes: Vec<StaticNode<P>>,
    pub functions: Vec<StaticFunction>,
    /// The flattened, hand-laid-out payload arena: array item slices and
    /// ext-value payload bytes, deduped by `(ptr, len)` so aliased handles
    /// keep identity equality.  Filled by the two-phase build in
    /// `StaticModule::from_module`; never mutated afterwards, and shared by
    /// every importer — values are used in place, never copied out.
    pub arena: Vec<u8>,
}

/// The result of freezing a dynamic module into the registry: the allocated
/// device key plus the source→statics node map, so callers can turn
/// dynamic node ids into [`StaticNodeId`]s for exported roots.
pub struct Freeze {
    /// The freshly allocated device key.
    pub key: ModuleKey,
    /// Source `NodeId` → home-module local node index.
    pub node_map: HashMap<NodeId, LocalNodeId>,
}

/// The device's module registry — the virtual file system of the device's
/// compiled modules, shared by every [`Module`] executing (in threads) as
/// `Arc<RwLock<Registry>>`.  It handles **registering** (compiling a
/// dynamic module into a static artifact and filing it under its device
/// key), **storage** (the resident map of loaded modules), and resolution
/// **during evaluating** (every static ref an executing module touches is
/// fetched through [`Self::get`]).
///
/// The resident map is keyed by [`ModuleKey`] — the device key.  The key
/// itself is allocated by the *device registry* (the persistent store
/// living outside the lowlevel — see `crates/lichen-language/src/package.rs`
/// and `persist.rs`), which maps keys to artifact content hashes and back;
/// the lowlevel only files a built artifact under the caller-provided key
/// ([`Self::freeze_mapped`], [`Self::insert_module`]).  Keys are compact
/// indices, stable across processes and reclaimed when a module is removed
/// from the device registry.
pub struct Registry<P: Program> {
    entries: HashMap<ModuleKey, Package<P>>,
}
impl<P: Program> Default for Registry<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Registry<P> {
    pub fn new() -> Self {
        Registry {
            entries: HashMap::new(),
        }
    }

    /// An executing module bound to this registry: every static ref it
    /// touches resolves through `self`.  Modules executing in threads share
    /// the one registry `Arc` — a `Module` itself is never shared
    /// (`Arc<Module>` does not exist; it is a per-thread owned value).
    pub fn new_module(registry: &Arc<RwLock<Registry<P>>>) -> Module<P> {
        Module::with_registry(registry.clone())
    }

    /// Compile a dynamic module into a static artifact and file it under
    /// `key` — the device key allocated by the device registry (the caller
    /// provides it so the artifact's refs are baked with their final key;
    /// the key must not already be registered — same content must not be
    /// compiled twice).  A failed build leaves the registry untouched.
    pub fn freeze(&mut self, module: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(module, key, hash).key
    }

    /// Like [`Self::freeze`], but also returns the source→statics node map
    /// so a caller can construct exported [`StaticNodeId`]s for root nodes.
    ///
    /// The source may itself carry static refs — its frozen dependencies
    /// (a package importing packages).  They are absolute from birth, so the
    /// artifact keeps them verbatim; this method checks the soundness
    /// precondition the verbatim refs imply: every module key the source's
    /// values reference must already be registered *here*, so the frozen
    /// artifact resolves from any importer through this registry.  Freeze
    /// dependencies first.
    pub fn freeze_mapped(
        &mut self,
        module: &Module<P>,
        key: ModuleKey,
        hash: [u8; 32],
    ) -> Freeze {
        for dep in crate::static_module::referenced_keys(module) {
            assert!(
                self.entries.contains_key(&dep),
                "freezing a module that references dependency key {dep:?}, which is not registered here — freeze dependencies first"
            );
        }
        assert!(
            !self.entries.contains_key(&key),
            "freezing a module under device key {key:?}, which is already registered — the same content must not be compiled twice"
        );
        let (static_module, node_map) = StaticModule::from_module_mapped(module, key);
        self.entries.insert(
            key,
            Package {
                module: Arc::new(static_module),
                meta: Default::default(),
                hash,
            },
        );
        Freeze { key, node_map }
    }

    /// File an already-built artifact (a module loaded from the device's
    /// persistent store) under its device `key` — the load-time mirror of
    /// [`Self::freeze_mapped`]: the artifact's refs are already baked with
    /// `key`, nothing is rebuilt or re-keyed.  The key must not already be
    /// registered — a registered key is a loaded module, and re-inserting
    /// it would shadow the resident one; the caller checks first
    /// ([`Self::get`], comparing [`Package::hash`] to recognize a key
    /// reallocated after reclamation).
    pub fn insert_module(&mut self, key: ModuleKey, hash: [u8; 32], module: StaticModule<P>) {
        assert!(
            self.entries.insert(
                key,
                Package {
                    module: Arc::new(module),
                    meta: Default::default(),
                    hash,
                }
            ).is_none(),
            "inserting a module under device key {key:?}, which is already registered — a loaded module is never shadowed"
        );
    }

    /// Set the opaque per-package metadata for an existing registered
    /// package.  Higher layers use this to store export markers, source
    /// paths, or any future package-level state without the lowlevel
    /// knowing what that state means.
    pub fn set_package_meta(&mut self, key: ModuleKey, meta: P::PackageMeta) {
        self.entries
            .get_mut(&key)
            .expect("set_package_meta on an unregistered module key")
            .meta = meta;
    }

    /// The registered module behind a device key — the file-system `get`.
    /// `None` is the "no such key" answer; a static ref naming an
    /// unregistered key is a broken module graph and panics at its
    /// resolution site.
    pub fn get(&self, key: ModuleKey) -> Option<&Package<P>> {
        self.entries.get(&key)
    }

    /// Iterate the registered modules — the device's directory listing.
    /// (The persistent store uses it to collect the arenas a serialized
    /// artifact's payload refs point into.)
    pub fn iter(&self) -> impl Iterator<Item = (ModuleKey, &Package<P>)> {
        self.entries.iter().map(|(&key, package)| (key, package))
    }

    /// Whether the registry holds no registered modules.  (Sources with
    /// static refs — packages importing packages — may only be frozen into
    /// a registry that holds their dependencies; see
    /// [`Self::freeze_mapped`].)
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub const MAX_APPLY_DEPTH: usize = 10_000;
    /// The default total-application budget.  Each application clones its
    /// function's body (~tens of nodes), so this also bounds the module's
    /// growth — an indeterminate recursion stops before it drains memory.
    pub const MAX_APPLY_TOTAL: usize = 100_000;
    pub const MAX_DEEP_DEPTH: usize = 300_000;

    pub fn new() -> Self {
        Self::with_registry(Arc::new(RwLock::new(Registry::new())))
    }

    /// A module bound to the given registry — every static ref it touches
    /// resolves through it.  Modules executing in threads share the one
    /// registry `Arc` ([`Registry::new_module`]); a standalone module owns
    /// a private registry, which is the same thing at one-module scale.
    fn with_registry(registry: Arc<RwLock<Registry<P>>>) -> Self {
        Module {
            registry,
            nodes: SlotMap::with_key(),
            blocks: SlotMap::with_key(),
            functions: SlotMap::with_key(),
            apply_depth_limit: Self::MAX_APPLY_DEPTH,
            apply_total_limit: Self::MAX_APPLY_TOTAL,
            evaluate_depth_limit: Self::MAX_DEEP_DEPTH,
            unify_errors: Vec::new(),
            eval_errors: Vec::new(),
            asserts: Vec::new(),
            assert_errors: Vec::new(),
            apply_errors: Vec::new(),
            global_ext: P::GlobalExt::default(),
            apply_depth: 0,
            apply_total: 0,
            deep_depth: 0,
        }
    }

    /// Compile `source` into a static artifact and register it with this
    /// module's registry under `key` — the device key allocated by the
    /// device registry (the caller provides it so the artifact's refs are
    /// baked with their final key).  Convenience over [`Registry::freeze`].
    /// `source` must be a different module — freezing a module into itself
    /// would deadlock its own registry lock.
    pub fn freeze(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> ModuleKey {
        self.freeze_mapped(source, key, hash).key
    }

    /// Compile `source` into a static artifact and register it with this
    /// module's registry under `key`, returning both the device key and the
    /// source→statics node map.  Convenience over
    /// [`Registry::freeze_mapped`].
    pub fn freeze_mapped(
        &mut self,
        source: &Module<P>,
        key: ModuleKey,
        hash: [u8; 32],
    ) -> Freeze {
        debug_assert!(
            !std::ptr::eq(self, source),
            "freezing a module into itself would deadlock its registry lock"
        );
        self.registry
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .freeze_mapped(source, key, hash)
    }

    /// Resets the per-run evaluation budgets ([`Self::apply_depth`],
    /// [`Self::apply_total`], [`Self::deep_depth`]) so a host can drive the
    /// module in a long-running loop (e.g. one kernel call per GUI frame)
    /// without the cumulative apply count exhausting
    /// [`Self::apply_total_limit`]. The budgets guard *one* run; a host that
    /// resets them per run keeps the guard while shedding lifetime
    /// accumulation. The limits themselves are unchanged.
    pub fn reset_apply_budget(&mut self) {
        self.apply_depth = 0;
        self.apply_total = 0;
        self.deep_depth = 0;
    }

    pub fn add_block(&mut self, parent: Option<BlockId>) -> BlockId {
        let block = self.blocks.insert(Block {
            arena: Bump::new(),
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
            functions: Vec::new(),
        });
        if let Some(parent) = parent {
            self.blocks[parent].children.push(block);
        }
        block
    }

    pub fn add_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.nodes.insert(Node {
            value,
            operation,
            function: None,
            block,
            visiting: false,
            evaluated_deep: None,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        node
    }

    /// Registers `condition` as an assert — an explicit constraint, not a
    /// unification, so an unbound condition is *not* bound to `1`, it stays
    /// untriggered until an apply binds it.  [`Self::check_asserts`]
    /// force-evaluates every registered condition (ignoring laziness) and
    /// requires `USize(1)`, see there.  The registry is a worklist; a
    /// condition owned by a function body ([`Function::asserts`]) is cloned
    /// and re-registered per apply, so a body's assert re-checks against
    /// each call's argument.
    pub fn add_assert(&mut self, condition: NodeId) -> NodeId {
        self.asserts.push(condition);
        condition
    }

    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: impl IntoIterator<Item = NodeId>,
        asserts: impl IntoIterator<Item = NodeId>,
    ) -> NodeId {
        let nodes: Vec<NodeId> = nodes.into_iter().collect();
        let function = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: ret,
            parameter: param,
            parent: None,
            asserts: asserts.into_iter().collect(),
            block,
        });
        // The passed nodes are this function's template: tag each with its
        // owner, so the apply clone walk's chain membership test recognizes
        // them.  The function id must exist before the tags point at it.
        for &node in &nodes {
            self.nodes[node].function = Some(function);
        }
        self.functions[function].nodes = nodes;
        self.blocks[block].functions.push(function);
        // The value node is the function's own too — tagged with it, so an
        // enclosing template (a nested function's parent link) clones it and
        // instantiates a fresh closure per call instead of referencing the
        // template's function value in place.
        let func_node = self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Function(AnyFunctionId::Dynamic(
                function,
            )))),
        );
        self.nodes[func_node].function = Some(function);
        func_node
    }
}

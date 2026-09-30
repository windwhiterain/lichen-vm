use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::PoisonError;
use std::sync::RwLock;

use lichen_utils::disjoint::{self};
use lichen_utils::extend::AsEnum;

pub use crate::assert::{AssertError, PendingAssert};
pub use crate::equality::{UnifyError, UnifyStep};
pub use crate::evaluation::EvalError;
pub use crate::function::ApplyError;
pub(crate) use crate::static_module::StaticModuleCache;

mod ancestors;
mod apply;
mod assert;
pub mod codec;
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

    /// The unification policy hook: what to do when a unification stalls
    /// because one or both classes hold a **pending computation** — a class
    /// with no decided value that carries an unevaluated operation, so
    /// neither side can be compared yet.
    ///
    /// The lowlevel itself stays untyped, so it only merges what is a
    /// *generic graph fact*: a pending computation against an all-unbound
    /// skeleton (it holds nothing to erase), and two pending `Index` reads
    /// (neither has a value to compare).  Every other deferral depends on
    /// what the values **mean** — a read whose type is being unified against
    /// a type value, for instance — and that is the program's decision, made
    /// here.  The default refuses, which is the honest answer for a VM that
    /// does not know what its values stand for.
    ///
    /// The policy is given the module to read (that is how it recognises its
    /// own encodings) and must not retain the borrow, merge classes, or
    /// write values.  `None` defers to the lowlevel's generic rules; the
    /// verdict is only consulted where those rules would otherwise record a
    /// conflict.
    fn defer_pending(module: &mut Module<Self>, sides: &PendingSides) -> Option<Deferral> {
        let _ = (module, sides);
        None
    }
}

/// What a [`Program::defer_pending`] policy decided about a stalled
/// unification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deferral {
    /// Merge the classes after all: the pending computation resolves later
    /// and the merge erases nothing.
    Merge,
    /// Record the conflict now.
    Conflict,
}

/// One side of a stalled unification, as the lowlevel sees it: the class
/// identity plus the graph facts that need no knowledge of the program's
/// values.  A policy reads the module for anything beyond these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSide {
    /// The class's equality-class representative.
    pub representative: NodeId,
    /// The class holds an unevaluated operation — a pending computation.
    pub pending: bool,
    /// That operation is an `Index` that cannot be resolved yet (its target
    /// is not a concrete array).
    pub pending_index_read: bool,
    /// The class is an all-unbound structure: no value, no operation.
    pub skeleton: bool,
    /// The class is a single unbound cell.
    pub pure_cell: bool,
}

/// Both sides of a stalled unification — the whole view a policy gets
/// besides the module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSides {
    pub a: PendingSide,
    pub b: PendingSide,
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
    /// An immutable string literal — the builtin `string` value.  The content
    /// is a `&'static str` (the source-owned literal is leaked once), so the
    /// variant is `Copy` like the other scalars and needs no arena relocation
    /// or GC edge.  There is no mutation, indexing, or concatenation: a string
    /// is an atomic value in this universe, exactly like `USize`.
    Str(&'static str),
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
    /// Computed nothing: the yield of a failed read (an out-of-bounds
    /// index, a table miss — each produced together with a recorded
    /// [`EvalError`], so consumers *propagate* it instead of
    /// re-reporting), the anonymous struct's "no name table" marker, and
    /// the no-operand sentinel the VM hands a nullary extension operator.
    /// A concrete, decided value — distinct from both the [`Self::None`]
    /// unit value and the unbound [`Self::Parameterized`] marker.
    Void,
    Parameterized,
}

/// A host-side, **optional** static shape of a node's eventual value.
///
/// This closes the gap that blocks emitting bytecode directly from the
/// lowlevel: a [`Node`] carries a value (possibly still [`LowValue::Parameterized`])
/// and an operator, but has no compile-time notion of *what shape* the value
/// will take.  A layer above the lowlevel — the checker, or a compute
/// frontend that *has* the type — generates a [`LowShape`] for exactly the
/// nodes a backend will **trace**, and stores it in [`Module::shapes`].  The
/// backend reads the shape and emits code without consulting the type half,
/// without forcing the value, and without waiting for evaluation.
///
/// Shape generation is optional and per-node, by design:
/// - a node with **no** entry in [`Module::shapes`] has no traced shape —
///   it is either *type-check-only* scaffolding the backend never reaches,
///   or it is *materialized before the backend runs* (so the backend sees a
///   concrete leaf, not a traceable computation);
/// - only the nodes that form the traceable value-graph spine are annotated.
///
/// A [`LowShape`] is never a lichen value — it is host metadata, sibling to
/// [`ArrayItem::shallow`] and [`Node::evaluated_deep`] — so "a type is just a
/// value" (`Type : Type`) is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowShape {
    /// A machine scalar (`USize`; the kernel-safe scalar subset) — `i64` in
    /// the wasm backend.
    USize,
    /// A heterogeneous fixed-arity tuple.  A kernel whose domain is a tuple
    /// has shape `Tuple(..)` and arity = `self.len()`.
    Tuple(Vec<LowShape>),
    /// A homogeneous fixed-length array: the element shape and the length.
    Array(Box<LowShape>, usize),
    /// A function: parameter → result.
    Function(Box<LowShape>, Box<LowShape>),
    /// A table: key → value.
    Table(Box<LowShape>, Box<LowShape>),
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
    /// The array's items.
    ///
    /// The `'static` return is the arena's lifetime, not a borrow of `&self`:
    /// the slice outlives one call, which the signature cannot express, so the
    /// obligation moves to the caller.  This method's `# Safety` is the one
    /// written contract for the arena accessors;
    /// [`AnyHandle<[TableItem]>::items`] and [`Module::array_items`] cite it
    /// rather than restating it.
    ///
    /// # Safety
    ///
    /// The payload pointer names bytes in a block's arena (a dynamic payload,
    /// allocated by [`Module::alloc_array`]) or in a registered static
    /// module's arena (a static payload, filed by a freeze and pinned by the
    /// registry entry every reader resolves `module` through).  The home
    /// storage must not be released while the returned slice is read.
    /// [`Module::drop_block`] releasing the block's `Bump` is what invalidates
    /// a dynamic payload's slice; a static module's arena lives as long as the
    /// module stays registered.
    pub unsafe fn items(&self) -> &'static [ArrayItem] {
        match self {
            // SAFETY: the caller upholds this method's `# Safety` — the
            // dynamic payload's home block arena has not been released — so
            // the pointer names a live `[ArrayItem]`.
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            // SAFETY: the caller upholds this method's `# Safety` — the static
            // module `handle.module` names is still registered, pinning the
            // arena the payload lives in.
            AnyHandle::Static(handle) => unsafe { &*handle.offset },
        }
    }
}

impl AnyHandle<[TableItem]> {
    /// The table's entries — same arena-lifetime contract as
    /// [`AnyHandle<[ArrayItem]>::items`], which states it.
    ///
    /// # Safety
    ///
    /// As [`AnyHandle<[ArrayItem]>::items`]: the payload's home storage must
    /// not have been released while the returned slice is read.
    pub unsafe fn items(&self) -> &'static [TableItem] {
        match self {
            // SAFETY: the caller upholds this method's `# Safety` — the
            // dynamic payload's home block arena has not been released — so
            // the pointer names a live `[TableItem]`.
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            // SAFETY: the caller upholds this method's `# Safety` — the static
            // module `handle.module` names is still registered, pinning the
            // arena the payload lives in.
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
    ///
    /// A table read: the key is force-evaluated, deep-content-hashed, and
    /// matched against the table's sorted entries; a miss (no entry for
    /// the key, or a target/key that is still unbound or a computed
    /// nothing) records a [`EvalError`] and yields [`LowValue::Void`].
    TableGet,
}

/// The cheap, structural equality a value vocabulary must provide —
/// marker/`USize` variants compare by their fields, handle payloads compare
/// by pointer identity ([`Handle`]'s [`PartialEq`]).  It decides the fast
/// checks (`is_unbound`, kind-marker lookups); the *full* equality
/// unification merges on is [`ValueExt::value_eq`], which compares handle
/// payloads by content.  Equality *through* arrays is not any `==`'s job —
/// unification recurses into them elementwise.
///
/// # Contract
///
/// The crate reads and relocates an implementor's handle payloads by raw
/// pointer, so every implementor owes three obligations:
///
/// - [`Self::handle`] must stay valid while the value lives, and its payload
///   must be stable — the same address and the same length — for as long as
///   the value is reachable.
/// - [`Self::alignment`] must be a power of two: the freeze layout
///   ([`crate::codec::arena_align`]) and the crate's copy path both derive
///   the arena slot from it.
/// - [`Self::handle`]'s length is a **byte** count, and the crate's copy path
///   copies exactly that many bytes into a slot aligned to
///   [`Self::alignment`].
pub trait ValueExt: Debug + Copy + PartialEq {
    fn is_handle(&self) -> bool;
    /// The value's handle payload as bytes.  Available if
    /// [`Self::is_handle()`].
    ///
    /// **Contract:** the returned handle must stay valid while the value
    /// lives, its payload must be stable for as long as the value is
    /// reachable, and its length is a byte count — exactly the bytes
    /// [`Self::value_eq`] compares and the crate's copy path moves.
    fn handle(&self) -> AnyHandle<[u8]> {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn set_handle(&mut self, _payload: AnyHandle<[u8]>) {
        unreachable!()
    }
    /// The payload alignment ext handle values need.  **Contract: must be a
    /// power of two** — the freeze layout ([`crate::codec::arena_align`]) and
    /// the crate's copy path (`Layout::from_size_align`) both derive from it
    /// and the latter refuses anything else.  Available if
    /// [`Self::is_handle()`]; a vocabulary with no handle payloads has no
    /// alignment need, so the default is 1 — this keeps
    /// [`crate::codec::arena_align`] total for every program.
    fn alignment() -> usize {
        1
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
                    // SAFETY: `a` and `b` are handle payloads of the same
                    // byte length.  Both values are reachable (`value_eq`
                    // reads them through `self`/`other`), so each payload's
                    // home storage is alive — the dynamic case by
                    // `drop_block`'s no-live-reference contract, the static
                    // case by the registry pinning the module — and
                    // `as_ptr()`/`len()` are the pointer and byte count the
                    // payload was built with.
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

    /// Whether a callee node the lowlevel cannot prove a function is a value
    /// this operator vocabulary applies.
    ///
    /// [`LowOperator::Apply`] refuses a callee it can prove is not a function —
    /// a scalar, a string, a table, the unit value — and records
    /// [`EvalError::ApplyTarget`]; everything else stays lazy.  It has to,
    /// because the program's own values are opaque to the lowlevel and a
    /// structural **array** is the same shape for a struct instance and for a
    /// compute kernel's `[native, sig]` pair.  Only the program knows which of
    /// its values are callable, and this is where it answers: a composed
    /// operator union ORs its extension leaves' policies, so the leaf whose
    /// vocabulary can apply the value declares it.
    ///
    /// The default refuses — the honest answer for a program that names none.
    /// The policy is given the callee node and reads the module (that is how
    /// it recognises its own values inside a structural array); it must not
    /// mutate the module.
    fn is_callable(module: &Module<P>, callee: AnyNodeId) -> bool {
        let _ = (module, callee);
        false
    }
}

// The structural operators implement [`OperatorExt`] so a composed program's
// operator union can dispatch *every* leaf uniformly (a composed `run` matches
// and calls `op.run` on each carry variant).  The VM routes the structural
// leaves through [`AsEnum`] *before* `run` is ever reached — the `None` arm
// of the dispatch is the extension fall-through — so a structural `run` is
// genuinely unreachable: a structural operator is never an extension
// computation.
impl<P: Program> OperatorExt<P> for LowOperator {
    fn run(&self, _operand: P::Value, _block: BlockId, _module: &mut Module<P>) -> P::Value {
        unreachable!("structural operators are dispatched by the VM")
    }
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
/// [`LowValue::Void`] (a computed failure) and [`LowValue::None`] (the
/// unit value) are concrete values, never unbound.
pub fn is_unbound(value: Option<impl AsEnum<LowValue>>) -> bool {
    value.is_none_or(|value| value.as_enum() == Some(LowValue::Parameterized))
}

/// Pointer into a [`Block::arena`] — or, after a freeze, into a
/// [`StaticModule`]'s shared arena.
/// `PartialEq` is pointer identity: two handles are equal iff they point at
/// the same allocation (same address and length) — never a dereference.
/// Content equality of two handle payloads is a value-level question,
/// answered by [`Module::value_eq`].
///
/// The pointer is private: a handle is built through [`Handle::from_raw`],
/// whose contract carries the payload's arena-lifetime obligation.
#[derive(Debug)]
pub struct Handle<T: ?Sized>(pub(crate) *const T);
#[derive(Debug)]
pub struct StaticHandle<T: ?Sized> {
    /// The payload's home module — [`StaticModule::key`], global and
    /// position-independent, so the handle reads identically from any
    /// importer that plugged the module and identity is shared across them.
    pub module: ModuleKey,
    /// The payload's address in the home module's static arena.  The codec
    /// resolves an artifact's stored form — a base-relative byte offset plus
    /// an element count (`codec.rs`) — into this address at load, and every
    /// reader dereferences it.
    pub(crate) offset: *const T,
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

impl<T: ?Sized> Handle<T> {
    /// A handle to the payload at `pointer`.
    ///
    /// # Safety
    /// `pointer` must name a payload in a live [`Block::arena`] — the address
    /// [`Module::alloc_array`] or [`Module::alloc_table`] returned, or the one
    /// the crate's copy path relocated it to — and must stay valid, meaning
    /// the payload's home block must not be released, for as long as any
    /// reader may dereference the handle.  [`Module::drop_block`] releasing
    /// the block's `Bump` is what invalidates the pointer.
    pub unsafe fn from_raw(pointer: *const T) -> Self {
        Handle(pointer)
    }

    /// The payload's raw pointer.  Dereferencing it carries the same
    /// arena-lifetime obligation as [`Self::from_raw`].
    pub fn as_ptr(&self) -> *const T {
        self.0
    }
}

impl<T: ?Sized> StaticHandle<T> {
    /// A handle to the payload at `offset` of `module`'s static arena — the
    /// mirror of [`Handle::from_raw`] for a payload a freeze filed into a
    /// registered [`StaticModule`].
    ///
    /// # Safety
    /// `offset` must name a payload inside `module`'s arena — what the freeze
    /// layout writes and the codec's load resolves a stored offset to — and
    /// `module` must stay registered, pinning that arena, for as long as any
    /// reader may dereference the handle.
    pub unsafe fn from_raw(module: ModuleKey, offset: *const T) -> Self {
        StaticHandle { module, offset }
    }

    /// The payload's raw pointer.  Dereferencing it carries the same
    /// arena-lifetime obligation as [`Self::from_raw`].
    pub fn as_ptr(&self) -> *const T {
        self.offset
    }
}

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
    /// The payload's byte length.  Reads the fat pointer's metadata
    /// (`<*const [T]>::len`) without forming a reference, so it is safe.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl StaticHandle<[u8]> {
    /// The payload's byte length.  Reads the fat pointer's metadata
    /// (`<*const [T]>::len`) without forming a reference, so it is safe.
    pub fn len(&self) -> usize {
        self.offset.len()
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

/// The device key naming a compiled module — defined in the `lichen-registry`
/// crate (the type-independent persistence layer) and re-exported here so the
/// lowlevel's registry and serialization can name modules without coupling to
/// the language stack.  Its key-space rules live on the type itself, in
/// `lichen-registry`'s `module_key`.
pub use lichen_registry::ModuleKey;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticNodeId {
    /// The target module — [`StaticModule::key`].  Refs are absolute from
    /// birth (the key is global), so the same ref reads identically from
    /// the module itself, an importer, or any static payload it was frozen
    /// into.
    pub module: ModuleKey,
    pub index: LocalNodeId,
}
/// A node's index within its own static module — the module-local half of
/// [`StaticNodeId`].  It carries `Ord` so a set of these can be grouped by a
/// stable sort (see `apply::regroup_clones`); the order is the plain index
/// order, not an opaque key encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// The template scope — [`StaticFunction::parameter`], [`Self::r#return`],
    /// and every node owned by this function's body, as local indices.  This
    /// is the static mirror of [`Function::nodes`]: a nested static closure
    /// re-homed as a dynamic `Function` uses it as the fresh closure's scope,
    /// so the clone walk can distinguish the closure's own nodes (re-tagged
    /// with the fresh owner) from its captures (kept in the enclosing
    /// template).
    pub nodes: Vec<LocalNodeId>,
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
    /// The node's value — **private**.  Read through [`Module::node_value`]
    /// (the node's own slot) or [`Module::class_value`] (through the class
    /// representative); written only through the controlled
    /// [`Module::write_node_value`] API, which maintains the class-consistency
    /// invariant (a concrete value replicates to the class's unbound
    /// pure-cell members).  External crates must never touch the field
    /// directly.
    value: Option<P::Value>,
    /// The node's optional [`LowShape`] — stored *with* the value, behind the
    /// same private gate.  A layer above the lowlevel (which *has* the type)
    /// sets it via [`Module::set_node_shape`], and a backend reads it via
    /// [`Module::node_shape`].  It is an **analysis result, not a checker
    /// stamp**: the checker cannot know a value's shape at lowering (types are
    /// lazy); the shape comes from the graph after it is resolved, and is
    /// absent for any node the backend will not trace (type-check-only
    /// scaffolding, or a node materialized before the backend runs).
    low_shape: Option<LowShape>,
    /// The node's computation — the operator and its single operand edge, or
    /// `None` for a node that carries a value instead.  **Private**: read
    /// through [`Module::node_operation`], and defined once, either by
    /// [`Module::add_node`] or by the cycle-closing late half
    /// [`Module::close_operation_cycle`].  Replacing it in place would strand
    /// the old operand edge and invalidate every cached value and deep-pass
    /// verdict derived through it.
    operation: Option<Operation<P>>,
    /// The function whose body owns this node — the template membership
    /// back-pointer ([`None`] for top-level and runtime-created nodes whose
    /// owner is not a template).  The apply clone walk tests membership by
    /// walking this chain through [`Function::parent`]; clones carry the
    /// tag of the context that created them.  **Private**: read through
    /// [`Module::node_function`]; a node joins a function's body through
    /// [`Module::register_in_function`], and the clone walks re-stamp the tag
    /// on the nodes they instantiate.
    function: Option<FunctionId>,
    /// Owner — the garbage-collection unit whose lifetime bounds this node.
    /// **Private**: read through [`Module::node_block`]; only
    /// [`Module::garbage_collect`] moves it.
    block: BlockId,
    /// Whether an evaluation attempt is computing this node *right now* —
    /// the cycle mark, owned by a frame and released on every exit including
    /// unwind (see [`Module::retain_node`]).  **Private**: read through
    /// [`Module::node_visiting`], which states what the mark does and does
    /// not mean.
    visiting: bool,
    /// Whether the deep pass ([`Module::evaluate_node_deep`],
    /// [`Module::evaluate_node_forced`]) has run on this node, and what it
    /// proved.  [`Some`] means the deep pass ran and
    /// [`EvaluatedDeep::parameterized`] records whether any node in self's
    /// reachable subtree has a [`LowValue::Parameterized`].  [`None`] means
    /// it never ran, so the node's concreteness is unknown.  **Private**:
    /// read through [`Module::node_evaluated_deep`].
    evaluated_deep: Option<EvaluatedDeep>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::equality_representative`].
    /// **Private**: read through [`Module::node_equality`]; the only writer
    /// is the union-find itself ([`Module::add_equality`],
    /// [`Module::equality_representative`], garbage collection's class
    /// splice).
    equality: disjoint::Meta<NodeId>,
}

pub struct StaticNode<P: Program> {
    pub value: Option<P::Value>,
    /// The optional [`LowShape`] copied from the source dynamic node by
    /// [`StaticModule::from_module`] — a frozen node keeps its shape so an
    /// importer's backend can still derive bytecode.
    pub low_shape: Option<LowShape>,
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

/// Which evaluation budget a [`Module`] exhausted, and what its limit was.
///
/// The budget guards (see [`Module::apply_depth_limit`],
/// [`Module::apply_total_limit`], [`Module::evaluate_depth_limit`]) refuse to
/// continue instead of unwinding, and record this — the fact the old
/// `panic!` message formatted and then threw away.  A host that needs the
/// non-termination decision reads it after the call; the module keeps no
/// other trace of the refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExhausted {
    /// Nested applications ran deeper than [`Module::apply_depth_limit`] — a
    /// function applying itself directly, with no base case.
    ApplyDepth { limit: usize },
    /// The cumulative application count passed
    /// [`Module::apply_total_limit`] — the work bound that catches a
    /// recursion the lazy graph flattens below the nesting guard.
    ApplyTotal { limit: usize },
    /// [`Module::evaluate_node_deep`] nested deeper than
    /// [`Module::evaluate_depth_limit`] — deep-evaluating an infinitely
    /// growing value.
    EvaluateDepth { limit: usize },
}

pub struct Module<P: Program> {
    /// The device's registry — shared with every module bound to it (see the
    /// `Registry` doc for the thread rule).  All static refs resolve through
    /// it; the module itself is never shared (`Arc<Module>` does not exist).
    pub registry: Arc<RwLock<Registry<P>>>,
    /// The node table — the slot allocation that **names** a node and owns
    /// its lifetime.  Node *state* is not read here: every state field of
    /// [`Node`] is private, so the only route to a node's block, operation,
    /// owner, visit mark, deep verdict or equality class is the `node_*`
    /// accessors on [`Module`] (see [`Self::node_block`] and friends).  The
    /// table itself stays public because iteration and the union-find walk
    /// it ([`lichen_utils::disjoint::members`]), and because a released
    /// node's absence is itself a readable fact ([`Self::node_value`]).
    pub nodes: SlotMap<NodeId, Node<P>>,
    pub blocks: SlotMap<BlockId, Block>,
    pub functions: SlotMap<FunctionId, Function>,
    /// Nested-application guard: a run records a
    /// [`BudgetExhausted::ApplyDepth`] when function applications nest
    /// deeper than this (a non-terminating function applying itself
    /// directly, e.g. `f(x) = f(x)`) and stops evaluating.  Defaults to
    /// [`Self::MAX_APPLY_DEPTH`]; tests lower it to trip fast.
    pub apply_depth_limit: usize,
    /// Total-application guard: a run records a
    /// [`BudgetExhausted::ApplyTotal`] when the *cumulative* number of
    /// function applications exceeds this — the lazy graph flattens most
    /// recursion (an apply returns its result pair and the outer deep pass
    /// descends into it, so nested depth stays 1 even for an infinite loop
    /// behind a lazy branch, and a wide recursion like fib is never deep at
    /// all), so nested depth alone cannot bound the work.  The total count
    /// bounds both.  Defaults to [`Self::MAX_APPLY_TOTAL`]; tests lower it
    /// to trip fast.
    pub apply_total_limit: usize,
    /// Deep-evaluation guard: a run records a
    /// [`BudgetExhausted::EvaluateDepth`] when [`Self::evaluate_node_deep`]
    /// nests deeper than this (deep-evaluating an infinitely growing value,
    /// e.g. `f(x) = [x, f(x)]`).  Defaults to [`Self::MAX_DEEP_DEPTH`],
    /// which sits above the legitimately ~200k-deep block chains exercised
    /// by the `#[stacksafe]` tests; tests lower it to trip fast.
    pub evaluate_depth_limit: usize,
    /// Which budget a guard refused on, once one has — see
    /// [`BudgetExhausted`].  Never cleared except by a whole-run reset
    /// ([`Self::reset_apply_budget`]), because it is *the* record of why the
    /// last walk stopped: a second walk must know it ran against an already
    /// abandoned graph rather than discovering the exhaustion again.
    pub budget_exhausted: Option<BudgetExhausted>,
    pub unify_errors: Vec<UnifyError<P>>,
    /// Runtime evaluation failures (an out-of-bounds [`LowOperator::Index`]),
    /// recorded instead of panicking — same append-only, never-cleared
    /// contract as [`Self::unify_errors`].
    pub eval_errors: Vec<EvalError>,
    /// The module's assert worklist — the conditions registered by
    /// [`Self::add_assert`].  Spawn and every apply clone register here;
    /// [`Self::check_asserts`] drains it, consuming decided entries and
    /// leaving exactly the not-yet-triggered ones.  Garbage collection
    /// prunes the entries of dropped blocks.  Each entry keeps the body
    /// condition it came from, so a failure can be attributed without the
    /// module knowing anything about the host's per-assert metadata.
    pub asserts: Vec<PendingAssert>,
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
    /// The apply nodes [`Self::apply_errors`] already holds an entry for — the
    /// dedup's membership test, kept beside the list so recording an apply
    /// error costs one hash probe instead of a scan of every entry recorded
    /// so far.  Both stay append-only and are never cleared, so the two
    /// cannot drift.
    apply_error_nodes: HashSet<NodeId>,
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
/// compiled modules, shared by every [`Module`] bound to one
/// `Arc<RwLock<Registry>>`.  A filed value carries arena handles (raw
/// pointers), so the registry cannot cross a thread: the sharing is within one
/// thread, never across threads.  It handles **registering** (compiling a
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
/// indices, stable across processes; the device registry decides when one is
/// reused (see [`ModuleKey`]).
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
    /// touches resolves through `self`.  Modules in one thread share the one
    /// registry `Arc` (see the `Registry` doc for why it cannot cross a
    /// thread) — a `Module` itself is never shared (`Arc<Module>` does not
    /// exist; it is a per-thread owned value).
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
    pub fn freeze_mapped(&mut self, module: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> Freeze {
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
            !self.entries.contains_key(&key),
            "inserting a module under device key {key:?}, which is already registered — a loaded module is never shadowed"
        );
        self.entries.insert(
            key,
            Package {
                module: Arc::new(module),
                meta: Default::default(),
                hash,
            },
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
    /// resolves through it.  Every module in one thread shares the one
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
            apply_error_nodes: HashSet::new(),
            global_ext: P::GlobalExt::default(),
            apply_depth: 0,
            apply_total: 0,
            deep_depth: 0,
            budget_exhausted: None,
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
    pub fn freeze_mapped(&mut self, source: &Module<P>, key: ModuleKey, hash: [u8; 32]) -> Freeze {
        // Hard in release too: a self-freeze (reachable only through a raw
        // pointer, since the borrows of `self` and `source` exclude it)
        // deadlocks on the registry write lock.
        assert!(
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
    ///
    /// [`Self::budget_exhausted`] resets with them: it is a per-run verdict,
    /// so a host starting a new run must not read the previous run's
    /// refusal.
    pub fn reset_apply_budget(&mut self) {
        self.apply_depth = 0;
        self.apply_total = 0;
        self.deep_depth = 0;
        self.budget_exhausted = None;
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
            low_shape: None,
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

    /// The block `node` is homed in — the garbage-collection unit
    /// [`Self::garbage_collect`] moves the node out of when it is released,
    /// and the arena its compound payloads live in.  A node is homed in
    /// exactly one live block; a released block's nodes are removed with it.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_block(&self, node: NodeId) -> BlockId {
        self.nodes[node].block
    }

    /// The function whose template owns `node`, or [`None`] when the node
    /// belongs to no template (a top-level node, or one created at runtime).
    ///
    /// A reader may rely on the tag naming the function that owns the node:
    /// the apply clone walk's membership test is this tag's chain through
    /// [`Function::parent`] reaching the applied function — *not* a lookup
    /// in [`Function::nodes`] — so a wrongly tagged node is cloned or
    /// referenced as a member of the wrong template.  A node joins a body
    /// through [`Self::register_in_function`], which decides the tag and the
    /// scope list together; the clone walks re-stamp the clones they
    /// instantiate, and a function's own value node is tagged without
    /// joining the scope ([`Self::add_function`]).
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_function(&self, node: NodeId) -> Option<FunctionId> {
        self.nodes[node].function
    }

    /// The operation `node` computes — its operator and single operand edge
    /// — or [`None`] for a node that carries a value instead.
    ///
    /// A reader may rely on the operation being **fixed once**: it comes
    /// from [`Self::add_node`] or, when the operand is only nameable after
    /// the node exists, from [`Self::close_operation_cycle`].  It is never
    /// replaced, so an operand edge read once is the edge that computes the
    /// node.  [`Some`] does not mean "unevaluated": an operation node caches
    /// its result, and [`Self::node_value`] is the current value.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_operation(&self, node: NodeId) -> Option<Operation<P>> {
        self.nodes[node].operation
    }

    /// Whether an evaluation attempt — or a deep-pass descent cut against
    /// it — is computing `node` **at this moment**.
    ///
    /// A reader may rely on the mark being a *liveness* flag, not a "was
    /// visited" flag.  It is taken when a frame starts computing the node and
    /// released when that frame exits, on the cached-answer, lazy-answer and
    /// unwinding-panic paths alike (see the invariant on the module's
    /// evaluation-attempt mark, `retain_node`), so `true` means an active
    /// frame holds the node right now; it never means "already evaluated"
    /// (read [`Self::node_value`]) and never means "known concrete" (read
    /// [`Self::node_evaluated_deep`]).  Because the mark is never sticky,
    /// `true` on a node with no cached value is a genuine cyclic read.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_visiting(&self, node: NodeId) -> bool {
        self.nodes[node].visiting
    }

    /// The deep pass's verdict for `node`, or [`None`] when it never ran
    /// there.
    ///
    /// A reader may rely on [`Some`] meaning the deep pass
    /// ([`Self::evaluate_node_deep`], [`Self::evaluate_node_forced`]) ran on
    /// this node and [`EvaluatedDeep::parameterized`] recording whether any
    /// node in its reachable subtree is [`LowValue::Parameterized`] — i.e.
    /// whether the pass could **not** prove the subtree concrete.  [`None`]
    /// means concreteness is *unknown*, which a reader must treat as
    /// parameterized, never as proven concrete: the apply clone walk and the
    /// operation postlude both do, and a budget refusal as well as a node
    /// reached only as an operand leave [`None`].  The verdict covers the
    /// node's graph at the time it was reached; it is cleared when a late
    /// operation edge is added ([`Self::close_operation_cycle`]).
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_evaluated_deep(&self, node: NodeId) -> Option<EvaluatedDeep> {
        self.nodes[node].evaluated_deep
    }

    /// The disjoint-set metadata of `node`'s equality class, as
    /// [`Self::add_equality`] maintains it.
    ///
    /// A reader may rely on `parent` being the union-find link ([`None`]
    /// meaning `node` is its own root) and on `next`, `tail` and `size`
    /// describing the class's member list — which is meaningful only at the
    /// representative.  A reader that needs the representative must not
    /// follow `parent` by hand: use [`Self::equality_representative`], which
    /// also compresses the path.  The metadata is written **only** by the
    /// union-find (merge classes with [`Self::add_equality`]); writing a
    /// parent link by hand would break the size bound and the member list at
    /// once.
    ///
    /// Panics if `node` is not in [`Self::nodes`].
    pub fn node_equality(&self, node: NodeId) -> disjoint::Meta<NodeId> {
        self.nodes[node].equality
    }

    /// **Close an operation cycle** — define the operation of a node whose
    /// operand is only nameable after the node itself exists.
    ///
    /// [`Self::add_node`] takes a node's operation at allocation, which is
    /// enough for an acyclic graph: the operands are allocated first.  A
    /// cycle is not — a recursive value's operation names a node allocated
    /// after it, up to the self-referential `node := op(node)` — so it needs
    /// this late half of a two-phase construction, which is the only way to
    /// give an existing node an operation.
    ///
    /// Contract:
    /// - `node` must be freshly allocated with **no** operation: an
    ///   operation is defined once, never replaced (replacing it would
    ///   strand the previous operand edge and contradict every cached value
    ///   and deep verdict derived through it).
    /// - `node` must not already hold a concrete value: a decided node is
    ///   never evaluated again, so its operation would never run — the edge
    ///   would be dead.
    /// - The node's deep-pass verdict ([`Self::node_evaluated_deep`]) is
    ///   cleared, because the proof predates this operand edge and no longer
    ///   describes the node's graph.  On a freshly allocated node the
    ///   verdict is already [`None`], so the clearing is a no-op there; it
    ///   is what makes the contract hold for any other node.
    pub fn close_operation_cycle(&mut self, node: NodeId, operation: Operation<P>) {
        debug_assert!(
            self.nodes[node].operation.is_none(),
            "a node's operation is defined once: {node:?} already computes one"
        );
        debug_assert!(
            is_unbound(self.nodes[node].value),
            "an operation must not be defined on a node that already holds a concrete value: {node:?}"
        );
        self.nodes[node].operation = Some(operation);
        self.nodes[node].evaluated_deep = None;
    }

    /// Register `node` in `function`'s body scope: tag the node as owned by
    /// `function` ([`Self::node_function`]) **and** append it to
    /// [`Function::nodes`].  Both halves are written together because they
    /// are read for different purposes and must agree: the apply clone walk
    /// follows the owner tag's chain, while garbage collection and a nested
    /// closure's clone walk start from the scope list.
    ///
    /// Contract: `node` must have just been allocated for that function's
    /// body.  The function's own value node is deliberately *not* registered
    /// here — it is reached through the template's return subtree, so
    /// [`Self::add_function`] tags it without listing it.
    pub fn register_in_function(&mut self, function: FunctionId, node: NodeId) {
        debug_assert!(
            self.functions.contains_key(function),
            "a node cannot be owned by a function that does not exist: {function:?}"
        );
        self.nodes[node].function = Some(function);
        self.functions[function].nodes.push(node);
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
        self.asserts.push(PendingAssert {
            condition,
            template: AnyNodeId::Dynamic(condition),
        });
        condition
    }

    /// Create a function template's **shell**, before any of its nodes exist.
    ///
    /// The record this inserts is deliberately incomplete: its
    /// [`Function::parameter`] and [`Function::r#return`] are unset until
    /// [`Self::finish_function`] names them, and nothing may read a function
    /// in between.  Building the shell first is what lets a compiler emit the
    /// parameter nodes *into this function's scope*: the node allocator tags
    /// and registers each node against the function currently being built, so
    /// a parameter allocated after the shell lands in the right template
    /// without being moved there afterwards.
    ///
    /// `parent` is the enclosing template, or `None` at the top level and for
    /// a same-depth sibling (see [`Function::parent`]).
    pub fn begin_function(&mut self, block: BlockId, parent: Option<FunctionId>) -> FunctionId {
        let function = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: NodeId::default(),
            parameter: NodeId::default(),
            parent,
            asserts: Vec::new(),
            block,
        });
        self.blocks[block].functions.push(function);
        function
    }

    /// Complete the shell begun by [`Self::begin_function`], naming the
    /// return and parameter slots.
    ///
    /// Both must already be registered in the function's scope: the apply
    /// clone walk reads its members through [`Function::nodes`], and
    /// `parameter` in particular is what it instantiates, so a parameter
    /// missing from the scope is a construction error, not a runtime one.
    pub fn finish_function(&mut self, function: FunctionId, r#return: NodeId, parameter: NodeId) {
        debug_assert!(
            self.functions[function].nodes.contains(&parameter),
            "a function's parameter slot must be registered in its own scope before the shell is finished"
        );
        self.functions[function].r#return = r#return;
        self.functions[function].parameter = parameter;
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
        let function = self.begin_function(block, None);
        // The passed nodes are this function's template body: register each
        // with its owner, so the apply clone walk's chain membership test
        // recognizes them and garbage collection keeps them with the
        // function.  The function id must exist before the tags point at it.
        for &node in &nodes {
            self.register_in_function(function, node);
        }
        self.functions[function].asserts = asserts.into_iter().collect();
        self.finish_function(function, ret, param);
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

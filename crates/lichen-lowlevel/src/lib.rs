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

pub mod ancestors;
mod apply;
mod assert;
pub mod codec;
mod equality;
mod evaluation;
mod function;
mod gc;
mod low_type;
mod module;
mod registry;
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
    /// and the merge erases nothing.  The lowlevel commits the other side's
    /// decided value onto the merged class (its pending operations keep
    /// their operand edge, so the apply's clone machinery still recomputes
    /// against the real argument — the deferred check surfacing there).
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
    /// That operation is an `Apply` — a call that stays lazy because its
    /// argument is not decided yet (a type-level computation spelled as a
    /// type-function call rather than a read).
    pub pending_apply: bool,
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
#[derive(Debug, Clone, Copy)]
pub enum LowValue {
    USize(usize),
    /// A machine float — the lowlevel's only non-integer number.  `f32` is one
    /// packed buffer component's width, so a buffer handoff is a copy rather
    /// than a conversion.  It is a scalar like `USize`: no handle, no arena
    /// payload, no GC edge, and no conversion to or from `USize`
    /// (`docs/notes/floating-point.md` §4.1, §4.2).
    Float(f32),
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

/// Value identity as the lowlevel decides it: field-wise like the derive, with
/// one variant deliberately different — a float compares by its 32 bits, never
/// by IEEE `==`.
///
/// # The invariant: a float compares by its 32 bits
///
/// Every other variant compares field-wise, exactly as the derive would; the
/// float compares by [`f32::to_bits`], so `0.0 != -0.0` (two bit patterns, so
/// two values) and two equal `NaN` bit patterns are one value (`NaN == NaN`
/// here, where IEEE `==` refuses).
///
/// That is the relation the three readers of this identity need, and all three
/// compare a value against one the artifact codec reproduced **bit for bit**
/// (tag `8`, `codec.rs`): whether two classes unify on one concrete value
/// (`equality.rs`, via [`ValueExt::value_eq`]), whether two table keys are the
/// same content ([`Module::key_eq`]), and whether a frozen artifact is the
/// value a load is reusing.  "Equal but not the same bits" would let reuse
/// accept one of two distinct artifacts, and IEEE `NaN != NaN` would make a
/// cached `NaN` never match its own reload.  The content hash owes the one
/// direction this forces — equal keys hash equal — and pays it by hashing the
/// bits (`table.rs`, `hash_step`).
///
/// The language's `==` over floats is a separate relation, owned by the
/// operator set (`docs/notes/floating-point.md` §3.7), and is not this one.
impl PartialEq for LowValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (LowValue::Float(a), LowValue::Float(b)) => a.to_bits() == b.to_bits(),
            (LowValue::USize(a), LowValue::USize(b)) => a == b,
            (LowValue::Str(a), LowValue::Str(b)) => a == b,
            (LowValue::Array(a), LowValue::Array(b)) => a == b,
            (LowValue::Table(a), LowValue::Table(b)) => a == b,
            (LowValue::Function(a), LowValue::Function(b)) => a == b,
            (LowValue::None, LowValue::None) => true,
            (LowValue::Void, LowValue::Void) => true,
            (LowValue::Parameterized, LowValue::Parameterized) => true,
            _ => false,
        }
    }
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
    /// Undecided: the class is traced (it has a low-type slot) but nothing has
    /// refined it yet.  The bottom of the lattice — the only answer a reader
    /// may get for a value the graph cannot decide, and the one every backend
    /// must handle with a conservative fallback.
    Unknown,
    /// A machine scalar (`USize`; the kernel-safe scalar subset) — `i64` in
    /// the wasm backend.
    USize,
    /// A machine float — a decided shape beside [`LowShape::USize`] and part of
    /// the same scalar subset: a float with no shape is a float no backend can
    /// trace.  It takes no rule of its own in [`LowShape::join`]: the two are
    /// different decided shapes, which the lattice already sends to
    /// [`LowShape::Unknown`] (`docs/notes/floating-point.md` §3.8).
    Float,
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

impl LowShape {
    /// The lattice join of two lower bounds of one class: the least shape
    /// that is at least as precise as both.  `Unknown` is the bottom
    /// (`Unknown ∨ k = k`), and two equal decided shapes are unchanged.
    ///
    /// `Tuple(..)` of arity `n` and `Array(_, n)` are **two views of the same
    /// array value** — a seeded positional domain and an observed homogeneous
    /// one — so the tuple view wins: it is strictly more precise, and a class
    /// legitimately carries both writers' contributions.
    ///
    /// Two *different* decided shapes join to [`LowShape::Unknown`], and
    /// deliberately so.  The design note expected this case to be unreachable
    /// ("the checker already proved `value : type` consistent, so a debug_assert
    /// suffices"); measurement says otherwise.  A unification **deferral**
    /// merges two classes whose values were never compared — a pending
    /// computation against a skeleton, a deferred field read, a type
    /// round-trip — so two arrays of different arity can legitimately end up
    /// on one class, and the class is only reconciled later, if at all.  An
    /// assertion would therefore fire on ordinary checked programs; the
    /// conservative `Unknown` is the sound answer, and the one the design's own
    /// safety argument requires: a reader degrades to "undecided" rather than to
    /// a wrong shape.
    ///
    /// The cost is bounded by what reads low types: a class whose two writers
    /// disagree is a class the *encoding* arrays live on — a `[value, type]`
    /// pair, a kind, a tuple type's element list — none of which a backend
    /// compiles against.  A kernel domain is a seeded `Tuple` whose arity the
    /// argument agrees with, which is the overlap rule above.
    pub fn join(left: &LowShape, right: &LowShape) -> LowShape {
        match (left, right) {
            (LowShape::Unknown, other) | (other, LowShape::Unknown) => other.clone(),
            (LowShape::Tuple(items), LowShape::Array(_, n)) if items.len() == *n => {
                LowShape::Tuple(items.clone())
            }
            (LowShape::Array(_, n), LowShape::Tuple(items)) if items.len() == *n => {
                LowShape::Tuple(items.clone())
            }
            _ if left == right => left.clone(),
            _ => LowShape::Unknown,
        }
    }

    /// Whether the shape is decided — anything but [`LowShape::Unknown`].
    pub fn is_known(&self) -> bool {
        !matches!(self, LowShape::Unknown)
    }
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
/// marker/`USize` variants compare by their fields, a float by its bits
/// ([`LowValue`]'s [`PartialEq`]), handle payloads compare by pointer identity
/// ([`Handle`]'s [`PartialEq`]).  It decides the fast checks (`is_unbound`,
/// kind-marker lookups); the *full* equality unification merges on is
/// [`ValueExt::value_eq`], which compares handle payloads by content.
/// Equality *through* arrays is not any `==`'s job — unification recurses into
/// them elementwise.
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
/// What a program-specific value may look at while declaring the nodes it keeps
/// alive — the read-only half of a [`Module`], handed to
/// [`ValueExt::traced`].
///
/// **Narrower than `&Module<P>` on purpose, and for a structural reason.** Every
/// question a holder legitimately asks here — which block is this node in, is it
/// still there, what does this block or this function hold — is answered in
/// lowlevel's own types, so none of it needs a program's value type. The single
/// thing `&Module<P>` would add is reading a node's **value**, and a value that
/// keeps nodes alive is keeping ids, not values. Keeping `P` off [`ValueExt`]
/// therefore costs nothing a holder wants and saves a program parameter being
/// threaded through every `ValueType` bound above the highlevel's checker.
///
/// Read-only, and that is not incidental: [`ValueExt::traced`] runs *inside* the
/// GC walk, which holds the module mutably to move what it names.
pub trait TraceContext {
    /// The block `node` is homed in, or `None` if it has been released.
    fn node_block(&self, node: NodeId) -> Option<BlockId>;

    /// The nodes homed directly in `block`, or `None` if it is gone.
    fn block_nodes(&self, block: BlockId) -> Option<&[NodeId]>;

    /// The body nodes of `function`, or `None` if it has been released.
    fn function_nodes(&self, function: FunctionId) -> Option<&[NodeId]>;
}

impl<P: Program> TraceContext for Module<P> {
    fn node_block(&self, node: NodeId) -> Option<BlockId> {
        self.nodes.get(node).map(|node| node.block)
    }

    fn block_nodes(&self, block: BlockId) -> Option<&[NodeId]> {
        self.blocks.get(block).map(|block| block.nodes.as_slice())
    }

    fn function_nodes(&self, function: FunctionId) -> Option<&[NodeId]> {
        self.functions
            .get(function)
            .map(|function| function.nodes.as_slice())
    }
}

/// A resource a frozen [`StaticModule`] owns outside its arena.
///
/// Built by a value at freeze time ([`ValueExt::release_obligations`]) and run
/// exactly once, when the owning artifact is dropped.  It exists so that the
/// ownership transfer a sub-graph freeze performs is **general**: the lowlevel
/// never names a device, a file or any other outside resource — it carries an
/// obligation the program knows how to discharge.
pub trait Release {
    /// Release the resource.  Called exactly once, on the artifact's drop.
    fn release(self: Box<Self>);
}

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
    /// Append every node this one keeps alive to `out`.
    ///
    /// **One kind of thing, exactly like an array item.** The GC walks a node by
    /// looking at that node's *value* and dispatching on its shape — an array's
    /// items, a table's entries, a function's scope. That dispatch is the
    /// walk's business, so a value names only *nodes*, and the same
    /// [`Module::garbage_collect_node`] that walks an array item walks these. A
    /// function is not a separate kind here: a closure is kept alive by naming
    /// the node it is the value of, and the walk takes it from there.
    ///
    /// # Why `out` and not a slice back
    ///
    /// `&[NodeId]` would demand that a value carry its references as one
    /// contiguous run it could hand back by slice, and no real holder has that
    /// shape. A compiled graph interleaves the nodes it keeps with the kernel
    /// ids, counts and element data it also holds, so there is no slice of it
    /// that is "the node list". Appending also costs a value that keeps nothing
    /// nothing at all, where a slice forces it to own an empty array.
    ///
    /// # Why the context is handed over, and why it is not `&Module<P>`
    ///
    /// Because the set need not be *stored*. A value that keeps a node because
    /// of where that node sits — a block it shares, an operand edge above it —
    /// has to be able to look, and the module is where looking happens.
    ///
    /// It is handed over as a [`TraceContext`] rather than as the module itself,
    /// for two reasons that point the same way. It is read-only, so naming and
    /// walking cannot overlap and the walk needs no second phase. And it keeps
    /// `P` off this trait: everything a holder legitimately needs to look at
    /// here — a node's block, a block's node list, a function's scope — is
    /// spelled in lowlevel's own types, so nothing that matters needs a
    /// program's value type. The one thing a `&Module<P>` would add is reading a
    /// node's **value**, and a value that keeps nodes is keeping ids, not values.
    /// Putting `P` on this trait to get at it would cost a program parameter
    /// threaded through every `ValueType` bound in the highlevel's checker, for
    /// a capability no holder wants.
    ///
    /// # Why this is a seam and not a convenience
    ///
    /// The GC's contract is that everything reachable from a live value is moved
    /// out of the block being vacated *before* [`Module::drop_block`] removes it
    /// — and `drop_block` deletes by block membership, not by reachability. An
    /// operator's result is cached, and a cached node's operand is deliberately
    /// not followed ("a cached value means the node is memoized and its operand
    /// is dead"). So a value holding a reference only the GC cannot see is
    /// dropped at the end of the very block evaluation that produced it, with no
    /// diagnostic. That is the failure this closes.
    ///
    /// The default appends nothing, and for the compute vocabulary that is not a
    /// simplification but the truth: a `Buffer` payload is element bytes, a
    /// `DeviceBuffer` is an id and a length, a kernel id is a registry slot
    /// number. None of them names a node, so none of them is an edge the GC has
    /// to follow. A value that *does* keep nodes alive past its own evaluation
    /// answers this — a compiled graph, which holds the **buffer values it will
    /// read on every run** and which live in the block that produced them, so a
    /// graph that did not name them would read freed arena memory. Those
    /// references are behind a process registry, which is sound here for a reason
    /// worth stating: `garbage_collect_node` moves a node by changing its block
    /// and **keeps its id**, so an id held anywhere outside the module stays
    /// valid across a collection.
    ///
    /// The other shape this seam was cut for — a graph holding the *closures* it
    /// will call later — is **not** what answers this today, and the reason is a
    /// recorded contradiction rather than an oversight: the graph IR's native
    /// node is a bare `fn` pointer, which cannot capture and has no channel to
    /// name a closure, so a user-written closure cannot become one without
    /// changing a decision that was made deliberately. See
    /// `docs/notes/compute-graph-jit.md`.
    ///
    /// **A value that fails to answer is not caught.** There is nothing to check
    /// this against: the lowlevel cannot see what a value holds, so an unlisted
    /// node is not a detectable omission, it is a node that quietly disappears
    /// at the end of the block that made it. This contract is held by review and
    /// by the tests beside it, not by the collector. That is worth knowing before
    /// writing one, because the failure it guards is the quiet kind.
    ///
    /// Nodes only, and only from this module: a static-module object is pinned by
    /// the registry for as long as the value can be read, so it needs no edge,
    /// and naming it would be a category error rather than a stronger claim.
    fn traced(&self, context: &dyn TraceContext, out: &mut Vec<NodeId>) {
        let _ = (context, out);
    }

    /// The **release obligations** this value carries: the resources it owns
    /// outside the arena, handed to a freeze so that the artifact it is filed
    /// into can release them when it is dropped — its eviction.
    ///
    /// The ownership half of freezing a sub-graph.  A handle payload is *copied*
    /// into the artifact's arena (the freeze's phase 2); everything else a value
    /// owns is transferred by obligation, and this is where a value says what
    /// that is.  The default owns nothing, so a vocabulary with no out-of-arena
    /// resource pays nothing.
    ///
    /// Per **leaf**, so a composed value dispatches it to whichever leaf carries
    /// the resource — the same shape [`Self::is_handle`] and [`Self::handle`]
    /// have — and a leaf reaches its issuer however it can: a device buffer names
    /// memory by an id that is meaningless outside the backend that issued it, and
    /// that backend is process-wide.  Called once per frozen value, in the
    /// artifact's node order; the value's own copy is untouched, and because a
    /// value dropped by `drop_block` does not release, taking an obligation cannot
    /// double-release.
    fn release_obligations(&self, out: &mut Vec<Box<dyn Release>>) {
        let _ = out;
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

    /// Evaluate this operator's operand and hand the **value** to [`Self::run`].
    ///
    /// The VM calls this rather than `run`, and the default is exactly what the
    /// VM did before this method existed — including the deep pass, the
    /// `Parameterized` read-back, and the nullary stand-in — so an operator that
    /// does not override it cannot tell the difference.
    ///
    /// # Why an operator would override it
    ///
    /// [`Self::run`] is handed a value with the operand's structure already
    /// collapsed into it. That is the right shape for an operator that answers
    /// from the value alone, and the wrong one for an operator that has to
    /// decide for itself *when* its operand is evaluated: the module is already
    /// there, but the node the operand hangs off is not, so there is nowhere to
    /// start. An operator that keeps lichen references alive past its own call
    /// needs that node, and needs it unevaluated, because the references it must
    /// keep alive are in the structure — a compiled graph holds the buffers it
    /// will read, and reading which of a function's nodes are those buffers is a
    /// question about the body's *shape*, which a deep pass has already erased.
    ///
    /// Overriding this is that capability, and it is **only sound together with
    /// [`ValueExt::traced`]**: a reference kept past this call is invisible to
    /// the GC unless the value holding it declares it, and the default GC
    /// contract is that everything reachable moves out of the block before it is
    /// dropped.
    fn run_deferred(
        &self,
        operand: Option<NodeId>,
        block: BlockId,
        module: &mut Module<P>,
    ) -> P::Value {
        let value = match operand {
            Some(node) => {
                let value = module.evaluate_node_deep(node, Some(block));
                // The deep pass returns before it writes `evaluated_deep` when
                // it refuses on budget exhaustion, so an absent node or an unset
                // flag means "concreteness unknown" — read as parameterized,
                // never as proven concrete.
                let parameterized = module
                    .nodes
                    .get(node)
                    .is_none_or(|node| node.evaluated_deep.is_none_or(|deep| deep.parameterized));
                if parameterized {
                    P::Value::from(LowValue::Parameterized)
                } else {
                    value
                }
            }
            // A nullary operator (e.g. `TypeOperator::Fresh`) has no operand
            // node: the honest stand-in is the computed-nothing value — never
            // the `None` unit value, which a program can genuinely produce.
            None => P::Value::from(LowValue::Void),
        };
        self.run(value, block, module)
    }

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

    /// The **low-type transfer** of this operator: the low type its result
    /// has, given the low types of its operand array's elements.
    ///
    /// `arguments` is one entry per element of the operand array, in order;
    /// an entry is `None` when that element's class is untraced, and a
    /// `Some(Unknown)` when it is traced but undecided.  Returning `None`
    /// means the operator **declines**: its result stays undecided and a
    /// reader falls back conservatively — which is the honest default, and
    /// the only one a generic operator can give.
    ///
    /// This is the sole place an operator vocabulary *outside* the lowlevel's
    /// own [`LowOperator`] set states what its computation produces.  It
    /// lives on the operator rather than in the pass because the meaning of an
    /// operator belongs to whoever defined it — the same split that put the
    /// unification policy on [`Program::defer_pending`].
    fn low_type(&self, arguments: &[Option<LowShape>]) -> Option<LowShape> {
        let _ = arguments;
        None
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

impl From<NodeId> for AnyNodeId {
    fn from(node: NodeId) -> Self {
        Self::Dynamic(node)
    }
}

impl AnyNodeId {
    /// The node itself, when it is one of the *reading* module's own — `None`
    /// for a frozen node, whose structure only its own module can walk.
    pub fn dynamic(self) -> Option<NodeId> {
        match self {
            Self::Dynamic(node) => Some(node),
            Self::Static(_) => None,
        }
    }
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
    /// The **template node this one was cloned from**, for a node an apply
    /// instantiated ([`None`] for every other node).  The apply clone walk
    /// records it on each clone it creates, so a reader that holds a per-call
    /// clone — a runtime failure's own operand, say — can reach the template
    /// node the source declares (the highlevel's per-node source
    /// attribution is keyed by the template, never by a clone).  **Private**:
    /// read through [`Module::node_origin`].  The origin is a node of the
    /// *template*: it is referenced, never cloned by the walk, and it is not
    /// a keep-alive edge — garbage collection moves each node with its own
    /// home block, so a reader must tolerate the origin's release.
    origin: Option<NodeId>,
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
    ///
    /// A node a cycle cut re-entered has no verdict yet; the verdict
    /// computation assumes such a node concrete (the coinductive step a
    /// self-referential value needs) and tells it apart by [`Self::assumed_concrete`],
    /// not by this field.  See `P1-31` in `docs/notes/code-audit.md`.
    evaluated_deep: Option<EvaluatedDeep>,
    /// Whether a **cycle cut** assumed this node concrete while its own frame
    /// was still computing it — see `evaluate_node_deep_inner`'s structural
    /// cycle cut.  The verdict computation reads it for a position whose frame
    /// has not written [`Self::evaluated_deep`] yet, which is what tells "in
    /// progress, assumed concrete" apart from "the pass never ran here": the
    /// conflation `P1-31` fixed.  Set at the cut, cleared where the real
    /// verdict is written and where a late operand edge invalidates one.
    /// **Private**.
    assumed_concrete: bool,
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
    /// Diagnostics recorded by layers above the lowlevel — see
    /// [`ExtensionDiagnostic`].  Append-only and never cleared, the same
    /// contract as [`Self::unify_errors`] and [`Self::eval_errors`]: an entry
    /// is a fact about work that already happened, and a second pass must be
    /// able to see the first pass's records.  Recording the *same* fact twice
    /// is the one thing it declines (`Module::record_extension_diagnostic`):
    /// a node is deep-evaluated more than once, and refusing twice is not two
    /// findings.
    pub extension_diagnostics: Vec<ExtensionDiagnostic>,
    /// Program-global extension state — see [`Program::GlobalExt`].
    pub global_ext: P::GlobalExt,
    apply_depth: usize,
    apply_total: usize,
    deep_depth: usize,
}

/// One diagnostic a layer **above** the lowlevel recorded through
/// [`Module::record_extension_diagnostic`]: a plugin refusing to lower a
/// computation, a backend declining to compile a shape, a host error a future
/// extension needs to surface.  The lowlevel stores the entry and knows
/// nothing else about it — not the meaning of `category`, not the node it
/// points at — so adding an external error kind never means adding a channel
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDiagnostic {
    /// The recording layer's own category, so a consumer can select the
    /// diagnostics it renders without parsing the message (for example
    /// `"compute.jit"`).  It is a `&'static str` because a layer's categories
    /// are its own compile-time constants, never a runtime string.
    pub category: &'static str,
    /// The node the diagnostic is about, when the recorder can name one.  A
    /// `None` is an honest "not about a node", not a placeholder: a record
    /// with no attributable node still carries its reason.
    pub node: Option<NodeId>,
    /// The message, rendered by the layer that recorded it — the lowlevel has
    /// no vocabulary for it and never parses it.
    pub message: String,
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
    /// Every module key this artifact's values reference — its frozen
    /// dependencies, recorded at freeze time (the same set
    /// [`Registry::freeze_mapped`] asserts is registered, so computing it is not
    /// extra work).  [`Registry::evict`] reads it to refuse freeing an artifact
    /// that a live one still references: that reference is a raw handle into this
    /// artifact's arena.
    pub refs: HashSet<ModuleKey>,
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
    /// The out-of-arena resources the artifact owns, released when it is dropped
    /// — its eviction (see [`Release`] and [`Program::release_obligations`]).
    /// Empty for an artifact loaded from the device's store: bytes cannot carry
    /// a resource handle, so a loaded artifact owns none, and the store's own
    /// eviction is the only path that has one to run.
    pub releases: Vec<Box<dyn Release>>,
}

impl<P: Program> Drop for StaticModule<P> {
    fn drop(&mut self) {
        for release in self.releases.drain(..) {
            release.release();
        }
    }
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

/// What [`Registry::evict`] did.
///
/// Three outcomes rather than a `bool`, because "it was not there" and "it is
/// still referenced" are different facts to the caller: the first means the key
/// is gone (or was never filed), the second means *retry later*, once the
/// artifact that references it is gone too.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eviction {
    /// The artifact was filed and is now freed.
    Freed,
    /// Nothing was filed under that key.
    NotRegistered,
    /// A live registered artifact references it, so freeing it would leave that
    /// artifact's static ref dangling.
    StillReferenced,
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
    /// The counter behind [`Self::allocate_cell_key`] — the registry's, not a
    /// caller's, because the registry is what a key must be unique *in*: two
    /// sessions (two documents) share one registry and each allocates keys.
    next_cell_key: u64,
}

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
pub use crate::loop_conversion::{
    LoopArm, LoopConversion, LoopExit, LoopRefusal, LoopStep, LoopTest,
};
pub use crate::resolve::{Define, Selection};
pub use crate::static_module::Rehome;
pub(crate) use crate::static_module::StaticModuleCache;

pub mod ancestors;
mod apply;
mod assert;
pub mod codec;
mod equality;
mod evaluation;
mod function;
mod gc;
pub mod loop_conversion;
mod loop_run;
mod low_type;
mod module;
mod registry;
pub mod resolve;
mod static_module;
mod table;
mod utils;

pub trait Program: Sized + Copy + Debug + PartialEq {
    /// The program's value vocabulary: its own variants plus [`LowValue`], composed by `enum_ext!`.
    type Value: ValueExt + From<LowValue> + AsEnum<LowValue> + Clone;
    /// The program's operator vocabulary: its own variants plus [`LowOperator`], same composition.
    type Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>;
    /// Program-global extension state, initialized once by [`Module::new`] and read by extensions.
    type GlobalExt: GlobalExt;
    /// Per-registered-package metadata; the lowlevel treats it as an opaque default-constructible slot.
    type PackageMeta: Default;
}

/// Program-global extension state — the marker trait for `Program::GlobalExt`.
///
/// # Invariant
/// The lowlevel only initializes this state ([`Module::new`] calls
/// `P::GlobalExt::default()`), never copying, comparing or formatting it, so
/// [`Default`] is all it requires.
pub trait GlobalExt: Default {}

/// The structural values the lowlevel itself produces and consumes. See docs/notes/lowlevel-vm.md.
#[derive(Debug, Clone, Copy)]
pub enum LowValue {
    USize(usize),
    /// A machine float — a scalar like `USize`, no handle or GC edge. See docs/notes/floating-point.md.
    Float(f32),
    /// An immutable string literal: a `&'static str`, so `Copy` and arena-free like `USize`.
    Str(&'static str),
    /// A structural array and the attribute layout it carries — see [`ArrayValue`].
    Array(ArrayValue),
    /// A constant table value, entries sorted by key hash; immutable, built once.
    Table(AnyHandle<[TableItem]>),
    Function(AnyFunctionId),
    None,
    /// **The empty value of a failed read**, produced with a recorded [`EvalError`].
    ///
    /// # Invariant
    /// It is decided and cached, so one failed read is one recorded error; undecided is
    /// an **empty node slot** instead, deliberately never cached so the next read
    /// re-runs. It is not the [`Self::None`] unit value. Consumers **propagate** it
    /// rather than re-reporting the failure.
    Error,
    /// **The wildcard**: it unifies with any value and constrains nothing.
    ///
    /// # Invariant
    /// Unification against a class holding `Any` neither merges nor writes, so the
    /// wildcard never propagates and its node stays reusable. A value reader treats it
    /// as **undecided** — no value, no low type — so it never leaks into a computed
    /// result; the **concreteness verdict** alone takes it as decided, which is what
    /// lets a written type be proven concrete and reused in place.
    Any,
}

/// Value identity as the lowlevel decides it: field-wise like the derive, except a float.
///
/// # Invariant
/// A float compares by [`f32::to_bits`], so `0.0 != -0.0` and equal `NaN` patterns are one
/// value; that is the relation the codec's bit-for-bit reloads need, not the language's
/// `==` over floats. See docs/notes/floating-point.md.
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
            (LowValue::Error, LowValue::Error) => true,
            (LowValue::Any, LowValue::Any) => true,
            _ => false,
        }
    }
}

/// A class-routed lower bound on a node's eventual value shape.
///
/// # Invariant
/// A low type belongs to the union-find class, read through its representative by
/// [`Module::class_low_type`]. A position nothing states is `None`, and that is the
/// only "undecided": the shape itself has no `Unknown` variant. It is host
/// metadata, never a lichen value, so "a type is just a value" (`Type : Type`) is
/// untouched. See docs/notes/lowlevel-low-types.md.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LowShape {
    /// A machine scalar (`USize`; the kernel-safe scalar subset) — `i64` in
    /// the wasm backend.
    USize,
    /// A machine float, a decided scalar; two different decided shapes never
    /// share a class.
    Float,
    /// A heterogeneous fixed-arity tuple.  A kernel whose domain is a tuple
    /// has shape `Tuple(..)` and arity = `self.len()`.
    Tuple(Vec<Option<LowShape>>),
    /// A homogeneous fixed-length array: the element shape and the length.
    Array(Box<Option<LowShape>>, usize),
    /// A function: parameter → result.
    Function(Box<Option<LowShape>>, Box<Option<LowShape>>),
    /// A table: key → value.
    Table(Box<Option<LowShape>>, Box<Option<LowShape>>),
}

/// One element of a structural array value: its node and its `shallow` marker.
///
/// # Invariant
/// `shallow` is inert metadata — structure and unification ignore it — but it travels
/// through GC and apply clones, and [`Module::evaluate_node_deep`] skips a marked
/// element's subtree.
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

/// A structural array value's payload plus the **attribute layout** it carries.
///
/// # Invariant
///
/// The layout id is mandatory and never inferred: its `i`-th element names the canonical
/// attribute in the value's `PAIR_ATTR_BASE + i`-th slot, and id `0` is the empty layout
/// [`Self::new`] picks. `Deref` keeps every `unsafe { array.items() }` read as it was; the
/// layout is read through [`Self::layout`], never through the payload.
/// See `docs/notes/array-attribute-view.md`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrayValue {
    items: AnyHandle<[ArrayItem]>,
    layout: u32,
}

impl ArrayValue {
    /// The empty layout's array: a plain value array carrying no attribute.
    pub fn new(items: AnyHandle<[ArrayItem]>) -> Self {
        Self { items, layout: 0 }
    }

    /// The array carrying the registered layout `layout`.
    ///
    /// # Invariant
    /// `layout` names an entry of the owning module's registry — see [`Self`].
    pub fn with_layout(items: AnyHandle<[ArrayItem]>, layout: u32) -> Self {
        Self { items, layout }
    }

    /// The registered layout this array carries.
    pub fn layout(&self) -> u32 {
        self.layout
    }
}

impl From<AnyHandle<[ArrayItem]>> for ArrayValue {
    /// The default (empty) layout — the [`Self::new`] form.
    fn from(items: AnyHandle<[ArrayItem]>) -> Self {
        Self::new(items)
    }
}

impl std::ops::Deref for ArrayValue {
    type Target = AnyHandle<[ArrayItem]>;

    fn deref(&self) -> &AnyHandle<[ArrayItem]> {
        &self.items
    }
}

/// The slot an array's attributes begin at: canonical attribute `i` at `PAIR_ATTR_BASE + i`.
///
/// # Invariant
///
/// This is the arithmetic the module's attribute-layout registry and
/// [`Module::array_attribute`] both read, so a layout's `i`-th entry names the value's
/// `PAIR_ATTR_BASE + i`-th slot and the default's `PAIR_ATTR_BASE + canonical`-th slot. It
/// mirrors `lichen_highlevel::shape::PAIR_ATTR_BASE`; neither crate depends on the other.
pub const PAIR_ATTR_BASE: usize = 2;

/// One entry of a structural table value: key node, value node, and the key's hash.
///
/// # Invariant
/// `hash` is the key's deep-content hash, computed at table build time, so it is stable
/// for the table's life. See docs/notes/lowlevel-vm.md.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableItem {
    pub key: AnyNodeId,
    pub value: AnyNodeId,
    pub hash: u64,
}

impl AnyHandle<[ArrayItem]> {
    /// The array's items — the one written contract for the arena accessors.
    ///
    /// # Safety
    /// The pointer names bytes in a block's arena (dynamic) or a registered static module's
    /// arena; the `'static` return is the arena's lifetime, not a borrow of `&self`, so the
    /// caller owes that the home storage is not released while the slice is read.
    /// [`Module::drop_block`] invalidates a dynamic payload's; a registered static module's
    /// arena lives as long as the registration.
    pub unsafe fn items(&self) -> &'static [ArrayItem] {
        match self {
            // SAFETY: the caller upholds this method's `# Safety`; the payload's arena is live.
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            // SAFETY: the caller upholds this method's `# Safety`; the module is registered.
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
            // SAFETY: the caller upholds this method's `# Safety`; the payload's arena is live.
            AnyHandle::Dynamic(handle) => unsafe { &*handle.0 },
            // SAFETY: the caller upholds this method's `# Safety`; the module is registered.
            AnyHandle::Static(handle) => unsafe { &*handle.offset },
        }
    }
}

/// The structural operators the lowlevel itself dispatches; a program composes it too.
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
    /// A miss records a [`EvalError`] and yields [`LowValue::Error`].
    TableGet,
    /// One instance of a `link`ed library: `library` names it, `instance` the entry.
    ///
    /// # Safety
    ///
    /// The entry is the one the host installed ([`Module::install_external`]),
    /// and its library was built by this module's program and compiler: it reads
    /// the module's values through this build's layouts
    /// (`docs/notes/native-link-library.md` §4). A missing entry is no panic: a
    /// note is recorded and the node stays undecided.
    External { library: u32, instance: u32 },
}

/// One dynamically loaded library's entry, erased to plain pointers so a program
/// generic over `P` can hold it.
///
/// # Safety
///
/// `module` is a live `Module<P>` whose `P` is the library's program, `operand`
/// a `P::Value` of that module, and `out` uninitialized storage for one
/// `P::Value`, which the entry writes exactly once.
pub type ExternalEntry = unsafe extern "C" fn(
    module: *mut std::ffi::c_void,
    operand: *const std::ffi::c_void,
    out: *mut std::ffi::c_void,
);

/// The process-wide entries of loaded libraries, keyed by `(library, instance)`.
///
/// # Invariant
/// A loaded library is process-wide, so its entries are installed here before a
/// build runs: a program's definition pass evaluates while no module is in the
/// caller's hands yet. A module's own table overrides this one.
static PROCESS_EXTERNALS: std::sync::OnceLock<
    std::sync::Mutex<HashMap<(u32, u32), ExternalEntry>>,
> = std::sync::OnceLock::new();

/// Install an entry for every module in this process — the loader's path; see
/// [`PROCESS_EXTERNALS`].
pub fn install_process_external(library: u32, instance: u32, entry: ExternalEntry) {
    PROCESS_EXTERNALS
        .get_or_init(Default::default)
        .lock()
        .expect("the process entry table is not poisoned")
        .insert((library, instance), entry);
}

/// The process-wide entry installed for `(library, instance)`, if any.
pub fn process_external(library: u32, instance: u32) -> Option<ExternalEntry> {
    PROCESS_EXTERNALS
        .get()?
        .lock()
        .expect("the process entry table is not poisoned")
        .get(&(library, instance))
        .copied()
}

/// What a program-specific value may look at while declaring the nodes it keeps alive.
///
/// # Invariant
/// This is the read-only half of a [`Module`], handed to [`ValueExt::traced`], which runs
/// inside the GC walk while the module is held mutably. It is narrower than `&Module<P>`
/// on purpose: every question a holder asks is answered in lowlevel types, and a value
/// that keeps nodes alive keeps ids, not values.
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

/// A resource a frozen [`StaticModule`] owns outside its arena; released once, on drop.
///
/// # Invariant
/// Built by a value at freeze time, so the lowlevel never names a device or file — it
/// carries an obligation the program knows how to discharge.
pub trait Release {
    /// Release the resource.  Called exactly once, on the artifact's drop.
    fn release(self: Box<Self>);
}

/// The cheap, structural equality a value vocabulary must provide.
///
/// # Invariant
/// The crate reads and relocates implementor handle payloads by raw pointer, so
/// [`Self::handle`] stays valid while the value lives with a stable address and length,
/// [`Self::alignment`] is a power of two, and the handle's length is a byte count the copy
/// path moves exactly. The full equality unification merges on is [`Self::value_eq`].
pub trait ValueExt: Debug + Copy + PartialEq {
    fn is_handle(&self) -> bool;
    /// The value's handle payload as bytes; available if [`Self::is_handle()`].
    ///
    /// # Safety
    /// The returned handle stays valid while the value lives, with a stable payload, and
    /// its length is a byte count — exactly the bytes [`Self::value_eq`] compares and the
    /// copy path moves.
    fn handle(&self) -> AnyHandle<[u8]> {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn set_handle(&mut self, _payload: AnyHandle<[u8]>) {
        unreachable!()
    }
    /// The payload alignment handle values need; available if [`Self::is_handle()`].
    ///
    /// # Safety
    /// It must be a power of two: the freeze layout and the crate's copy path derive the
    /// arena slot from it.
    fn alignment() -> usize {
        1
    }
    /// Append every node this one keeps alive to `out`.
    ///
    /// # Invariant
    /// A value names only nodes, and only from this module — the same
    /// [`Module::garbage_collect_node`] that walks an array item walks these. The
    /// [`TraceContext`] is read-only and keeps `P` off this trait. A value that fails to
    /// answer quietly loses a node at the end of its block; nothing checks it. The default
    /// appends nothing. See docs/notes/compiler-plugin.md.
    fn traced(&self, context: &dyn TraceContext, out: &mut Vec<NodeId>) {
        let _ = (context, out);
    }

    /// The out-of-arena resources this value hands a freeze, released when the artifact drops.
    ///
    /// # Invariant
    /// Per leaf, called once per frozen value in artifact node order; the value's own copy
    /// is untouched, and a value dropped by `drop_block` does not release, so taking an
    /// obligation cannot double-release. The default owns nothing.
    fn release_obligations(&self, out: &mut Vec<Box<dyn Release>>) {
        let _ = out;
    }
    /// Full equality of two values: handle payloads by content, else the derived [`PartialEq`].
    ///
    /// # Invariant
    /// An array is one allocation, so two arrays compare equal only when they share it;
    /// structural equality through arrays is unification's elementwise recursion.
    fn value_eq(&self, other: &Self) -> bool {
        if self.is_handle()
            && other.is_handle()
            && std::mem::discriminant(self) == std::mem::discriminant(other)
        {
            let (a, b) = (self.handle(), other.handle());
            return a.len() == b.len()
                && (std::ptr::eq(a.as_ptr(), b.as_ptr())
                    // SAFETY: both payloads have the same byte length and are alive for this call.
                    || unsafe {
                        std::slice::from_raw_parts(a.as_ptr(), a.len())
                            == std::slice::from_raw_parts(b.as_ptr(), b.len())
                    });
        }
        self == other
    }
}

pub trait OperatorExt<P: Program>: Debug + Copy {
    /// This operator's answer, or `None` when it cannot decide yet — undecided, not an error.
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> Option<P::Value>;

    /// Evaluate this operator's operand and hand the value to [`Self::run`].
    ///
    /// # Invariant
    /// Overriding is only sound together with [`ValueExt::traced`]: a reference kept past
    /// this call is invisible to the GC unless the value declares it. See
    /// docs/notes/compiler-plugin.md.
    fn run_deferred(
        &self,
        operand: Option<NodeId>,
        block: BlockId,
        module: &mut Module<P>,
    ) -> Option<P::Value> {
        let value = match operand {
            Some(node) => {
                let value = module.evaluate_node_deep(node, Some(block));
                // An absent node or unset `evaluated_deep` means concreteness unknown, never concrete.
                let undecided = module.node_is_undecided(node);
                if undecided {
                    return None;
                }
                // A deep pass that answered nothing means the same thing this
                // operator must say: undecided.
                value?
            }
            // A nullary operator's stand-in is the computed-nothing value, never the unit `None`.
            None => P::Value::from(LowValue::Error),
        };
        self.run(value, block, module)
    }

    /// Whether a callee node the lowlevel cannot prove a function is a value this vocabulary
    /// applies.
    ///
    /// # Invariant
    /// Only the program knows which of its values are callable, because a structural array
    /// is the same shape for a struct instance and a kernel's `[native, sig]` pair. The
    /// policy reads the module and must not mutate it; the default refuses.
    fn is_callable(module: &Module<P>, callee: AnyNodeId) -> bool {
        let _ = (module, callee);
        false
    }

    /// The low-type transfer of this operator: the low type its result has, given its operand
    /// elements' low types.
    ///
    /// # Invariant
    /// `arguments` has one entry per operand element; `None` means untraced and
    /// A shape is stated once; returning `None` means the operator declines and
    /// a reader falls back conservatively.
    fn low_type(&self, arguments: &[Option<LowShape>]) -> Option<LowShape> {
        let _ = arguments;
        None
    }
}

// Structural operators implement OperatorExt for uniform composition; the VM never calls run.
impl<P: Program> OperatorExt<P> for LowOperator {
    fn run(
        &self,
        _operand: P::Value,
        _block: BlockId,
        _module: &mut Module<P>,
    ) -> Option<P::Value> {
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

/// Pointer into a [`Block::arena`] — or, after a freeze, into a [`StaticModule`]'s arena.
///
/// # Invariant
/// `PartialEq` is pointer identity, never a dereference; content equality is
/// [`ValueExt::value_eq`]. The pointer is private, built through [`Handle::from_raw`],
/// whose contract carries the arena-lifetime obligation.
#[derive(Debug)]
pub struct Handle<T: ?Sized>(pub(crate) *const T);
#[derive(Debug)]
pub struct StaticHandle<T: ?Sized> {
    /// The payload's home module; global, so the handle reads identically from any importer.
    pub module: ModuleKey,
    /// The payload's address in the home module's arena; the codec resolves a stored offset.
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
    /// the copy path relocated it to — and must stay valid: the payload's home
    /// block must not be released while any reader may dereference it.
    /// [`Module::drop_block`] releasing the block's `Bump` invalidates it.
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
    /// The static mirror of [`Handle::from_raw`]: a payload a freeze filed into `module`.
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

impl StaticHandle<[ArrayItem]> {
    /// The payload as the **fat** pointer the relocated-handle codec writes.
    ///
    /// # Invariant
    /// The length is the payload's own, as the freeze laid it out and the codec
    /// resolved a stored offset, so a writer records the element count the
    /// artifact must restore — not a count taken from a live slice.
    pub fn items_ptr(&self) -> *const [ArrayItem] {
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
    /// Identity: both handles name the same storage, so two importers of one module share it.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AnyHandle::Dynamic(a), AnyHandle::Dynamic(b)) => a == b,
            (AnyHandle::Static(a), AnyHandle::Static(b)) => a == b,
            _ => false,
        }
    }
}

impl Handle<[u8]> {
    /// The payload's byte length, from the fat pointer metadata; no reference is formed.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl StaticHandle<[u8]> {
    /// The payload's byte length, from the fat pointer metadata; no reference is formed.
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

/// The device key naming a compiled module, re-exported from `lichen-registry`.
pub use lichen_registry::ModuleKey;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StaticNodeId {
    /// The target module; refs are absolute from birth, so the same ref reads identically.
    pub module: ModuleKey,
    pub index: LocalNodeId,
}
/// A node's index within its own static module; `Ord` groups a set by a stable sort.
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
    /// The node, when it is the reading module's own; `None` for a frozen node.
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

/// A function's ultimate identity after following origins. See docs/notes/function-type-merge.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionIdentity {
    /// A function the source built, with no re-export/materialization origin —
    /// its own [`FunctionId`] is its identity.
    Dynamic(FunctionId),
    /// A frozen function that is its own original (no further origin).
    Static(StaticFunctionRef),
}

/// A garbage-collection unit.
///
/// # Invariant
/// Only one node can be referenced from a parent block, and referencing a node whose
/// block was released is a panic.
#[derive(Debug)]
pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
    /// Functions homed in this block, registered like nodes so GC drops them with it.
    pub functions: Vec<FunctionId>,
    /// The node whose evaluation collects this block, when a layer
    /// declared it a scope (`mark_scope_root`); `None` otherwise.
    pub scope_root: Option<NodeId>,
}

/// The top-level function shell.
///
/// # Invariant
/// Only [`Self::nodes`] can reference [`Self::parameter`].
#[derive(Debug, Clone)]
pub struct Function {
    /// The template scope: the nodes owned by this function's body.
    ///
    /// # Invariant
    /// A node belongs to the template iff its [`Node::function`] chain through
    /// [`Self::parent`] reaches the applied function; this list is only the clone's
    /// starting set and the GC root set, iterated, never queried.
    pub nodes: Vec<NodeId>,
    pub r#return: NodeId,
    pub parameter: NodeId,
    /// The return expression's type cell; `r#return` itself may be an unevaluated operation.
    pub return_type: NodeId,
    /// The marker index of the attribute the return body states.
    pub return_marker: Option<u32>,
    /// The field the return's attribute follows, when the body states a reference.
    pub return_field: Option<u32>,
    /// The static function this closure was materialized from; `None` for a source closure.
    pub static_origin: Option<StaticFunctionRef>,
    /// The template a minted closure was made from; `None` for a template itself.
    ///
    /// # Invariant
    /// A node owned by a function nested in this template belongs to this template's body
    /// graph, so the membership test reads this id beside [`Self::parent`]: a closure
    /// instantiated per call must walk its template's body, and the template's own
    /// lexical chain never names the clone.
    pub template: Option<FunctionId>,
    /// The lexical parent function, or `None` at top level; not a keep-alive edge.
    pub parent: Option<FunctionId>,
    /// The body's assert conditions, cloned and re-registered per apply.
    ///
    /// # Invariant
    /// A condition the deep pass proved concrete is per-call invariant, so it is referenced
    /// in place and not re-registered; an unproven one is cloned and re-checked against each
    /// call's argument.
    pub asserts: Vec<NodeId>,
    /// Owner.
    pub block: BlockId,
    /// The `@loop` mark: permission, not a command, and the parameter slot that holds
    /// the loop's state.
    ///
    /// # Invariant
    /// Nothing here acts on the mark by itself. It changes what a recursion the deep pass
    /// cannot decide does — an expansion of it is what runs out of budget — and the
    /// conversion that replaces the expansion is built from this graph. It rides here
    /// because the cycle is a fact about this graph, not about syntax.
    /// See docs/notes/loop-conversion.md.
    pub looping: Option<usize>,
}

pub struct StaticFunction {
    pub parameter: LocalNodeId,
    pub r#return: LocalNodeId,
    /// The static mirror of [`Function::return_type`].
    pub return_type: LocalNodeId,
    /// The static mirror of [`Function::return_marker`].
    pub return_marker: Option<u32>,
    /// The static mirror of [`Function::return_field`].
    pub return_field: Option<u32>,
    /// The real original this static function is a re-export of; `None` for its own.
    pub origin: Option<StaticFunctionRef>,
    pub asserts: Vec<LocalNodeId>,
    /// The template scope as local indices — the static mirror of [`Function::nodes`].
    ///
    /// # Invariant
    /// A nested static closure re-homed as a dynamic `Function` uses it as the fresh
    /// closure's scope, so the clone walk can tell its own nodes from its captures.
    pub nodes: Vec<LocalNodeId>,
    /// The body reaches an `undecided` node outside this function's own template scope.
    pub open_captures: bool,
}

/// The deep pass's verdict on one node: whether its reachable subtree is undecided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvaluatedDeep {
    /// The deep pass could not prove the subtree concrete: some reachable slot is empty.
    pub undecided: bool,
}

#[derive(Debug)]
pub struct Node<P: Program> {
    /// The node's **private** value slot; every member of a class holds the class's one value.
    ///
    /// # Invariant
    /// A slot that holds a value is never given a different one — `write_node_value`
    /// asserts it, GC's relocation being the one licensed rewrite. Two axes meet here:
    /// decided or not ([`Module::node_value`]) and has-run or not, the run stamp
    /// ([`Module::has_no_result_yet`]). An operation-bearing member holds that value
    /// while the stamp stays zero, so it still reads as owed an answer.
    value: Option<P::Value>,
    /// The node's run stamp: zero means its operator never ran — a produced answer versus
    /// an asserted one.
    runned: u32,
    /// The node's low-type slot, behind the same private gate. See docs/notes/lowlevel-low-types.md.
    low_shape: Option<LowShape>,
    /// The node's computation (operator plus one operand), or `None` for a value node; private.
    operation: Option<Operation<P>>,
    /// The function whose body owns this node, or `None`; private, read via `node_function`.
    function: Option<FunctionId>,
    /// The node this clone is attributed through, or `None` for every other node.
    ///
    /// # Invariant
    /// A dynamic clone records the template node it instantiates; a static materializer's
    /// clone records the apply operation node that materialized it, because its template is
    /// in the frozen module. Following the origin reaches an attributed node, and it is not
    /// a keep-alive edge. Read through [`Module::node_origin`].
    origin: Option<NodeId>,
    /// How many apply levels this node was created under: `0` for the program's own nodes.
    ///
    /// # Invariant
    /// The stamp comes from the apply node the instantiation is for, not from the walk or
    /// the evaluation order, so a converted loop's iterations are all stamped alike. Read
    /// through [`Module::node_depth`]; a frame counter could not state it.
    depth: u32,
    /// The GC unit whose lifetime bounds this node; moved only by GC.
    block: BlockId,
    /// Whether an evaluation attempt is computing this node right now — the cycle mark.
    visiting: bool,
    /// The deep pass's verdict on this node, or `None` if it never ran.
    ///
    /// # Invariant
    /// A node a cycle cut re-entered has no verdict yet; the verdict computation assumes
    /// such a node concrete (the coinductive step a self-referential value needs) and tells
    /// it apart by [`Self::assumed_concrete`], not by this field. Read through
    /// [`Module::node_evaluated_deep`].
    evaluated_deep: Option<EvaluatedDeep>,
    /// Whether a cycle cut assumed this node concrete while its own frame was computing it.
    ///
    /// # Invariant
    /// Tells "in progress, assumed concrete" apart from "the pass never ran here": the
    /// verdict computation reads it for a position whose frame has not written
    /// [`Self::evaluated_deep`] yet. Set at the cut, cleared where the real verdict is
    /// written and where a late operand edge invalidates one.
    assumed_concrete: bool,
    /// Disjoint-set metadata for node equality classes; the union-find is the only writer.
    equality: disjoint::Meta<NodeId>,
}

pub struct StaticNode<P: Program> {
    pub value: Option<P::Value>,
    /// The [`LowShape`] copied from the source node by a freeze.
    pub low_shape: Option<LowShape>,
    pub operation: Option<StaticOperation<P>>,
    pub equality: disjoint::Meta<LocalNodeId>,
    /// Whether the source node's operator had run: an artifact carries no run clock, so this
    /// mirror is its zero test.
    pub runned: bool,
    /// The source node's deep-pass verdict. See [`Node::evaluated_deep`].
    pub evaluated_deep: Option<EvaluatedDeep>,
}

impl<P: Program> StaticNode<P> {
    /// The solved concreteness flag: `true` when never deep-passed (conservative).
    pub fn undecided(&self) -> bool {
        self.evaluated_deep.is_none_or(|e| e.undecided)
    }
}

/// Which evaluation budget a [`Module`] exhausted, and its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExhausted {
    /// Nested applications ran deeper than [`Module::apply_depth_limit`]; measured on the node.
    ApplyDepth { limit: usize },
    /// [`Module::evaluate_node_deep`] nested deeper than [`Module::evaluate_depth_limit`].
    EvaluateDepth { limit: usize },
}

pub struct Module<P: Program> {
    /// The device's registry, shared with every module bound to it.
    pub registry: Arc<RwLock<Registry<P>>>,
    /// The node table: the slot allocation that **names** a node and owns its lifetime.
    ///
    /// # Invariant
    /// Node state is private: every state field of [`Node`] is reached only through the
    /// `node_*` accessors. The table stays public because iteration and the union-find walk
    /// it, and a released node's absence is itself a readable fact.
    pub nodes: SlotMap<NodeId, Node<P>>,
    pub blocks: SlotMap<BlockId, Block>,
    pub functions: SlotMap<FunctionId, Function>,
    /// The **full-width layout**: the attribute order's own
    /// (`docs/notes/compound-element-slots.md`).
    ///
    /// # Invariant
    /// One layout per program, because the attribute order is the program's.
    pub full_width_layout: Option<u32>,
    /// The **type node** of each element value, keyed by the value node.
    ///
    /// # Invariant
    /// A compound's element is a slot, and the slot's second position is the element's
    /// type (`docs/notes/compound-element-slots.md`); the checker states that
    /// correspondence when it builds the slot, so a reader holding only the value node
    /// reaches the type node that is its shape authority.
    pub element_types: HashMap<NodeId, AnyNodeId>,
    /// The clone's return pair of each type-side apply, keyed by the apply node.
    pub applied_pairs: HashMap<NodeId, NodeId>,
    /// The clone's parameter of each type-side apply, keyed by the apply node.
    pub applied_parameters: HashMap<NodeId, NodeId>,
    /// The value-side apply of a call, keyed by its type-side apply.
    pub apply_pairs: HashMap<NodeId, NodeId>,
    /// Nested-application guard: refuses when an application's node depth exceeds this.
    ///
    /// # Invariant
    /// Depth is [`Self::node_depth`], so an expansion reaches the bound at its trip count
    /// while a converted loop's iterations never do. Defaults to [`Self::MAX_APPLY_DEPTH`].
    pub apply_depth_limit: usize,
    /// Deep-evaluation guard: refuses when [`Self::evaluate_node_deep`] nests deeper than this.
    ///
    /// # Invariant
    /// The default sits above the legitimately ~200k-deep block chains the `#[stacksafe]`
    /// tests exercise.
    pub evaluate_depth_limit: usize,
    /// Which budget a guard refused on; cleared only by a whole-run reset.
    ///
    /// # Invariant
    /// It is the record of why the last walk stopped, so a second walk knows it ran against
    /// an already abandoned graph.
    pub budget_exhausted: Option<BudgetExhausted>,
    /// Whether the type side's parameter check keeps its errors (a `Check` build).
    ///
    /// # Invariant
    /// It is a build fact, not a runtime mode: the type side reads it to choose
    /// between discarding and reporting its errors; the value side never does.
    pub report_type_side_failures: bool,
    pub unify_errors: Vec<UnifyError<P>>,
    /// Runtime evaluation failures, recorded instead of panicking; append-only like unify_errors.
    pub eval_errors: Vec<EvalError>,
    /// The assert worklist: conditions registered by [`Self::add_assert`].
    ///
    /// # Invariant
    /// [`Self::check_asserts`] drains it, consuming decided entries; GC prunes dropped
    /// blocks' entries. Each entry keeps the body condition it came from.
    pub asserts: Vec<PendingAssert>,
    /// Failed asserts: conditions that resolved to a concrete value other than `USize(1)`.
    ///
    /// # Invariant
    /// A condition that stays lazy is not triggered and records nothing; the list is
    /// append-only and never cleared.
    pub assert_errors: Vec<AssertError<P>>,
    /// Failed apply-time parameter checks, with the context the highlevel attributes by.
    ///
    /// # Invariant
    /// One entry per failed `function_apply`; the raw [`UnifyError`] entries it produced stay
    /// in [`Self::unify_errors`] alongside it.
    pub apply_errors: Vec<ApplyError>,
    /// The apply nodes [`Self::apply_errors`] already holds an entry for; the dedup's test.
    apply_error_nodes: HashSet<NodeId>,
    /// Diagnostics and refusals a layer above the lowlevel recorded; see
    /// [`ExtensionDiagnostic`].
    ///
    /// # Invariant
    /// Append-only and never cleared, so a second pass sees the first pass's records.
    /// Recording the same fact twice is declined: a node is deep-evaluated more than once.
    pub extension_diagnostics: Vec<ExtensionDiagnostic>,
    /// The operator whose run is in progress, for a record that names no node.
    ///
    /// # Invariant
    /// Set only around an extension operator's call and restored after it, so a plugin's
    /// unnamed record is about the operator that made it
    /// (`docs/notes/compiler-plugin.md`).
    operator_for_diagnostics: Option<NodeId>,
    /// Program-global extension state — see [`Program::GlobalExt`].
    pub global_ext: P::GlobalExt,
    /// The loaded libraries' entries, keyed by `(library, instance)` — see
    /// [`Module::install_external`].
    ///
    /// # Invariant
    /// Host-installed and never serialized: an artifact that names an
    /// [`LowOperator::External`] operator requires the same library installed
    /// wherever it is read.
    externals: HashMap<(u32, u32), ExternalEntry>,
    /// The **mark anchors** each lowered marked expression reached, keyed by the
    /// node its value became.
    ///
    /// # Invariant
    /// A mark is **text and nothing else**: no slot node rides along, so there is no node here
    /// for a reader to reach a value through, and nothing to unify, propagate or evaluate.
    /// The key is the one place a mark's *position* crosses into this layer as data
    /// (graph-fusion.md §3); a reader asks through [`Module::marks_of`].
    marks: HashMap<NodeId, Vec<String>>,
    /// The equality classes one [`Module::garbage_collect`] walk has entered, so
    /// each is walked once.
    ///
    /// # Invariant
    /// Empty between collects: [`Module::garbage_collect`] clears it, and a walk
    /// never re-enters another, so the set only ever holds one collect's classes.
    gc_classes: HashSet<NodeId>,
    /// The nodes one [`Module::garbage_collect`] walk has entered.
    ///
    /// # Invariant
    /// Empty between collects: a node is walked once, so a value cycle terminates and a
    /// node reached twice is not relocated twice.
    gc_visited: HashSet<NodeId>,
    /// The equality classes whose member-first evaluation walk is in progress.
    ///
    /// # Invariant
    /// A class is held exactly while one frame walks its members and cleared on every exit.
    /// The guard, not the union-find's finiteness, is what bounds that walk: reading a
    /// member re-enters the same class, so an unguarded walk never returns.
    evaluating_classes: HashSet<NodeId>,
    /// The unification walk's two recursion buffers, kept between calls.
    ///
    /// # Invariant
    /// [`Module::unify`] takes both with [`std::mem::take`] and puts them back on every
    /// exit, so the struct never holds a buffer a walk is using, and each walk starts
    /// from the empty state the recursion requires: the path holds no pair and the step
    /// stack is empty. Capacity is what is kept, never content.
    unify_path: crate::ancestors::AncestorPairs<NodeId>,
    unify_steps: Vec<UnifyStep>,
    /// The member snapshot one class-first read iterates, kept between reads.
    ///
    /// # Invariant
    /// [`Module::evaluate_class_first`] takes it and puts it back on every exit, empty
    /// of content at the take and the put-back; a nested read that takes it while a
    /// frame holds it gets a fresh one and leaves the holder's own to the holder.
    class_snapshot: Vec<NodeId>,
    /// The depth [`Module::add_node`] stamps on the nodes it creates, or zero outside an apply.
    stamp_depth: u32,
    deep_depth: usize,
    /// The module's **attribute-layout registry**: one entry per schema tail.
    ///
    /// # Invariant
    ///
    /// Entry `0` is the empty layout, present from construction, and every other entry's
    /// canonical indices are strictly increasing — which is what makes
    /// [`Module::array_attribute`]'s search sound. An id is a position here, so appending
    /// never renumbers one already in flight. See `docs/notes/array-attribute-view.md`.
    pub(crate) attribute_layouts: Vec<Box<[u32]>>,
    /// The registry's O(1) de-duplication index: the layout as key, its id as value.
    ///
    /// # Invariant
    ///
    /// Exactly the mirror of [`Self::attribute_layouts`] — one entry per registry entry,
    /// holding the same layout — so a re-registration of an identical layout returns the
    /// existing id and cannot allocate a second one for the same schema tail.
    pub(crate) attribute_layout_ids: HashMap<Box<[u32]>, u32>,
    /// The module-global **default attribute array**: the full-width slots an array's
    /// layout does not cover.
    ///
    /// # Invariant
    ///
    /// It is full width over the canonical attribute order, so attribute `i` reads at
    /// [`PAIR_ATTR_BASE`] + `i`; `None` is a module that has declared none, and every hole
    /// then reads as absent. See `docs/notes/array-attribute-view.md`.
    pub(crate) default_attributes: Option<AnyHandle<[ArrayItem]>>,
}

/// One diagnostic a layer above the lowlevel recorded. See [`Module::record_extension_diagnostic`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDiagnostic {
    /// The recording layer's own category, a `&'static str` compile-time constant.
    pub category: &'static str,
    /// The node the diagnostic is about, if any; `None` is not about a node, not a placeholder.
    pub node: Option<NodeId>,
    /// The message, rendered by the layer that recorded it — the lowlevel never parses it.
    pub message: String,
    /// Whether this record **fails the build** rather than noting and leaving it usable.
    ///
    /// # Invariant
    /// It is set by the recording layer, never inferred: only
    /// [`Module::has_extension_failures`] and a reader naming the two kinds consume it,
    /// so the lowlevel itself never branches on it. It is deliberately **not** part of
    /// the duplicate test — the same sentence is one fact whichever way it was recorded.
    pub fails_build: bool,
}

impl ExtensionDiagnostic {
    /// A record that leaves the build usable. See [`Self::fails_build`].
    pub fn note(category: &'static str, node: Option<NodeId>, message: impl Into<String>) -> Self {
        ExtensionDiagnostic {
            category,
            node,
            message: message.into(),
            fails_build: false,
        }
    }

    /// A record that fails the build. See [`Self::fails_build`].
    pub fn failure(
        category: &'static str,
        node: Option<NodeId>,
        message: impl Into<String>,
    ) -> Self {
        ExtensionDiagnostic {
            category,
            node,
            message: message.into(),
            fails_build: true,
        }
    }
}

/// One entry of the [`Registry`]: a registered static module plus per-key state.
pub struct Package<P: Program> {
    pub module: Arc<StaticModule<P>>,
    /// Opaque per-package metadata; see [`Program::PackageMeta`].
    pub meta: P::PackageMeta,
    /// The artifact's content hash, the module's stable identity on the device.
    pub hash: [u8; 32],
    /// Every module key this artifact's values reference; eviction refuses while one is live.
    pub refs: HashSet<ModuleKey>,
}

/// What a frozen artifact must remain good for.
///
/// # Invariant
/// `Portable` (default) keeps every operation, so its values are content-identical to the
/// module's and can be table keys. `ReadOnly` drops a decided node's operation and the
/// operand chain, which is far smaller and **not key-portable**: the key hash refuses.
/// See `docs/notes/heavy-value-cache.md` §9.4.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FreezeMode {
    #[default]
    Portable,
    ReadOnly,
}

/// A fully-solved module frozen into an immutable, shareable form.
pub struct StaticModule<P: Program> {
    /// The module's global name, allocated at freeze and carried by every ref into it.
    pub key: ModuleKey,
    pub nodes: Vec<StaticNode<P>>,
    pub functions: Vec<StaticFunction>,
    /// The flattened payload arena: array item slices and ext-value bytes, deduped by `(ptr, len)`.
    pub arena: Vec<u8>,
    /// The frozen module's **attribute-layout registry**, copied verbatim from the source.
    ///
    /// # Invariant
    ///
    /// Entry `0` is the empty layout and an array's layout id indexes this vector, so a
    /// registry order that disagreed with the source's would re-read every frozen array's
    /// attributes; the freeze copies rather than rebuilds, and the codec round-trips it in
    /// order. See [`Module::attribute_layouts`].
    pub attribute_layouts: Vec<Box<[u32]>>,
    /// The frozen module's **default attribute array**, copied from the source.
    ///
    /// # Invariant
    ///
    /// It is a **static** handle when the source held one: the freeze's payload pass lays
    /// a dynamic default out and re-keys it as it does a node's array payload, so an
    /// artifact's default names the artifact's own arena and never a released block's.
    pub default_attributes: Option<StaticHandle<[ArrayItem]>>,
    /// Whether the artifact was frozen `ReadOnly`; its values are not content-portable.
    pub read_only: bool,
    /// The out-of-arena resources the artifact owns, released on drop.
    ///
    /// # Invariant
    /// Empty for an artifact loaded from the store: bytes cannot carry a resource handle, so the
    /// store's own eviction is the only path with one to run.
    pub releases: Vec<Box<dyn Release>>,
}

impl<P: Program> Drop for StaticModule<P> {
    fn drop(&mut self) {
        for release in self.releases.drain(..) {
            release.release();
        }
    }
}

/// The result of freezing a dynamic module: the device key and the source→static node map.
pub struct Freeze {
    /// The freshly allocated device key.
    pub key: ModuleKey,
    /// Source `NodeId` → home-module local node index.
    pub node_map: HashMap<NodeId, LocalNodeId>,
}

/// What [`Registry::evict`] did: freed, not registered, or still referenced.
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

/// The device's module registry — the virtual file system of compiled modules.
///
/// # Invariant
/// A filed value carries raw arena pointers, so the registry never crosses a thread: the
/// sharing is within one thread. The resident map is keyed by [`ModuleKey`], a compact
/// index stable across processes; the device registry outside the lowlevel decides when one
/// is reused.
pub struct Registry<P: Program> {
    entries: HashMap<ModuleKey, Package<P>>,
    /// The counter behind [`Self::allocate_cell_key`]; the registry's, not a caller's.
    next_cell_key: u64,
}

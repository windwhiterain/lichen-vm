//! The lowered-kernel IR and the backend contract that consumes it.
//! See docs/notes/compute-jit-low-types.md.
//!
//! # Invariant
//! The IR is narrower than any target's instruction set, leaving calling convention,
//! storage layout and how a buffer reference is obtained to a backend; the body is
//! structured control flow, because a loop needs a backedge and a value that survives it.
//! The crate depends on nothing, so a backend inherits no other's runtime.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

mod barrier;
mod body;
mod compose;
mod fan_in;
mod marks;
mod plan;
mod unary;

pub use body::{
    Barrier, BasicBlock, Br, FlatOp, KernelBody, NotIndexLocal, Terminator, ValueDef, ValueId,
    from_flat,
};
pub(crate) use body::{Cone, ConeOrigin};
pub use compose::{ComposeRefusal, compose, compose_fan_out};
pub use fan_in::{Producer, compose_fan_in};
pub use marks::{CLAIM_PREFIX, ClaimRefusal, KernelClaim, KernelMark, MarkField};
pub use plan::{Plan, PlanRefusal, Staging};
pub use unary::{
    TRANSCENDENTALS, cos_body, cos_value, exp_body, exp_value, floor_body, floor_value,
    inlined_cos_operand, inlined_exp_operand, inlined_ln_operand, inlined_sin_operand,
    inlined_sqrt_operand, ln_body, ln_value, sin_body, sin_cos_value, sin_value, sqrt_body,
    sqrt_value,
};

/// What an element is: an `Int` (a machine-sized unsigned integer, `i64`) or a `Float`
/// (`f32`).
///
/// # Invariant
/// Fieldless: the class is a tag, and the one representation fact that follows — its
/// width — is [`Self::byte_width`], because a field would be a second place the same
/// number could be written wrong. A single variant space over both classes would give
/// `bits()` two ways to answer `32` with no way to tell which it described.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarClass {
    /// An unsigned machine-sized integer — the language's `Int`.
    Int,
    /// A 32-bit float — the language's `Float`.
    Float,
    /// A brain float: the top sixteen bits of a `Float`, and the only half a model needs.
    BFloat16,
}

impl ScalarClass {
    /// The bytes one buffer element of this class occupies.
    ///
    /// # Invariant
    /// The one answer, on the class because the class is the only thing that knows it:
    /// every width — host byte arithmetic, a module's `ArrayStride`, a dispatch's padding —
    /// is derived from here. Two copies of the rule is how the wasm backend came to lay a
    /// float element out at eight bytes while SPIR-V read four.
    pub fn byte_width(self) -> usize {
        match self {
            ScalarClass::Int => 8,
            ScalarClass::Float => 4,
            ScalarClass::BFloat16 => 2,
        }
    }

    /// A dense index for this class, so a backend can key a per-class table on it.
    ///
    /// # Invariant
    /// The two are in step with the two variants: a module holding one element type per
    /// class — an SPIR-V storage buffer, whose `ArrayStride` is the class's
    /// `byte_width()` — sizes its table by the number of classes, not by a number
    /// restated beside it.
    pub const ALL: [ScalarClass; 3] = [ScalarClass::Int, ScalarClass::Float, ScalarClass::BFloat16];

    pub fn index(self) -> usize {
        match self {
            ScalarClass::Int => 0,
            ScalarClass::Float => 1,
            ScalarClass::BFloat16 => 2,
        }
    }
}

/// A payload that crossed back from a backend, with the class it is to be read as.
///
/// # Invariant
/// The class lives in the value because [`ParallelBackend::fetch`]'s implementor has no
/// fragment to consult — the caller holds only a [`ResidentId`] and its count — so a
/// `Float` buffer cannot be handed over as integers. The payload is the buffer's bytes and
/// the variant says how to read them: no conversion happens here.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarData {
    /// `Int` elements, one machine-sized unsigned integer each.
    Int(Vec<i64>),
    /// `Float` elements, one `f32` each.
    Float(Vec<f32>),
}

impl ScalarData {
    /// The class this payload is to be read as.
    pub fn class(&self) -> ScalarClass {
        match self {
            ScalarData::Int(_) => ScalarClass::Int,
            ScalarData::Float(_) => ScalarClass::Float,
        }
    }

    /// How many elements the payload holds.
    pub fn len(&self) -> usize {
        match self {
            ScalarData::Int(elements) => elements.len(),
            ScalarData::Float(elements) => elements.len(),
        }
    }

    /// Whether the payload holds no elements.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The elements as an integer payload, or `None` for a float one.
    ///
    /// # Invariant
    /// Total rather than an `unwrap`: a caller that has not matched the class gets to name
    /// the mismatch instead of panicking.
    pub fn as_ints(&self) -> Option<&[i64]> {
        match self {
            ScalarData::Int(elements) => Some(elements),
            ScalarData::Float(_) => None,
        }
    }

    /// The elements as a float payload, or `None` for an integer one.
    pub fn as_floats(&self) -> Option<&[f32]> {
        match self {
            ScalarData::Float(elements) => Some(elements),
            ScalarData::Int(_) => None,
        }
    }
}

/// A backend's own name for a buffer it is holding.
///
/// # Invariant
/// Opaque and backend-scoped: an id means nothing without the backend that issued it, so
/// it is never compared across backends and never persisted; passing one to another
/// backend is a caller error, not a lookup that misses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResidentId(pub u64);

/// One input buffer as it crosses the backend boundary.
///
/// # Invariant
/// Data the backend already holds costs nothing to use again: a kernel chain runs without
/// its intermediates reaching the host. A host slot carries no class — that is the
/// dispatch's — and its payload is packed elements at [`ScalarClass::byte_width`] bytes
/// each. It is `PartialEq`, not `Eq`: equal-length bytes can hold different counts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BufferSlot<'a> {
    /// Data the host already holds: `count` elements, read as the class the fragment
    /// declares for this position.
    Host(&'a [u8]),
    /// A buffer the backend is already holding, from an earlier [`ParallelBackend::run`].
    Resident(ResidentId),
}

/// Where a dispatch's index range lives when the device, not the host, computed it.
///
/// # Invariant
/// **A place, not a number**: the host has not read it, which is the whole point — a count
/// fetched back is a stall between two dispatches. `capacity` is how many elements the run is
/// **allocated** for, because a buffer must be sized before the number deciding how much of it
/// to write is known. What surrounds the count is the override's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceExtent {
    /// The buffer holding the number, as one `Int` element — what a `compute.write` produces.
    pub count: ResidentId,
    /// Which element of that buffer holds it.
    pub index: usize,
    /// How many elements this run's outputs are allocated for.
    pub capacity: usize,
}

/// A submission a backend has taken but not yet waited for.
///
/// # Invariant
/// It is not a token and does not outlive the run: a graph run submits and waits inside one
/// call, so there is no boundary for "still in flight". [`Self::outputs`] exists so the next
/// node can be recorded against this one's buffers, and those ids' contents the device may
/// not have written yet — not for fetching.
pub trait Pending: Send {
    /// The buffers this submission will produce, before it has finished.
    fn outputs(&self) -> &[ResidentId];

    /// Wait for the submission, release what it held, and hand the ids over.
    fn wait(self: Box<Self>) -> Result<Vec<ResidentId>, String>;
}

/// A submission already finished when it was handed back.
struct Waited {
    ids: Vec<ResidentId>,
}

impl Pending for Waited {
    fn outputs(&self) -> &[ResidentId] {
        &self.ids
    }

    fn wait(self: Box<Self>) -> Result<Vec<ResidentId>, String> {
        Ok(self.ids)
    }
}

/// One launch set: the fragments a dispatch emits together, and the runtime
/// scalars it fixes.
///
/// # Invariant
/// `ordered[0]` is the root and `index` covers every fragment: a [`KernelInstr::CallKernel`]
/// resolves to a position, so both spellings of a run read the same two facts. The leaves
/// travel **with** the set rather than beside it, and are owned rather than borrowed
/// because one set is built per node, from a leaf list that node's own plan produced.
#[derive(Debug, Clone)]
pub struct LaunchSet<'a> {
    ordered: Vec<&'a KernelFragment>,
    index: HashMap<KernelId, u32>,
    leaves: Vec<ScalarLeaf>,
}

impl<'a> LaunchSet<'a> {
    /// The set a caller already has: the fragments and their positions.
    pub fn new(ordered: &'a [KernelFragment], index: &HashMap<KernelId, u32>) -> Self {
        Self {
            ordered: ordered.iter().collect(),
            index: index.clone(),
            leaves: Vec::new(),
        }
    }

    /// The degenerate one-fragment set — one kernel, no callee, no position to
    /// resolve.
    pub fn single(fragment: &'a KernelFragment) -> Self {
        Self {
            ordered: vec![fragment],
            index: HashMap::new(),
            leaves: Vec::new(),
        }
    }

    /// This set, with the scalar leaves the launch fixes beside the extent.
    ///
    /// # Invariant
    /// One list, in [`KernelFragment::runtime_scalars`] order and that long: a backend reads a
    /// leaf at the position the parameter declares, so a list of another length would place a value
    /// under a leaf the body never names. The extent is **not** here — it is the count every run is
    /// already handed — and stating it twice would be two arguments that could disagree.
    pub fn with_leaves(mut self, leaves: Vec<ScalarLeaf>) -> Self {
        self.leaves = leaves;
        self
    }

    /// The fragments, root first.
    pub fn ordered(&self) -> &[&'a KernelFragment] {
        &self.ordered
    }

    /// The launch's runtime scalars: the root's parameter leaves beside the extent.
    pub fn leaves(&self) -> &[ScalarLeaf] {
        &self.leaves
    }

    /// Each callee [`KernelId`]'s position in [`Self::ordered`].
    pub fn index(&self) -> &HashMap<KernelId, u32> {
        &self.index
    }

    /// The fragment that runs over the index range: `ordered[0]`.
    ///
    /// # Invariant
    /// It exists, because a set is only ever built from a root — an empty one has
    /// nothing to dispatch, and a caller that has one has already gone wrong.
    pub fn root(&self) -> &'a KernelFragment {
        self.ordered
            .first()
            .copied()
            .expect("a launch set is built from a root, so it holds at least one fragment")
    }

    /// The fragment `kernel` names, and where it sits, or `None` when the set does
    /// not hold it.
    pub fn callee(&self, kernel: KernelId) -> Option<(usize, &'a KernelFragment)> {
        let at = *self.index.get(&kernel)? as usize;
        let fragment = *self.ordered.get(at)?;
        Some((at, fragment))
    }
}

/// What one dispatch inside a batch reads.
///
/// # Invariant
/// Three arms and no middle, because a batch is recorded before any of it has run: an
/// earlier dispatch's output has no id yet, so it is named by where it will be
/// ([`Self::Produced`]) and only a buffer already held is named by id. A [`Self::Host`] arm
/// is uploaded as the batch is recorded, so a whole graph costs one copy per host input
/// and none per device one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BatchInput<'a> {
    /// Data the host already holds: `count` elements, read as the class the fragment
    /// declares for this position.
    Host(&'a [u8]),
    /// A buffer this backend is already holding, from an earlier run.
    Resident(ResidentId),
    /// The `ordinal`-th output of the `dispatch`-th dispatch before this one.
    Produced { dispatch: usize, ordinal: usize },
}

/// Where one dispatch inside a batch gets its index range from.
///
/// # Invariant
/// The arms differ in where the number lives and nowhere else, as [`DeviceExtent`]'s do,
/// and every one of them allocates [`Self::allocated`] elements. A device-held count is
/// named the way its buffer is — by id when the backend already holds it, by position when
/// this batch produces it — because an output has no id until it is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchExtent {
    /// `[0, count)`, a number the host holds.
    Host(usize),
    /// `[0, device)`, read out of element `index` of a buffer this backend already holds.
    Resident {
        count: ResidentId,
        index: usize,
        capacity: usize,
    },
    /// `[0, device)`, read out of element `index` of an earlier dispatch's output.
    Produced {
        dispatch: usize,
        ordinal: usize,
        index: usize,
        capacity: usize,
    },
}

impl BatchExtent {
    /// How many elements a dispatch over this extent is allocated for.
    ///
    /// # Invariant
    /// The device arms' `capacity`, for the reason [`DeviceExtent::capacity`] gives: the
    /// returned ids' count means this, so a fetch cannot be handed a length the allocation
    /// does not cover.
    pub fn allocated(self) -> usize {
        match self {
            BatchExtent::Host(count) => count,
            BatchExtent::Resident { capacity, .. } | BatchExtent::Produced { capacity, .. } => {
                capacity
            }
        }
    }
}

/// One dispatch of a batch: what it launches, over how much, reading what.
///
/// # Invariant
/// A batch is handed the graph's own shape and not a chain's: an input is bytes, a buffer
/// already held, or the output of an earlier dispatch, so a fan-out and a fan-in are the
/// same thing to record — which a chain cannot say, since it has one input and one output
/// per link.
#[derive(Debug, Clone)]
pub struct Dispatch<'a> {
    /// The fragments this dispatch launches, root first.
    pub launch: &'a LaunchSet<'a>,
    /// One input per read position of [`LaunchSet::root`].
    pub inputs: &'a [BatchInput<'a>],
    /// The index range this dispatch covers.
    pub extent: BatchExtent,
}

/// Why a batch did not run.
///
/// # Invariant
/// Two kinds, and which one it is is the caller's: [`Self::Cannot`] is "this backend has
/// no way to record a batch", which a caller answers by choosing another schedule, while
/// [`Self::Refused`] is this graph on this device — a different diagnosis, and the one a
/// fallback would hide behind the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchRefusal {
    /// This backend cannot record several dispatches into one submission.
    Cannot,
    /// This backend refuses this batch, and says why.
    Refused(String),
}

impl BatchRefusal {
    /// What a backend that has no such capability says.
    ///
    /// # Invariant
    /// One sentence, quoted wherever the refusal is reported, so a caller reads the same
    /// words whether they came from this contract's default or from a backend declining
    /// for the same reason.
    pub const CANNOT: &'static str =
        "this backend cannot record several dispatches into one submission";
}

impl fmt::Display for BatchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BatchRefusal::Cannot => f.write_str(Self::CANNOT),
            BatchRefusal::Refused(reason) => f.write_str(reason),
        }
    }
}

/// A backend that can run a parallel fragment over an index range.
///
/// # Invariant
/// The smallest thing a host hands over and a backend promises: given a launch set, its
/// input buffers and a count, produce one output buffer per declared output. It is handed a
/// set, because a [`KernelInstr::CallKernel`] names a kernel rather than inlining it.
/// Outputs are ids a host must [`Self::release`], and a refusal means "not here" — a host
/// falls back rather than fails.
pub trait ParallelBackend: Send + Sync {
    /// A short name for this backend, recorded in whatever diagnostic a refusal produces.
    fn name(&self) -> &'static str;

    /// Run the launch set's root over the index range `[0, count)`.
    ///
    /// # Invariant
    /// `inputs` holds one slot per read position of [`LaunchSet::root`]; a
    /// [`BufferSlot::Host`] must be at least `count` elements long, each at the class's
    /// width, and the result is one [`ResidentId`] per [`KernelFragment::outputs`], owned
    /// by the host until it [`Self::release`]s it.
    fn run(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String>;

    /// [`Self::run`], handing back a submission that is still in flight.
    ///
    /// # Invariant
    /// Defaults to `run`: a backend that cannot overlap still satisfies the contract.
    /// Requiring it would mean every stub wrote a method whose only correct body does
    /// nothing, and a caller could not tell "cannot overlap" from "not implemented yet".
    fn submit<'backend>(
        &'backend self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn Pending + 'backend>, String> {
        Ok(Box::new(Waited {
            ids: self.run(launch, inputs, count)?,
        }))
    }

    /// [`Self::run`] for a whole graph: every dispatch recorded, then one submission and
    /// one wait.
    ///
    /// # Invariant
    /// Defaults to refusing by name, because a serial fallback is a different schedule
    /// wearing this one's name and a caller would be counting a schedule it did not ask
    /// for. The answer is one id list per [`Dispatch`] and in that order, every buffer
    /// settled when this returns, so the result says nothing a caller must wait for.
    fn run_batch(&self, _batch: &[Dispatch<'_>]) -> Result<Vec<Vec<ResidentId>>, BatchRefusal> {
        Err(BatchRefusal::Cannot)
    }

    /// [`Self::run`], with the index range read from a number the device already holds.
    ///
    /// # Invariant
    /// Defaults to reading the number and calling [`Self::run`]: a backend that cannot dispatch
    /// over a device-held extent still satisfies the contract, and requiring the entry would
    /// leave a caller unable to tell "cannot" from "not implemented yet". Only an override pays
    /// nothing for it — the default's read is the stall this exists to avoid.
    fn run_over_device_extent(
        &self,
        launch: &LaunchSet<'_>,
        inputs: &[BufferSlot],
        extent: DeviceExtent,
    ) -> Result<Vec<ResidentId>, String> {
        let read = extent.index + 1;
        let number = self
            .fetch(extent.count, read)?
            .as_ints()
            .and_then(|elements| elements.get(extent.index))
            .copied()
            .ok_or_else(|| {
                format!(
                    "buffer {} holds no integer element {}, so there is no number there to \
                     dispatch over",
                    extent.count.0, extent.index
                )
            })?;
        let count = usize::try_from(number).map_err(|_| {
            format!("the extent the device holds is {number}, which is not a count")
        })?;
        self.run(launch, inputs, count)
    }

    /// The first `count` elements of a buffer this backend is holding.
    ///
    /// # Invariant
    /// The point where a resident buffer crosses back to the host, and so the point that
    /// costs: a run whose results are never fetched never pays for them. The elements come
    /// back as [`ScalarData`], which is where the class is forced onto the value — an
    /// implementor has no fragment to read one off — so the buffer must have been waited
    /// for.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String>;

    /// Release a resident buffer. Idempotent on an id already released.
    fn release(&self, id: ResidentId);
}

/// The installed backend, if a host program installed one.
///
/// # Invariant
/// Process-global, like the fragment registry and the module cache: a backend that could
/// differ per module would be the odd one out, and installing twice replaces the first.
static BACKEND: OnceLock<Mutex<Option<Arc<dyn ParallelBackend>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Arc<dyn ParallelBackend>>> {
    BACKEND.get_or_init(Default::default)
}

/// Install the backend a host program wants [`parallel_backend`] to hand back.
///
/// # Invariant
/// Installing a second replaces the first, which is dropped — so a host that installs a stub
/// in one test and a real backend in the next does not accumulate them.
pub fn install_parallel_backend(backend: Arc<dyn ParallelBackend>) {
    *slot().lock().unwrap() = Some(backend);
}

/// Remove any installed backend, returning to the built-in one.
pub fn clear_parallel_backend() {
    *slot().lock().unwrap() = None;
}

/// The installed backend, or `None` when the host installed none.
///
/// # Invariant
/// `None` is not a refusal: no backend was ever in the picture, so a caller falls back
/// without a diagnostic; a backend that *declines* is the case worth recording.
pub fn parallel_backend() -> Option<Arc<dyn ParallelBackend>> {
    slot().lock().unwrap().clone()
}

/// A kernel's identity in the compiler's registry: what a cross-kernel call names.
///
/// # Invariant
/// Opaque: it is a registry slot number, and nothing about a target's representation is
/// implied by it — a backend resolves it however its own linking does.
pub type KernelId = usize;

/// The integer width a fragment was compiled for, declared rather than assumed.
///
/// # Invariant
/// A fragment states the width it needs, and a backend whose target cannot represent it
/// natively refuses the fragment by name rather than narrowing silently: the language says
/// "integer" and stops, so the width is the host compiler's choice. `I64` is the only
/// variant; the point of the type is that the decision has a name and a place to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    I64,
}

impl IntWidth {
    /// The width in bits a fragment declaring this was lowered to mean.
    pub const fn bits(self) -> u32 {
        match self {
            IntWidth::I64 => 64,
        }
    }
}

/// The lane count one row of a workgroup runs, declared rather than assumed.
///
/// # Invariant
/// A fragment states how many lanes one row of a workgroup runs, and a backend whose target
/// cannot schedule that many refuses the fragment by name rather than rounding silently: the
/// language names no lane count, so the width is the host compiler's choice, carried as a fact
/// rather than assumed from any backend's constant
/// (`docs/notes/shared-memory-and-barriers.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneWidth(u32);

impl LaneWidth {
    /// The lane count a host compiler declares for one row of a workgroup.
    pub const fn new(lanes: u32) -> Self {
        Self(lanes)
    }

    /// The lane count, as the number one row of a workgroup runs.
    pub const fn lanes(self) -> u32 {
        self.0
    }
}

/// The row count one workgroup runs, declared rather than assumed.
///
/// # Invariant
/// The second axis of [`LaneWidth`]'s declaration, and **one row is the only value downstream
/// can honour**: the launch counts lanes, `OpExecutionModeLocalSize` is emitted from one number
/// and a `Shared` slice takes one index, so a second row is scheduled as if it were the first and
/// answers a different program with no refusal (`docs/notes/two-dimensional-workgroup.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneHeight(u32);

impl LaneHeight {
    /// The row count a host compiler declares for one workgroup.
    pub const fn new(rows: u32) -> Self {
        Self(rows)
    }

    /// The row count, as the number of rows one workgroup runs.
    pub const fn rows(self) -> u32 {
        self.0
    }
}

/// The facts a fragment declares about the target it was lowered for.
///
/// # Invariant
/// Declared together because they are the same kind of fact: a choice the host compiler made,
/// carried as data rather than assumed, and refused by name by a backend whose target cannot
/// honour it. [`fragment_digest`] hashes the whole struct, so a field added here is in the digest
/// by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetFacts {
    /// The width the body's integers were lowered to mean.
    pub int_width: IntWidth,
    /// The lane count one row of a workgroup runs.
    pub lane_width: LaneWidth,
    /// The row count one workgroup runs.
    pub lane_height: LaneHeight,
}

/// A fragment's parameter domain, in the IR's own terms rather than the host's.
///
/// # Invariant
/// A kernel domain is only ever a scalar or a tuple of scalars — the compiler refuses every
/// other shape by name before lowering — so re-expressing that subset here keeps the crate
/// free of a dependency on the host IR. A leaf carries its own class, which is also what
/// makes the digest cover it for free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelShape {
    /// One scalar leaf, of the class it names.
    Scalar(ScalarClass),
    /// A tuple, flattened in order into scalar leaves.
    Tuple(Vec<KernelShape>),
}

impl KernelShape {
    /// How many scalar leaves this shape flattens to.
    ///
    /// # Invariant
    /// This is the count a backend's parameter list is built from, so a backend that
    /// flattened the shape differently would call one kernel with another's arguments. It is
    /// a function of the structure alone: a leaf's class does not change the count.
    pub fn flat_arity(&self) -> usize {
        match self {
            KernelShape::Scalar(_) => 1,
            KernelShape::Tuple(items) => items.iter().map(KernelShape::flat_arity).sum(),
        }
    }

    /// Each leaf's class, in the order [`Self::flat_arity`] counts them.
    ///
    /// # Invariant
    /// One walk, one order: a body that declared its arguments in a second order than the
    /// parameter list would read one kernel's arguments as another's, and a `Float` leaf read
    /// at another leaf's width is a wrong number rather than a failure.
    pub fn leaf_classes(&self) -> Vec<ScalarClass> {
        match self {
            KernelShape::Scalar(class) => vec![*class],
            KernelShape::Tuple(items) => items.iter().flat_map(KernelShape::leaf_classes).collect(),
        }
    }
}

/// One launch scalar: the value of a parameter's scalar leaf, at that leaf's own class.
///
/// # Invariant
/// The class is the leaf's, not the launch's: an `Int` extent and a `Float` scalar are leaves of
/// one parameter, and a list carrying a single class would read the `Float` at eight bytes. The
/// payload is one word either way — the number for `Int`, the `f32`'s bits for `Float` — because
/// that is what a buffer payload carries and a scalar must cross the boundary the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScalarLeaf {
    /// The class this leaf's parameter field declared.
    pub class: ScalarClass,
    /// The leaf's bits: the number for [`ScalarClass::Int`], the `f32`'s bits for
    /// [`ScalarClass::Float`].
    pub bits: i64,
}

/// A binary arithmetic/comparison operator of the kernel-safe subset.
///
/// # Invariant
/// The arithmetic is unsigned, because the language's `Int` is a machine-sized unsigned
/// integer and `Sub` wraps: `/`, `%` and the order comparisons are the unsigned ones. A
/// comparison yields `0`/`1`, the language having no `Bool`. `Max`/`Min` leave the operand
/// class, lowered by each backend to a comparison and a select so both answer a NaN alike.
/// See `docs/notes/operators.md` §11.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelBin {
    /// The two arithmetic families, with `Div`/`Rem` undefined on a zero divisor.
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// The larger of two values, in the operand class.
    Max,
    /// The smaller of two values, in the operand class.
    Min,
    /// The comparisons, each yielding `0`/`1`.
    Lt,
    Gt,
    Leq,
    Geq,
    Eq,
    Neq,
    /// The bitwise operators, which over `0`/`1` are the language's boolean ones.
    BitAnd,
    BitOr,
    BitXor,
}

impl KernelBin {
    /// Whether this operator yields the language's `0`/`1` scalar rather than a value of
    /// its operand class.
    ///
    /// # Invariant
    /// Stated here because both emitters narrow a comparison to a condition and neither may
    /// re-derive the list: a `Max` a backend read as a comparison would select on a value
    /// the language never made a condition.
    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            KernelBin::Lt
                | KernelBin::Gt
                | KernelBin::Leq
                | KernelBin::Geq
                | KernelBin::Eq
                | KernelBin::Neq
        )
    }
}

/// One abstract instruction in a lowered kernel body.
///
/// # Invariant
/// A fragment stores a `Vec<KernelInstr>` and not target code, so a backend resolves
/// cross-kernel calls with link-time information after the reachable set is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelInstr {
    /// Push a constant, of the class it names.
    ///
    /// # Invariant
    /// The class is per instruction, not per fragment: a body may compute in more than one,
    /// so a constant's bits are readable only against the class it was lowered in.
    Const(ScalarClass, i64),
    /// A binary [`KernelBin`] operator over two values, **in the class it names**.
    ///
    /// # Invariant
    /// Per instruction for the same reason [`Self::Const`] is: the opcode a backend
    /// emits for `Add` is `i64.add` in one class and `f32.add` in the other, and a body
    /// that mixes the two needs both.
    Bin(ScalarClass, KernelBin),
    /// `-A` over one `Float`, which SPIR-V spells `OpFNegate` and wasm `f32.neg`.
    ///
    /// # Invariant
    /// A negation, not a subtraction from zero: `0.0 - a` is `-a` for every operand except
    /// `a = +0.0`, where it answers `+0.0` while a negation answers `-0.0`. A compensated
    /// division's residual reads the sign of the divisor, so the two are not interchangeable.
    Negate,
    /// `A·B + C` over three `Float`s, rounded **once**.
    ///
    /// # Invariant
    /// **One rounding is the whole instruction**: two of them are [`Self::Bin`] twice, and on a
    /// compensated division's residual the second costs the bit that made the answer exact
    /// (`docs/notes/fused-multiply-add.md` §2).
    ///
    /// **Not [`crate::TRANSCENDENTALS`]**, which is what the IR defines and inlines: this one is
    /// native where it exists at all, and wasm's MVP has none.
    Fma,
    /// Convert a value to the condition width a `select` needs.
    I32WrapI64,
    /// A `if c then a else b`, which is `Select`.
    Select,
    /// The exponential of one `Float`.
    ///
    /// # Invariant
    /// No target has a native form the two read alike — wasm has none, and SPIR-V's GLSL
    /// `Exp` is implementation-defined per vendor — so the IR **defines** it and every
    /// backend lowers it by [`KernelBody::expand_transcendentals`]: one primitive sequence,
    /// so the two compute the identical numbers. `Float` is the class, and the only one.
    /// See `docs/notes/floating-point.md` §6.
    Exp,
    /// The square root of one `Float`.
    ///
    /// # Invariant
    /// Defined by the IR rather than emitted by a target, like [`Self::Exp`]: wasm's
    /// `f32.sqrt` and SPIR-V's `OpSqrt` are both correctly rounded, but a definition in the
    /// primitives is what makes the two answer the same bits *and* what the interpreter can
    /// read. See `docs/notes/floating-point.md` §7.
    Sqrt,
    /// The largest integral `Float` not above one `Float`.
    ///
    /// # Invariant
    /// Defined by the IR, like [`Self::Exp`] and [`Self::Sqrt`]: SPIR-V has no core
    /// `OpFloor`, so a native emission would mean two instruction sets rather than one
    /// operation both targets agree on. The definition is exact for every operand in the
    /// class, because the conversion and the subtraction it is written with are.
    /// `docs/notes/floating-point.md` §7.
    Floor,
    /// The natural logarithm of one `Float`.
    ///
    /// # Invariant
    /// Defined by the IR, like [`Self::Exp`]: no target has a logarithm the two read
    /// alike, so every backend lowers it by [`KernelBody::expand_transcendentals`]. The
    /// reduction needs no shift and no bitcast — it is the fold [`Self::Sqrt`] uses.
    ///
    /// It **divides once**, and `OpFDiv` misrounds here, so it carries a cross-backend
    /// divergence of up to a unit in the last place (§4.4).
    Ln,
    /// The sine of one `Float`.
    ///
    /// # Invariant
    /// Defined by the IR and named in [`crate::TRANSCENDENTALS`] like [`Self::Exp`], so
    /// every backend lowers it by [`KernelBody::expand_transcendentals`]. It used to carry
    /// no instruction of its own and to be spliced by `lichen-compute` instead; the two
    /// mechanisms now are one.
    Sin,
    /// The cosine of one `Float`.
    ///
    /// # Invariant
    /// As [`Self::Sin`]'s.
    Cos,
    /// A cross-kernel call: its `args` are the argument, and a backend resolves this to
    /// a call to the callee.
    CallKernel(KernelId),
    /// The language's two class conversions as one instruction: it reads one class and
    /// leaves the other.
    ///
    /// # Invariant
    /// *Which* of the two cannot be read off the operand — `Int → Float` and `Float →
    /// Int` are the same shape — and the direction is the language's decision, not a
    /// target's, so a backend that re-derived it would guess, and the two could guess
    /// differently.
    Conv {
        /// The class the operand is, as the language names it.
        from: ScalarClass,
        /// The class this leaves behind, as the language names it.
        to: ScalarClass,
    },
    /// Read one element of one input buffer: the stack holds `[position, index]`.
    ///
    /// # Invariant
    /// The value pushed is of the class named here — the buffer's element class — while the
    /// position and the index are `Int` regardless: one is a compile-time ordinal, the other
    /// a lane number. So this instruction names two classes at once.
    BufferReadCall(ScalarClass),
    /// Write one element of one output buffer: `[position, index, value]`.
    ///
    /// # Invariant
    /// The class is the element class, as in [`Self::BufferReadCall`]; the position and the
    /// index are `Int`.
    BufferWriteCall(ScalarClass),
    /// Write one element of one slice: `[slice, index, value]`.
    ///
    /// # Invariant
    /// The class is the element class, as in [`Self::BufferWriteCall`]; `slice` is a compile-time
    /// ordinal into [`KernelFragment::slices`] and `index` is `Int`. What it **leaves** is
    /// the version a later [`Self::SliceReadCall`] names — the edge a barrier is derived from —
    /// so it produces one value (`docs/notes/shared-memory-and-barriers.md` §3).
    SliceWriteCall(ScalarClass),
    /// Read one element of one slice: `[slice, index, version]`.
    ///
    /// # Invariant
    /// `slice` and `index` are as in [`Self::SliceWriteCall`]. The third operand is the
    /// [`Self::SliceWriteCall`] whose result this read observes, so a pair says *which* write to
    /// *which* slice it is ordered after rather than "after something"
    /// (`docs/notes/shared-memory-and-barriers.md` §3).
    SliceReadCall(ScalarClass),
}

/// The role of every field of a parallel kernel's parameter struct.
///
/// # Invariant
/// A field's role is a fact of where it sits, not of what it holds: `.in` holds input
/// buffers, `.out` output buffers, both at any depth, and anything else is a runtime scalar.
/// A path's index in `inputs`, `outputs` or `scalars` is the position the host import or the
/// wasm local takes, all in field order, so the emitter and the run cannot disagree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KernelRoles {
    /// Scalar parameter paths, in the order they become wasm locals.
    pub scalars: Vec<Vec<usize>>,
    /// Input buffer payload paths, in declaration order.
    pub inputs: Vec<Vec<usize>>,
    /// Output buffer payload paths, in declaration order.
    pub outputs: Vec<Vec<usize>>,
    /// The class each input buffer's parameter states, per [`Self::inputs`], or none.
    ///
    /// # Invariant
    /// A stated class is the `T` the parameter's own `Buf T` names, and it is the authority
    /// over the body's classes: a `Float` buffer whose write was a `select` was read as `Int`
    /// otherwise, and the run refused the buffer it was given. `None` is *not stated*, not
    /// `Int`: the older open element cell leaves the write's own class the answer.
    pub input_classes: Vec<Option<ScalarClass>>,
    /// The class each output buffer's parameter states, per [`Self::outputs`], or none.
    pub output_classes: Vec<Option<ScalarClass>>,
}

impl KernelRoles {
    /// The ordinal of `path` among the input buffers — the position a read of it
    /// takes.
    pub fn input_pos(&self, path: &[usize]) -> Option<usize> {
        self.inputs.iter().position(|candidate| candidate == path)
    }

    /// The ordinal of `path` among the output buffers — the position a write of it
    /// fills.
    ///
    /// # Invariant
    /// The write's counterpart to [`Self::input_pos`], and the reason a write's target
    /// is a function of its operands rather than of the order it was emitted in.
    pub fn output_pos(&self, path: &[usize]) -> Option<usize> {
        self.outputs.iter().position(|candidate| candidate == path)
    }

    /// The wasm local offset of a scalar parameter read, within its slot.
    pub fn scalar_offset(&self, path: &[usize]) -> Option<usize> {
        self.scalars.iter().position(|candidate| candidate == path)
    }
}

impl KernelInstr {
    /// How many values this instruction leaves behind.
    ///
    /// # Invariant
    /// **One for everything except [`KernelInstr::BufferWriteCall`]**, the side effect that leaves
    /// nothing: a slice write leaves the version its reader consumes. The whole of the arity
    /// question, so [`KernelBody::validate`](crate::KernelBody::validate) can check a definition
    /// against its declaration.
    pub fn produces(&self) -> usize {
        match self {
            KernelInstr::BufferWriteCall(_) => 0,
            _ => 1,
        }
    }

    /// How many values this instruction reads, when the instruction fixes it.
    ///
    /// # Invariant
    /// **Stated here because it is a fact about the language's operators, not about any
    /// target.** `None` means *not fixed by the instruction*, and is
    /// [`Self::CallKernel`], whose arity is the **callee's own domain**: this crate does
    /// not know it, so a body states its arguments and the check that could contradict it
    /// belongs to whoever holds the callee.
    pub fn arity(&self) -> Option<usize> {
        Some(match self {
            KernelInstr::Const(..) => 0,
            KernelInstr::Bin(_, _) | KernelInstr::BufferReadCall(_) => 2,
            KernelInstr::I32WrapI64
            | KernelInstr::Conv { .. }
            | KernelInstr::Negate
            | KernelInstr::Exp
            | KernelInstr::Sqrt
            | KernelInstr::Floor
            | KernelInstr::Ln
            | KernelInstr::Sin
            | KernelInstr::Cos => 1,
            KernelInstr::Fma | KernelInstr::Select => 3,
            KernelInstr::BufferWriteCall(_) => 3,
            KernelInstr::SliceReadCall(_) | KernelInstr::SliceWriteCall(_) => 3,
            KernelInstr::CallKernel(_) => return None,
        })
    }

    /// The class this instruction states in its own form, if it states one.
    ///
    /// # Invariant
    /// **Stated, not derived**: a constant's bits and an arithmetic operator's operands are
    /// only readable against the class they were lowered in, so an instruction names its
    /// class because the fact cannot be recovered from the value later.
    pub fn own_class(&self) -> Option<ScalarClass> {
        match self {
            KernelInstr::Const(class, _) => Some(*class),
            KernelInstr::Bin(class, _) => Some(*class),
            KernelInstr::Conv { to, .. } => Some(*to),
            KernelInstr::Exp
            | KernelInstr::Sqrt
            | KernelInstr::Floor
            | KernelInstr::Ln
            | KernelInstr::Sin
            | KernelInstr::Cos => Some(ScalarClass::Float),
            KernelInstr::Negate => Some(ScalarClass::Float),
            KernelInstr::Fma => Some(ScalarClass::Float),
            KernelInstr::BufferReadCall(class) | KernelInstr::BufferWriteCall(class) => {
                Some(*class)
            }
            KernelInstr::SliceReadCall(class) | KernelInstr::SliceWriteCall(class) => Some(*class),
            KernelInstr::I32WrapI64 | KernelInstr::Select | KernelInstr::CallKernel(_) => None,
        }
    }
}

/// Where a slice's elements live, as the plan names the choice.
///
/// # Invariant
/// The three spaces a plan admits (`docs/notes/graph-fusion.md` §5), and the vocabulary
/// [`Slice::space`] states: a backend that cannot honour one refuses it **by name** rather than
/// emitting the other. `Global` is not a member: a value two workgroups must exchange means the
/// region did not fuse — the graph's decision, not a slice's home
/// (`docs/notes/shared-memory-and-barriers.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Space {
    /// One array per lane, readable by the lane that wrote it and no other.
    Register,
    /// One value per lane, readable by any lane of the writing lane's subgroup.
    ///
    /// # Invariant
    /// Element `i` is staged by the lane whose subgroup-local id is `i`, which is the lane a read's
    /// index names as the one it exchanges from; a backend narrows the slice to its target's
    /// subgroup width and refuses a wider one **by name** rather than exchanging it in shared
    /// memory (`docs/notes/shared-memory-and-barriers.md` §5).
    Warp,
    /// One array per workgroup, readable by every lane of it.
    Shared,
}

/// One memory slice a fragment declares: the class of its elements, how many it holds, and where
/// they live.
///
/// # Invariant
/// `count` is the length of the array the emitter declares in `space`, and an index that may reach
/// it is refused ([`KernelFragment::barrier_scan`]); a target that cannot honour `space` refuses the
/// fragment by name. The fragment derives the count from `target.lane_width` while the surface that
/// names one is unbuilt, so the bound and the declared array are one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Slice {
    /// The class of one element of the slice.
    pub class: ScalarClass,
    /// How many elements the slice holds.
    pub count: u32,
    /// Where the elements live.
    pub space: Space,
}

/// A compiled kernel-callable unit: a lowered body plus the facts a caller needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelFragment {
    /// The parameter domain, flattened by [`KernelShape::flat_arity`] into the
    /// backend's parameter list.
    pub param_shape: KernelShape,
    /// Where the parameter's leaves sit in the value a caller hands over, as index paths.
    pub roles: KernelRoles,
    /// The lowered body, as structured control flow.
    ///
    /// # Invariant
    /// A backend must call [`KernelBody::validate`] before reading it: that is what lets a
    /// transfer be added ahead of the backends that emit it, so one that has not learned it
    /// is refused by name rather than quietly dropping a branch.
    pub body: KernelBody,
    /// How many input buffers this fragment reads, one past the highest read position.
    ///
    /// # Invariant
    /// A property of the compiled fragment, so a caller knows how many buffers to hand over.
    /// It is not in `param_shape` and cannot be: a parallel fragment's shape is `(config,
    /// index)` however many buffers it reads, because they are bound storage buffers. How
    /// many a caller is given is the dispatch's; how many is needed is this.
    pub inputs: usize,
    /// How many output buffers this fragment writes: the codomain's arity.
    ///
    /// # Invariant
    /// A property of the compiled fragment, so a caller allocates exactly this many and never
    /// discovers at run time which were written.
    pub outputs: usize,
    /// The element class of each input buffer, one entry per read position, in order.
    ///
    /// # Invariant
    /// The length is the declared `inputs`, including positions a body never read, and this
    /// is the list a host slot is matched against — a slot carries no class of its own. A
    /// parallel fragment's shape is `(config, index)` whatever its buffers' classes, so a
    /// class that lived only on the parameter leaves could say nothing about a buffer.
    pub input_classes: Vec<ScalarClass>,
    /// The element class of each output buffer, one entry per write ordinal.
    ///
    /// # Invariant
    /// The length is `outputs`, and element `k` is the class of the buffer write `k` filled —
    /// the same compile-time constant the write call is fed. A scalar fragment's results are
    /// in `result_classes` instead, because a write ordinal and a wasm result are positions
    /// in different spaces.
    pub output_classes: Vec<ScalarClass>,
    /// The class of each value the body leaves on the stack, one entry per result.
    ///
    /// # Invariant
    /// A count alone is not enough: the emitted result list is typed per position, so a body
    /// returning `(Int, Float)` and one returning `(Float, Int)` have one arity and different
    /// signatures, and typing both from one class would emit a module that cannot validate.
    pub result_classes: Vec<ScalarClass>,
    /// The target facts the fragment declares: what a backend is asked to honour.
    ///
    /// # Invariant
    /// They travel together because they are one kind of fact — a host compiler's choice this
    /// crate carries as data rather than assumes — and [`fragment_digest`] hashes the whole
    /// struct, so a fact added here is in the digest by construction.
    pub target: TargetFacts,
    /// Each slice this body addresses, one entry per ordinal.
    ///
    /// # Invariant
    /// The length is the slice set: a [`KernelInstr::SliceWriteCall`]'s first operand is an ordinal
    /// into it, as a buffer write's position is into `outputs`. It is not a target fact — the
    /// slices are the program's own memory — and [`fragment_digest`] hashes it, so two fragments
    /// differing only in a slice's space are two modules.
    pub slices: Vec<Slice>,
    /// The marks expressions in the body carried, in the order the walk published them.
    ///
    /// # Invariant
    /// **Metadata, and additive**: no backend reads it, because a mark says what a value
    /// *means* rather than what the kernel computes. A consumer that needs the marks reads
    /// them itself and refuses by name when the role it needs is not there.
    pub marks: Vec<KernelMark>,
}

impl KernelFragment {
    /// How many scalar leaves a launch fixes for this fragment: its domain's leaves but the
    /// index.
    ///
    /// # Invariant
    /// The index is the last leaf and is not a launch value — on a device it is the invocation a
    /// lane is, and on the host the worker's loop variable — so a launch supplies exactly this
    /// many. A backend that pushed a different number would place one leaf's value under another.
    pub fn scalar_leaves(&self) -> usize {
        self.param_shape.flat_arity().saturating_sub(1)
    }

    /// How many runtime scalars a launch fixes for this fragment: its leaves but the extent
    /// and the index.
    ///
    /// # Invariant
    /// **The extent is not a runtime scalar.** It is the count the launch dispatches over, which
    /// every backend already holds, and the ABI's first scalar by definition — so a launch states
    /// it once, as the count, rather than twice.
    pub fn runtime_scalars(&self) -> usize {
        self.scalar_leaves().saturating_sub(1)
    }
}

/// A fragment's content digest: what makes a [`KernelId`] an identity.
///
/// # Invariant
/// Every field is hashed, and a field left out is a way for two fragments that differ in it
/// to share one identity — a cache then serves one kernel's compiled form for another's,
/// silently. `marks` is hashed for the same reason: what a value means is part of the
/// artifact. The **claim** is not a fragment field: the registry interns the two together.
pub fn fragment_digest(fragment: &KernelFragment) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", fragment.param_shape).hash(&mut hasher);
    format!("{:?}", fragment.roles).hash(&mut hasher);
    format!("{:?}", fragment.body).hash(&mut hasher);
    fragment.inputs.hash(&mut hasher);
    fragment.outputs.hash(&mut hasher);
    format!("{:?}", fragment.input_classes).hash(&mut hasher);
    format!("{:?}", fragment.output_classes).hash(&mut hasher);
    fragment.result_classes.hash(&mut hasher);
    format!("{:?}", fragment.target).hash(&mut hasher);
    format!("{:?}", fragment.slices).hash(&mut hasher);
    format!("{:?}", fragment.marks).hash(&mut hasher);
    hasher.finish()
}

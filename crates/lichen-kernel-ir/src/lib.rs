//! The lowered-kernel IR and the backend contract that consumes it.
//! See docs/notes/compute-jit-low-types.md.
//!
//! # Invariant
//! The IR is narrower than any target's instruction set, leaving calling convention,
//! storage layout and how a buffer reference is obtained to a backend; the body is
//! structured control flow, because a loop needs a backedge and a value that survives it.
//! The crate depends on nothing, so a backend inherits no other's runtime.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

mod body;

pub use body::{BasicBlock, Br, FlatOp, KernelBody, Terminator, ValueDef, ValueId, from_flat};

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
        }
    }

    /// A dense index for this class, so a backend can key a per-class table on it.
    ///
    /// # Invariant
    /// The two are in step with the two variants: a module holding one element type per
    /// class — an SPIR-V storage buffer, whose `ArrayStride` is the class's
    /// `byte_width()` — sizes its table by the number of classes, not by a number
    /// restated beside it.
    pub const ALL: [ScalarClass; 2] = [ScalarClass::Int, ScalarClass::Float];

    pub fn index(self) -> usize {
        match self {
            ScalarClass::Int => 0,
            ScalarClass::Float => 1,
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
/// travel **with** the set rather than beside it: a dispatch reads its values from the launch
/// it was handed, and a second place they could be changed in is a way to run one value as
/// another.
#[derive(Debug, Clone)]
pub struct LaunchSet<'a> {
    ordered: Vec<&'a KernelFragment>,
    index: HashMap<KernelId, u32>,
    leaves: &'a [ScalarLeaf],
}

impl<'a> LaunchSet<'a> {
    /// The set a caller already has: the fragments and their positions.
    pub fn new(ordered: &'a [KernelFragment], index: &HashMap<KernelId, u32>) -> Self {
        Self {
            ordered: ordered.iter().collect(),
            index: index.clone(),
            leaves: &[],
        }
    }

    /// The degenerate one-fragment set — one kernel, no callee, no position to
    /// resolve.
    pub fn single(fragment: &'a KernelFragment) -> Self {
        Self {
            ordered: vec![fragment],
            index: HashMap::new(),
            leaves: &[],
        }
    }

    /// This set, with the scalar leaves the launch fixes beside the extent.
    ///
    /// # Invariant
    /// One list, in [`KernelFragment::runtime_scalars`] order and that long: a backend reads a
    /// leaf at the position the parameter declares, so a list of another length would place a value
    /// under a leaf the body never names. The extent is **not** here — it is the count every run is
    /// already handed — and stating it twice would be two arguments that could disagree.
    pub fn with_leaves(mut self, leaves: &'a [ScalarLeaf]) -> Self {
        self.leaves = leaves;
        self
    }

    /// The fragments, root first.
    pub fn ordered(&self) -> &[&'a KernelFragment] {
        &self.ordered
    }

    /// The launch's runtime scalars: the root's parameter leaves beside the extent.
    pub fn leaves(&self) -> &[ScalarLeaf] {
        self.leaves
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
/// integer and `Sub` wraps: `/` and `%` are the unsigned operations and so are the order
/// comparisons. A comparison yields a `0`/`1` scalar, not a boolean — the language has no
/// `Bool` — so materialising it, and narrowing it for a `select`, are the backend's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelBin {
    /// The two arithmetic families, with `Div`/`Rem` undefined on a zero divisor.
    Add,
    Sub,
    Mul,
    Div,
    Rem,
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
    /// Convert a value to the condition width a `select` needs.
    I32WrapI64,
    /// A `if c then a else b`, which is `Select`.
    Select,
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
}

impl KernelRoles {
    /// The ordinal of `path` among the input buffers — the position a read of it
    /// takes.
    pub fn input_pos(&self, path: &[usize]) -> Option<usize> {
        self.inputs.iter().position(|candidate| candidate == path)
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
    /// **One for everything except a write**, a side effect that leaves nothing — the
    /// whole of the arity question, so [`KernelBody::validate`](crate::KernelBody::validate)
    /// can check a definition against its declaration.
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
            KernelInstr::I32WrapI64 | KernelInstr::Conv { .. } => 1,
            KernelInstr::Select => 3,
            KernelInstr::BufferWriteCall(_) => 3,
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
            KernelInstr::BufferReadCall(class) | KernelInstr::BufferWriteCall(class) => {
                Some(*class)
            }
            KernelInstr::I32WrapI64 | KernelInstr::Select | KernelInstr::CallKernel(_) => None,
        }
    }
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
    /// The integer width the body was lowered to mean.
    pub int_width: IntWidth,
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
/// to share one identity — a cache keyed on that identity then serves one kernel's compiled
/// form for another's, silently. `results` is the sharpest case: it types the result list,
/// so a two-result and a three-result fragment must not collapse.
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
    format!("{:?}", fragment.int_width).hash(&mut hasher);
    hasher.finish()
}

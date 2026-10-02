//! The lowered-kernel IR: what a lichen function becomes *before* any machine
//! code is emitted for it.
//!
//! # The split
//!
//! A kernel reaches machine code in two stages, and only the second one is
//! target-specific:
//!
//! 1. **Lower** — walk the checked lichen graph and produce a
//!    [`KernelFragment`]: a domain shape, a body of [`KernelInstr`], and the
//!    two arities a caller needs.  This stage needs the host IR, because it
//!    reads it.
//! 2. **Emit** — turn a fragment into a module for some target.  This stage
//!    needs the fragment and nothing else.
//!
//! Stage 2 is what has more than one plausible implementation (the wasm
//! backend in `lichen-compute` is the first), and it is the stage that must not
//! be forced to inherit stage 1's dependencies. So stage 2's input lives here,
//! in a crate that depends on nothing.
//!
//! # What this IR deliberately does not carry
//!
//! The lowered body is a small SSA-style instruction list, deliberately
//! *narrower* than any target's own instruction set — it names only operations
//! every plausible target can express, and leaves the encoding decisions
//! (calling convention, storage layout, how a buffer reference is obtained) to
//! the backend. A `BufferReadCall` here is "read one element of one input
//! buffer"; how that becomes a host call, a memory load, or something else is
//! not in the IR.
//!
//! See `docs/notes/compute-jit-low-types.md` for how the domain half of a
//! fragment is decided, and `docs/notes/lichen-compute.md` for the wasm backend
//! that consumes it.
//!
//! # The backend contract
//!
//! The data above is what a compiler hands a backend. [`ParallelBackend`] is what
//! the two sides *agree on*, and the process-global slot below is where a host
//! program installs the one it wants. Both live here because this is the only
//! crate both can depend on without either pulling in the other's runtime: a
//! backend contract that lived in a backend's crate would make every other
//! backend depend on that backend.

use std::sync::{Arc, Mutex, OnceLock};

/// Which scalar class a value, a parameter leaf or a buffer element is.
///
/// # Why this is beside [`IntWidth`] rather than inside it
///
/// The two answer different questions and only one of them has a width. An
/// `Int`'s width is a compiler decision `IntWidth` names; a `Float`'s width is
/// `f32` and is fixed, so a single variant space covering both would give
/// `bits()` two ways to answer `32` and no way to tell which class it was
/// describing — and every `bits() != 64` check in a backend would then accept a
/// float fragment as an integer one (`docs/notes/floating-point.md` §4.4).
///
/// # Fieldless on purpose
///
/// The class is a tag, and the payload's representation belongs to whoever
/// holds the payload ([`ScalarData`] for a fetched buffer). A fieldless enum
/// derives `Eq`, which the value-carrying carriers beside it need: a resident
/// buffer's class travels in a struct that *is* `Eq` because a device buffer's
/// identity and count are, and an `f32` payload is not `Eq` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarClass {
    /// An unsigned machine-sized integer — the language's `Int`.
    Int,
    /// A 32-bit float — the language's `Float`.
    Float,
}

/// The elements of a buffer a backend has handed back, with the class they are
/// to be read as.
///
/// # Why the class rides in the value rather than in a parameter
///
/// [`ParallelBackend::fetch`] is the read-back, and its implementor has no
/// fragment to consult: the only thing the caller holds is a
/// [`ResidentId`] and the count it was issued with. So the class has to be
/// **forced onto the value** at the one point where the data crosses back —
/// which is what this type is. Its two variants make the class and the
/// representation inseparable, so a payload cannot be read as the wrong class:
/// a `Float` buffer cannot be handed over as a bare `Vec<i64>` that a reader
/// would take for integers.
///
/// # The bits, not a conversion
///
/// The payload is the buffer's bytes, and the variant says how to read them.
/// No conversion happens here: this is the ABI's carrier, not a lowerer. The
/// one conversion the design has is a **boundary** conversion a lowering
/// chooses later, at a place nothing above the backend decided
/// (`docs/notes/floating-point.md` §4.3).
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
    /// A total accessor rather than an `unwrap`: a caller that has already
    /// matched the class would otherwise be re-deciding it, and one that has
    /// not gets to name the mismatch instead of panicking.
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
/// **Opaque on purpose, and backend-scoped:** this is whatever the backend calls
/// the buffer, so an id means nothing without the backend that issued it and must
/// never be compared across backends or persisted. Passing one to a backend other
/// than the issuer is a caller error, not a lookup that misses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResidentId(pub u64);

/// One input buffer as it crosses the backend boundary.
///
/// The point of the two forms is that **data the backend already holds costs
/// nothing to use again**: handing back a [`Self::Resident`] instead of its
/// contents is what lets a chain of kernels run without the intermediate results
/// ever reaching the host.
///
/// # The class of a slot is the fragment's, not the slot's
///
/// A host slot is a raw bit payload, and the class it is to be read as is a fact
/// of the *dispatch*: [`KernelFragment::input_classes`] says what each position
/// holds, and a caller that agrees with the fragment it named has said
/// everything there is to say about the slot.  So this type carries no class of
/// its own, and it stays `Eq`: a typed `&[f32]` variant would duplicate the
/// fragment's list, and would put a payload that is not `Eq` into a type whose
/// whole use is naming a buffer cheaply.  A resident slot's class is the
/// resident value's, where it is genuinely per-buffer and not derivable from an
/// ordinal — see `ResidentBuffer` in `lichen-compute`.
///
/// **That is a deferral, not a closed question**, because a `Host` slot is a
/// slice of the ABI's own element type rather than of [`ScalarData`]: when a
/// float producer exists its elements have to reach [`ParallelBackend::run`]
/// somehow, and there are exactly two routes — the host reinterprets the `f32`
/// bytes as `i64` words, or this type gains a class-carrying variant after all.
/// The reinterpretation is sound only while nothing reads two slots as one
/// value, and it is worth naming what it costs: the slot's `len()` would then
/// count `i64` words while the fragment's count counts `f32` elements, so the
/// two numbers a caller compares are no longer the same quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferSlot<'a> {
    /// Data the host already holds, at least `count` elements long, read as the
    /// class [`KernelFragment::input_classes`] declares for this position.
    Host(&'a [i64]),
    /// A buffer the backend is already holding, from an earlier [`ParallelBackend::run`].
    Resident(ResidentId),
}

/// A submission a backend has taken but not yet waited for.
///
/// [`ParallelBackend::submit`] hands one of these back rather than a
/// `Vec<ResidentId>`, so the host can do something else while the device is
/// still working. It is the only thing that can express that, because it is the
/// only thing that knows which of the backend's own resources the submission is
/// still holding.
///
/// # It is not a token, and it does not outlive the run
///
/// It is neither a bare number nor something a host should store. A graph run is
/// one operation that returns when the run is finished, so a submission made
/// inside it is submitted and waited for inside the same call — there is no
/// boundary for "still in flight" to cross. An implementation is free to be
/// `Drop`-with-wait as a backstop, and the GPU one is, because a dropped
/// submission is a bug rather than a shape to support.
///
/// # Outputs are readable only after the wait
///
/// [`Self::outputs`] exists so the next node can be recorded against this
/// one's buffers, and it returns ids whose **contents the device may not have
/// written yet**. They are not for fetching. A demand point — a place where the
/// host needs the data — is [`Self::wait`] followed by an ordinary fetch of the
/// ids it returns, and waiting is what makes that fetch sound.
pub trait Pending: Send {
    /// The buffers this submission will produce, before it has finished.
    fn outputs(&self) -> &[ResidentId];

    /// Wait for the submission, release whatever it held, and hand the ids over.
    ///
    /// After this the ids mean what they mean anywhere else: the device has
    /// written them and they can be fetched or fed to another run.
    fn wait(self: Box<Self>) -> Result<Vec<ResidentId>, String>;
}

/// A submission that was already finished when it was handed back.
///
/// What a backend that cannot overlap anything produces, and what every backend
/// produces for a submission it chose not to overlap.
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

/// A backend that can run a parallel fragment over an index range.
///
/// This is deliberately *not* the shape of a compiled module, a memory pool or a
/// device queue. It is the smallest thing a host program has to hand over, and
/// the smallest thing a backend has to promise: given a fragment, its input
/// buffers and a count, produce one output buffer per declared output.
///
/// # Outputs are ids, not data
///
/// [`Self::run`] returns [`ResidentId`]s rather than the results, and
/// [`Self::fetch`] is the only way to get the data back. A backend that cannot
/// keep a result would have to hand it over eagerly, and the boundary would
/// force the round trip on every run — the round trip being the thing worth
/// avoiding. Making the *host's* choice explicit also means a program that feeds
/// one kernel's output into the next never pays for data it never looks at.
///
/// A resident id is a resource: the host must [`Self::release`] it. There is no
/// finaliser behind an id, so a host that drops one leaks whatever the backend
/// spent on it.
///
/// A refusal is a `String` naming its own cause, and a host is expected to
/// **fall back** rather than fail: a backend is never more capable than the one
/// built into the language, so a refusal means "not here", not "impossible".
pub trait ParallelBackend: Send + Sync {
    /// A short name for this backend, recorded in whatever diagnostic a refusal
    /// produces, so a reader can tell *which* backend declined.
    fn name(&self) -> &'static str;

    /// Run `fragment` over the index range `[0, count)`.
    ///
    /// `inputs` holds one slot per read position; a [`BufferSlot::Host`] must be
    /// at least `count` long. The result is one [`ResidentId`] per
    /// [`KernelFragment::outputs`], each holding at least `count` elements, owned
    /// by the host until it [`Self::release`]s it.
    fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Vec<ResidentId>, String>;

    /// [`Self::run`], handing back a submission that is still in flight.
    ///
    /// **Defaults to `run`**, which is the point: a backend that cannot overlap
    /// anything still satisfies the contract, it just never collects from it.
    /// Requiring an implementation would mean every stub and every future
    /// backend wrote a method whose only correct body is the one that does
    /// nothing, and a caller could not tell "cannot overlap" from "has not
    /// implemented overlap yet".
    fn submit<'backend>(
        &'backend self,
        fragment: &KernelFragment,
        inputs: &[BufferSlot],
        count: usize,
    ) -> Result<Box<dyn Pending + 'backend>, String> {
        Ok(Box::new(Waited {
            ids: self.run(fragment, inputs, count)?,
        }))
    }

    /// The first `count` elements of a buffer this backend is holding.
    ///
    /// The point at which a resident buffer actually crosses back to the host,
    /// and so the point that costs: a run whose results are never fetched never
    /// pays for them.
    ///
    /// **The elements come back as [`ScalarData`], and that is where the class
    /// is forced onto the value.** An implementor has no fragment to read a
    /// class off — the caller holds a [`ResidentId`] and a count and nothing
    /// else — so the data itself has to say what it is, or a float result
    /// buffer would come back as `i64` and be read as integers.
    ///
    /// **The buffer must have been waited for.** That is automatic for anything
    /// from [`Self::run`], and for anything from [`Pending::wait`] — but not for
    /// an id read off [`Pending::outputs`] before the wait, and the difference is
    /// not a slow read: it is whatever the device happened to have written.
    fn fetch(&self, id: ResidentId, count: usize) -> Result<ScalarData, String>;

    /// Release a resident buffer. Idempotent on an id already released.
    fn release(&self, id: ResidentId);
}

/// The installed backend, if a host program installed one.
///
/// **Process-global, like the rest of compute's registries, and for the same
/// reason:** the fragment registry and the module cache are already process-wide,
/// so a backend that could differ per module would be the odd one out. Installing
/// twice replaces the first, which is what a host that composes plugins wants.
static BACKEND: OnceLock<Mutex<Option<Arc<dyn ParallelBackend>>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Arc<dyn ParallelBackend>>> {
    BACKEND.get_or_init(Default::default)
}

/// Install the backend a host program wants [`parallel_backend`] to hand back.
///
/// Installing a second backend **replaces** the first, and the replaced one is
/// dropped — so a host that installs a stub in one test and a real backend in the
/// next does not accumulate them.
pub fn install_parallel_backend(backend: Arc<dyn ParallelBackend>) {
    *slot().lock().unwrap() = Some(backend);
}

/// Remove any installed backend, returning to the built-in one.
pub fn clear_parallel_backend() {
    *slot().lock().unwrap() = None;
}

/// The installed backend, or `None` when the host installed none.
///
/// `None` is not a refusal: it means no backend was ever in the picture, so a
/// caller falls back without recording a diagnostic. A backend that *declines* is
/// the case worth recording.
pub fn parallel_backend() -> Option<Arc<dyn ParallelBackend>> {
    slot().lock().unwrap().clone()
}

/// A kernel's identity in the compiler's registry: the key a fragment is
/// interned under, and what a cross-kernel call names.
///
/// The type is deliberately opaque — it is a registry slot number, and nothing
/// about a target's representation is implied by it. A backend resolves it
/// however its own linking does.
pub type KernelId = usize;

/// The integer width a fragment was **compiled for**, declared rather than
/// assumed.
///
/// # Why this is declared at all
///
/// The lichen language says "integer" and stops there: an `Int` reaches the
/// lowered graph as a machine-sized unsigned integer, so the width that ends up
/// in a kernel is a property of the *host* the compiler ran on, not something
/// the language fixed. A fragment that silently assumed a particular width
/// would bake that host's choice into every backend, and a backend whose target
/// has a different (or absent) native width would have no way to tell that it
/// must convert at its boundary — the information would already be gone.
///
/// So a fragment states the width it needs, and a backend reads it:
/// [`bits`](IntWidth::bits) is what the body was lowered to mean, and a backend
/// whose target cannot represent that width natively must **refuse the fragment
/// by name** rather than narrow the values silently, because narrowing is a
/// semantic change to a program the author never wrote.
///
/// # One variant, on purpose
///
/// `I64` is the width this compiler currently produces, and it is the only
/// one: a second width is a decision the compiler has not made, so it is not
/// spelled here as a variant nothing produces. The point of the type today is
/// that the decision has a name, one place, and a place to land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    I64,
}

impl IntWidth {
    /// The width, in bits, that a fragment declaring this width was lowered to
    /// mean. A backend compares this against what its target offers natively.
    pub const fn bits(self) -> u32 {
        match self {
            IntWidth::I64 => 64,
        }
    }
}

/// A fragment's parameter domain, expressed in the IR's own terms rather than
/// the host's.
///
/// The compiler stage reads the host IR's shape lattice, which carries far more
/// than a kernel can accept — strings, tables, functions, arrays, and an
/// `Unknown` for "not decided yet". A kernel domain is only ever a scalar or a
/// tuple of scalars, and the compiler *refuses* every other shape by name
/// before it lowers, so none of the rest can reach this type. Re-expressing the
/// accepted subset here is what keeps this crate free of a dependency on the
/// host IR: a backend needs the domain's *structure* (to flatten it into a
/// parameter list) and nothing else, and every shape it could encounter is one
/// of these two variants.
///
/// # The leaf carries its class
///
/// A parameter's scalar class is a property of its leaf, not of the fragment:
/// a tuple domain's elements may be classes of their own, and the class of a
/// value the body computes is the class of the leaf it lands in. Carrying it
/// here is also what makes the digest cover it for free —
/// [`fragment_digest`] hashes `param_shape` whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelShape {
    /// One scalar leaf, of the class it names.
    Scalar(ScalarClass),
    /// A tuple, flattened in order into scalar leaves.
    Tuple(Vec<KernelShape>),
}

impl KernelShape {
    /// How many scalar leaves this shape flattens to — the number of scalar
    /// parameters a function with this domain takes.
    ///
    /// This is the count a backend's parameter list is built from, so it is
    /// load-bearing: a backend that flattened the same shape differently would
    /// call one kernel with another's arguments.
    ///
    /// The count is a function of the *structure* alone, so a leaf's class does
    /// not change it: a float local and an integer local are one local each,
    /// which is what the compiler's own arity filler states on the host side.
    pub fn flat_arity(&self) -> usize {
        match self {
            KernelShape::Scalar(_) => 1,
            KernelShape::Tuple(items) => items.iter().map(KernelShape::flat_arity).sum(),
        }
    }
}

/// A binary arithmetic/comparison operator of the kernel-safe subset.
///
/// # Arithmetic is unsigned, because the language's `Int` is
///
/// An `Int` reaches the lowered graph as a machine-sized **unsigned** integer
/// (`LowValue::USize`, and `Sub` wraps), so every arithmetic operator here means
/// the unsigned one: `/` and `%` are `OpUDiv`/`OpUMod` and `<` `<=` `>` `>=` are
/// the unsigned comparisons, not their signed siblings.  A backend that emitted
/// the signed form would agree with the interpreter for every value below
/// `2^63` and disagree above it, which is reachable (`0 - 1`) and silent — so
/// the choice is stated here, once, rather than left to each emitter's default.
///
/// # A comparison yields a scalar, not a boolean
///
/// There is no `Bool` value in the language: a comparison yields `0`/`1`, which
/// is what drives a lazy `Index` branch (the conditional form) and what a
/// `Select` consumes.  A target whose comparisons yield a native boolean (SPIR-V)
/// therefore *materialises* the scalar at the comparison; a target whose
/// `select` takes a narrower condition (wasm's `i32`) narrows it there.  Both
/// decisions are the backend's, which is why neither is an instruction here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelBin {
    /// The two arithmetic families: `Add`/`Sub`/`Mul` and `Div`/`Rem`.  `Div`
    /// and `Rem` are undefined on a zero divisor, which is the one thing the
    /// kernel-safe subset does not turn into a diagnostic — see
    /// `docs/notes/operators.md`.
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
    /// The bitwise operators, which are also the language's boolean operators:
    /// a comparison's result is `0`/`1`, so `and`/`or`/`xor` over those values
    /// are `BitAnd`/`BitOr`/`BitXor`.
    BitAnd,
    BitOr,
    BitXor,
}

/// One abstract instruction in a lowered kernel body.
///
/// A fragment stores a `Vec<KernelInstr>` — **not** target code — so a backend
/// can resolve cross-kernel calls with link-time information (which callee index
/// a call becomes) after the whole reachable fragment set is known, rather than
/// during the walk that lowers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelInstr {
    /// Push a constant of the fragment's declared [`IntWidth`].
    Const(i64),
    /// A binary [`KernelBin`] operator over the top two stack values.
    Bin(KernelBin),
    /// Read a parameter leaf, by its offset in the flattened domain.
    LocalGet(u32),
    /// Convert the top stack value to the condition width a `select` needs.
    I32WrapI64,
    /// A `if c then a else b`: the then value, the else value, the selector,
    /// the width conversion, then this.
    Select,
    /// A cross-kernel call: the top `arity` stack values are the argument, and
    /// a backend resolves this to a call to the callee. The arity is the
    /// callee's own domain, not known here.
    CallKernel(KernelId),
    /// Read one element of one input buffer: the stack holds
    /// `[buffer_position, index]`.
    BufferReadCall,
    /// Write one element of one output buffer: the stack holds
    /// `[buffer_position, index, value]`. The buffer position is a
    /// compile-time constant pushed immediately before the call.
    BufferWriteCall,
}

/// A compiled kernel-callable unit: a lowered function body plus the facts a
/// caller needs to build and run it.
///
/// `jit` lowers one lichen function to a fragment. The fragment is what gets
/// interned and cached, and a backend assembles a reachable set of them into
/// something runnable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelFragment {
    /// The parameter domain, flattened by [`KernelShape::flat_arity`] into the
    /// backend's parameter list.
    pub param_shape: KernelShape,
    /// The lowered body, in emission order.
    pub body: Vec<KernelInstr>,
    /// How many input buffers this fragment reads — `0` for a scalar fragment
    /// and for a parallel fragment that reads none, otherwise one past the
    /// highest position any `compute.read` names. The twin of [`Self::outputs`],
    /// and for the same reason: a property of the *compiled* fragment, so a
    /// caller knows how many buffers to hand over without running it.
    ///
    /// **This is not in `param_shape`, and cannot be.** A parallel fragment's
    /// shape is `(config, index)` however many buffers it reads, because the
    /// buffers are bound as storage buffers and reached through a read's
    /// position rather than through a further parameter. So the two facts a
    /// caller needs are separate: how many it is *given* is the dispatch's, read
    /// at apply time from the call site's buffer tuple, and how many is *needed*
    /// is this, counted as the read positions are emitted. Neither one can stand
    /// in for the other, and a caller that supplied the wrong number would
    /// otherwise have its shader read a binding that was never bound.
    pub inputs: usize,
    /// How many output buffers this fragment writes — `0` for a scalar
    /// fragment, and for a parallel fragment the index function's codomain
    /// arity. A property of the *compiled* fragment, so a caller allocates
    /// exactly this many buffers and never discovers at run time which ones were
    /// written.
    pub outputs: usize,
    /// The element class of each input buffer, **one entry per read position**,
    /// in position order — so the length is [`Self::inputs`].
    ///
    /// # Why this is beside `inputs` rather than in `param_shape`
    ///
    /// The reason `inputs` itself is not in `param_shape` applies verbatim: a
    /// parallel fragment's shape is `(config, index)` — integers — however many
    /// float buffers it reads, because the buffers are bound as storage buffers
    /// and reached through a read's position rather than through a further
    /// parameter. A class that only lived on the parameter leaves would
    /// therefore have nothing to say about a buffer at all.
    ///
    /// The length is the **declared** [`Self::inputs`], including positions a
    /// body never read but which the count still covers (read positions are a
    /// sparse space, so the highest one read sizes the list).
    ///
    /// **This is the list a host slot is matched against**, because a dispatch's
    /// slots are ordered to match it — so position *is* the ordinal a slot would
    /// need, and the slot's own carrier is the deferral [`BufferSlot`] records.
    pub input_classes: Vec<ScalarClass>,
    /// The element class of each output buffer, **one entry per write ordinal**,
    /// in ordinal order — so the length is [`Self::outputs`].
    ///
    /// The ordinal/write-position correspondence is the same compile-time
    /// constant [`KernelInstr::BufferWriteCall`] is fed, so a caller reading
    /// element `k` of this list is reading the class of the buffer write `k`
    /// filled. A backend reads it where it prepares a buffer; a host reads it
    /// when a resident buffer has to say what its elements are.
    pub output_classes: Vec<ScalarClass>,
    /// How many values the body leaves on the stack: the function's result
    /// arity. `1` for a scalar body, and one per leaf for a tuple codomain.
    pub results: usize,
    /// The integer width the body was lowered to mean. A backend whose target
    /// cannot represent it refuses the fragment rather than narrowing.
    pub int_width: IntWidth,
}

/// A fragment's content digest: what makes a [`KernelId`] an identity.
///
/// **Every field of the fragment is hashed, and that is the invariant.** The
/// digest is what two structurally identical fragments intern under, so a
/// field left out of it is a way for two fragments that differ in it to share
/// one identity — and then a cache keyed on that identity serves one kernel's
/// compiled form for another's, silently. `results` is the sharpest case: it
/// types the emitted function's result list, so a fragment returning two values
/// and one returning three must not collapse together. `inputs` is the same
/// kind of case in the other direction: two bodies that differ only in how many
/// buffers they read are different programs, and the one that reads more must
/// not be served the other's identity.
///
/// The `Debug` rendering is the canonical form because it is a total,
/// deterministic function of each field, and this runs once per `jit` against a
/// compile it exists to avoid repeating.
///
/// # The class-carrying fields are hashed here, and that is not optional
///
/// `param_shape` is hashed whole, so a leaf's class rides along with it. The two
/// buffer-class lists do not: they are separate fields, and a field left out of
/// this function is exactly the silent fragment-sharing failure the paragraph
/// above names — two fragments that read buffers of different classes would
/// intern to one identity, and one kernel's compiled form would be served for
/// the other's.
pub fn fragment_digest(fragment: &KernelFragment) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", fragment.param_shape).hash(&mut hasher);
    format!("{:?}", fragment.body).hash(&mut hasher);
    fragment.inputs.hash(&mut hasher);
    fragment.outputs.hash(&mut hasher);
    format!("{:?}", fragment.input_classes).hash(&mut hasher);
    format!("{:?}", fragment.output_classes).hash(&mut hasher);
    fragment.results.hash(&mut hasher);
    format!("{:?}", fragment.int_width).hash(&mut hasher);
    hasher.finish()
}

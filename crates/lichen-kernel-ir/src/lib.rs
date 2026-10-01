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

/// A backend that can run a parallel fragment over an index range.
///
/// This is deliberately *not* the shape of a compiled module, a memory pool or a
/// device queue. It is the smallest thing a host program has to hand over, and
/// the smallest thing a backend has to promise: given a fragment, its input
/// buffers and a count, produce one output buffer per declared output.
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
    /// `inputs` holds one buffer per read position, each at least `count` long.
    /// The result is one buffer per [`KernelFragment::outputs`], each exactly
    /// `count` long.
    fn run(
        &self,
        fragment: &KernelFragment,
        inputs: &[Vec<i64>],
        count: usize,
    ) -> Result<Vec<Vec<i64>>, String>;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelShape {
    /// One scalar leaf.
    Scalar,
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
    pub fn flat_arity(&self) -> usize {
        match self {
            KernelShape::Scalar => 1,
            KernelShape::Tuple(items) => items.iter().map(KernelShape::flat_arity).sum(),
        }
    }
}

/// A binary arithmetic/comparison operator of the kernel-safe subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelBin {
    Add,
    Sub,
    Leq,
    Eq,
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
    /// A binary `add/sub/leq/eq` over the top two stack values.
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
    /// How many output buffers this fragment writes — `0` for a scalar
    /// fragment, and for a parallel fragment the index function's codomain
    /// arity. A property of the *compiled* fragment, so a caller allocates
    /// exactly this many buffers and never discovers at run time which ones were
    /// written.
    pub outputs: usize,
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
/// and one returning three must not collapse together.
///
/// The `Debug` rendering is the canonical form because it is a total,
/// deterministic function of each field, and this runs once per `jit` against a
/// compile it exists to avoid repeating.
pub fn fragment_digest(fragment: &KernelFragment) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", fragment.param_shape).hash(&mut hasher);
    format!("{:?}", fragment.body).hash(&mut hasher);
    fragment.outputs.hash(&mut hasher);
    fragment.results.hash(&mut hasher);
    format!("{:?}", fragment.int_width).hash(&mut hasher);
    hasher.finish()
}

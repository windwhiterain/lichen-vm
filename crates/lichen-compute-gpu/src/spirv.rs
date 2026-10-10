//! The SPIR-V emitter: a [`KernelFragment`] to a SPIR-V compute module.
//!
//! # Why this is hand-written
//!
//! The IR has a handful of value-producing operations and two buffer
//! operations, so the whole emitter is a small walk over [`KernelInstr`] that
//! keeps **one id per [`ValueId`]** — SPIR-V is SSA and this emitter does not
//! pretend otherwise. A builder crate would not remove the part that actually goes
//! wrong: the **module contract** — the section order, which capabilities are
//! declared, which variables carry which decorations, and what the entry point's
//! interface lists. A builder spells opcodes for you and validates nothing about
//! a module's shape.
//!
//! What removes that risk instead is offline validation: the emitted words can be
//! run through `spirv-val` and disassembled with `spirv-dis` before they reach a
//! driver, so a wrong module is a readable diagnostic rather than a
//! `VK_ERROR_*` at pipeline creation — or, worse, a silently miscompiled kernel.
//! Those tools are a *development* aid; this crate depends on nothing but `ash`
//! and the IR.
//!
//! # Structure: two passes, because a module's sections are ordered
//!
//! SPIR-V requires instructions in a fixed section order, and the entry point —
//! which must name the function and list the globals that function reaches —
//! comes *before* the types, variables and body it names. Ids are therefore
//! **all pre-allocated** before anything is emitted, the body is walked first to
//! learn which buffers it touches, and only then is the module written in
//! section order. Emitting in one pass would mean either forward-referencing the
//! entry point or emitting it twice.
//!
//! # An invariant the caller relies on: the body is straight-line
//!
//! The entry function carries exactly one `OpLabel` and the emitter emits **no
//! branch and no phi** — a `Select` is compiled to a branchless choice, not to a
//! jump. So every invocation of a dispatch reaches every instruction in the body,
//! including its `BufferWriteCall`.
//!
//! `dispatch` depends on this: it allocates output buffers and **does not
//! initialise them**, because a dispatch covers `[0, padded)` and every one of
//! those elements is stored by the lane that owns it. A conditional write would
//! leave the skipped elements as whatever a fresh allocation held, and the host
//! would read them back believing they were results. **If a branch is ever
//! introduced here, output buffers have to start being cleared again.**
//!
//! # The one place this target disagrees with the wasm backend
//!
//! The IR says a comparison yields the `0`/`1` scalar, because lichen has no
//! `Bool` value. `KernelInstr::I32WrapI64` exists because the wasm MVP's `select`
//! takes an `i32` *condition*, so a comparison has to be narrowed to drive one;
//! SPIR-V's comparison operations (`OpULessThanEqual`, `OpIEqual`) already yield
//! `OpTypeBool`, which is what `OpSelect` takes. So for the shape the compiler
//! actually emits for an `if` — a comparison, the narrowing, then a select — the
//! narrowing has nowhere to go here, and **it is a no-op**: the same instruction
//! stream, one target needing it (there, to build the condition) and the other
//! not. That is the clearest evidence the IR is target-neutral.
//!
//! What neither backend can escape is the other direction: a comparison *used as
//! a value* — a bitwise operand, a buffer element, a select arm, another
//! comparison's operand — has to become the scalar the language says it is, and
//! a scalar *condition* has to become a bool. Wasm gets both from the two
//! integer instructions that surround a comparison; this target emits the same
//! two conversions where the position demands them instead (see
//! [`as_class`]/[`as_condition`]). Same IR, same semantics, two spellings.
//!
//! # A module's *buffers* have one numeric class; its values may be either
//!
//! [`module_class`] reads the class a module is built for off the fragment's
//! declared positions, and that class is baked in rather than chosen per
//! dispatch: the storage buffer's element type, the pointer into it and its
//! array stride are all module-scope instructions — and [`Binding`], the
//! caller's record of how many buffers a run binds, has nowhere to say which
//! class they hold.
//!
//! **Both element types are nevertheless declared in every module.** `Int` and
//! `Float` do not convert in either direction on their own
//! (`docs/notes/floating-point.md` §4.2), but a body may compute in one class
//! and cross to the other through the explicit `Conv`, so both the module's
//! scalar and the other element type have to have an id to name. `OpTypeFloat 32`
//! is core SPIR-V and costs no capability, so only the **64-bit integer** is
//! conditional: an integer module's scalar is that 64-bit unsigned integer, its
//! buffer element is eight bytes, and it declares `Int64`; a float module's
//! scalar is a 32-bit float, its element is four bytes, and its **index** is
//! 32-bit, so it declares no 64-bit integer at all and needs no device with
//! `shaderInt64` — [`needs_int64`] is what a caller checks before it builds a
//! pipeline.
//!
//! The price of the missing 64-bit integer is that a float module's `Int` data
//! is 32-bit where the wasm target's is 64-bit, so the two diverge past 2³²; it
//! is the same kind of recorded price as kernels computing `f32` while the
//! interpreter computes `f64` (`docs/notes/floating-point.md`).
//!
//! A comparison still yields `OpTypeBool` here and is still materialised only
//! where a position wants the scalar, but *which* scalar is the class's: the
//! language's `1`/`0` is `1.0`/`0.0` over two floats, not an integer select.

use std::collections::HashMap;
use std::fmt;

use lichen_kernel_ir::{
    KernelBin, KernelFragment, KernelInstr, KernelShape, ScalarClass, Terminator, ValueDef, ValueId,
};

/// SPIR-V opcodes.  Not from memory: these are the `SpvOp*` values in the
/// Khronos `spirv.h` shipped with the Vulkan SDK.
mod op {
    pub const CAPABILITY: u16 = 17;
    pub const MEMORY_MODEL: u16 = 14;
    pub const ENTRY_POINT: u16 = 15;
    pub const EXECUTION_MODE: u16 = 16;
    pub const DECORATE: u16 = 71;
    pub const MEMBER_DECORATE: u16 = 72;
    pub const TYPE_VOID: u16 = 19;
    pub const TYPE_BOOL: u16 = 20;
    pub const TYPE_INT: u16 = 21;
    pub const TYPE_FLOAT: u16 = 22;
    pub const TYPE_VECTOR: u16 = 23;
    pub const TYPE_RUNTIME_ARRAY: u16 = 29;
    pub const TYPE_POINTER: u16 = 32;
    pub const TYPE_STRUCT: u16 = 30;
    pub const TYPE_FUNCTION: u16 = 33;
    pub const CONSTANT: u16 = 43;
    pub const COMPOSITE_EXTRACT: u16 = 81;
    pub const VARIABLE: u16 = 59;
    pub const LOAD: u16 = 61;
    pub const STORE: u16 = 62;
    pub const ACCESS_CHAIN: u16 = 65;
    pub const FUNCTION: u16 = 54;
    pub const FUNCTION_END: u16 = 56;
    pub const LABEL: u16 = 248;
    pub const RETURN: u16 = 253;
    pub const U_CONVERT: u16 = 113;
    /// `OpConvertFToU` (109) — the `float2int` crossing: a 32-bit float to the
    /// unsigned integer type, truncating toward zero (and undefined outside what
    /// that type holds, which is why the language refuses what the interpreter
    /// can see is out of range).
    pub const CONVERT_F_TO_U: u16 = 109;
    /// `OpConvertUToF` (112) — the `int2float` crossing.  The **unsigned** source
    /// is the language's `Int`, so this is the conversion and not
    /// `OpConvertSToF` (111): the two read the same bits and answer different
    /// numbers past the signed range, so the wrong one is a wrong *answer* rather
    /// than a failed shape.  The wasm backend's `F32ConvertI64U` is the same
    /// unsigned reading, and `tests/spirv_validation.rs` pins this opcode.
    pub const CONVERT_U_TO_F: u16 = 112;
    /// The reinterpretation an integer constant and a float operand meet
    /// through: same width, same bits, no conversion of the value.
    pub const BITCAST: u16 = 124;
    pub const I_ADD: u16 = 128;
    pub const F_ADD: u16 = 129;
    pub const I_SUB: u16 = 130;
    pub const F_SUB: u16 = 131;
    pub const I_MUL: u16 = 132;
    pub const F_MUL: u16 = 133;
    pub const U_DIV: u16 = 134;
    pub const F_DIV: u16 = 136;
    pub const U_MOD: u16 = 137;
    pub const SELECT: u16 = 169;
    pub const I_EQUAL: u16 = 170;
    pub const I_NOT_EQUAL: u16 = 171;
    pub const U_GREATER_THAN: u16 = 172;
    pub const U_GREATER_THAN_EQUAL: u16 = 174;
    pub const U_LESS_THAN: u16 = 176;
    pub const U_LESS_THAN_EQUAL: u16 = 178;
    /// The **ordered** float comparisons: false when either operand is `NaN`,
    /// which is what Rust's operators and wasm's `f32.lt` and friends do, so the
    /// two backends agree about a `NaN` operand.
    pub const F_LESS_THAN: u16 = 184;
    pub const F_GREATER_THAN: u16 = 186;
    pub const F_LESS_THAN_EQUAL: u16 = 188;
    pub const F_GREATER_THAN_EQUAL: u16 = 190;
    pub const BITWISE_OR: u16 = 197;
    pub const BITWISE_XOR: u16 = 198;
    pub const BITWISE_AND: u16 = 199;
}

/// `SpvCapabilityShader`, `SpvCapabilityInt64`.
mod capability {
    pub const SHADER: u32 = 1;
    pub const INT64: u32 = 11;
}

/// `SpvStorageClass` values used below.
mod storage_class {
    pub const INPUT: u32 = 1;
    pub const STORAGE_BUFFER: u32 = 12;
}

/// `SpvDecoration` values used below.
mod decoration {
    pub const BUILT_IN: u32 = 11;
    pub const BLOCK: u32 = 2;
    pub const ARRAY_STRIDE: u32 = 6;
    pub const OFFSET: u32 = 35;
    pub const BINDING: u32 = 33;
    pub const DESCRIPTOR_SET: u32 = 34;
}

/// `SpvBuiltInGlobalInvocationId`.
const BUILT_IN_GLOBAL_INVOCATION_ID: u32 = 28;
/// `SpvExecutionModelGLCompute`, `SpvExecutionModeLocalSize`.
const EXECUTION_MODEL_GL_COMPUTE: u32 = 5;
const EXECUTION_MODE_LOCAL_SIZE: u32 = 17;
/// `SpvAddressingModelLogical`, `SpvMemoryModelGLSL450`.
const ADDRESSING_MODEL_LOGICAL: u32 = 0;
const MEMORY_MODEL_GLSL450: u32 = 1;
/// A non-zero generator magic, as SPIR-V requires.
const GENERATOR: u32 = 0x0030_0000;
/// 1.3 is the first version with the `StorageBuffer` storage class, which is what
/// lets a buffer be a plain runtime array with an `ArrayStride` and no `Block`
/// wrapper.  The device is Vulkan 1.4, so 1.3 is in range.
const SPIRV_VERSION: u32 = 0x0001_0300;
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// The bytes one buffer element occupies, per class: the array stride of the
/// module's runtime array.
///
/// **Derived, not decided here.**  What an element occupies is a fact about the
/// class ([`ScalarClass::byte_width`]) and this is the target's spelling of it,
/// so a module's `ArrayStride` and the host that hands the buffer over cannot
/// disagree: the wasm backend packs at the same width, and the host's byte
/// arithmetic is that width too (`docs/notes/floating-point.md` §4.1).  A literal
/// here would be a second copy of the rule, and it is exactly the drift that one
/// produced: this target read a float buffer at four bytes while the host wrote
/// it at eight, and a float fragment was undispatchable until the width moved
/// onto the class.
fn element_stride(class: ScalarClass) -> u32 {
    class.byte_width() as u32
}

/// The local workgroup size.  A run of `count` indices is
/// `ceil(count / LOCAL_SIZE_X)` workgroups; a workgroup that runs past the end
/// computes an index whose slot the bound buffers do not cover, so the surplus
/// lanes of the last workgroup are a **caller** obligation, not the shader's.
/// See [`crate::dispatch`].
pub const LOCAL_SIZE_X: u32 = 64;

/// Why a fragment cannot be emitted for this target.
///
/// Every variant names its own cause and nothing is approximated: a kernel that
/// runs but computes the wrong thing is worse than one that does not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpirvRefusal {
    /// A cross-kernel call.  Supporting it means emitting several functions into
    /// one module and resolving the call graph, which this slice does not do.
    CrossKernelCall { kernel: usize, at: usize },
    /// A parameter read that is not the index.  A buffer is *bound* to this
    /// shader as a storage buffer, never passed as a value, so there is no value
    /// a non-index parameter could produce.
    NonIndexParameter { local: u32, at: usize },
    /// A buffer position that is not a compile-time constant.  The IR guarantees
    /// this by construction — a position is an emitted ordinal — so this means a
    /// malformed fragment rather than a limitation of the target.
    NonConstantBufferPosition { at: usize },
    /// A buffer position outside the space it addresses.
    BufferPositionOutOfRange {
        position: i64,
        space: &'static str,
        bound: usize,
        at: usize,
    },
    /// The fragment declares an integer width this target cannot represent.
    UnsupportedIntWidth { bits: u32 },
    /// The fragment's declared positions are not all of one class, and a module
    /// has one element type: one struct, one pointer into it, one array stride.
    /// `Int` and `Float` do not convert (`docs/notes/floating-point.md` §4.2),
    /// so there is no second element type to emit and nothing to choose between.
    MixedElementClasses,
    /// An operation over `Float` the language has no form for. A `Float` takes
    /// `+ - * /` and the four order comparisons and nothing else
    /// (`docs/notes/floating-point.md` §3.7), so `%` and the bitwise trio have
    /// no opcode to reach here: the alternatives would be a different function
    /// or a reinterpretation of a float's bits, and a kernel that runs but
    /// computes something nobody wrote is worse than one that does not run.
    UnsupportedFloatOperator { operator: &'static str, at: usize },
    /// A value and a buffer index of different classes met in one operation.
    /// `Int` and `Float` do not convert in either direction
    /// (`docs/notes/floating-point.md` §4.2), so this is a malformed fragment
    /// rather than a shape a conversion could serve.
    MixedClasses { at: usize },
    /// The body does not leave exactly the one value a compute shader needs.
    ResultArity { results: usize, left: usize },
    /// A body whose structure the target has not been taught to emit.
    ///
    /// **Refused rather than emitted straight-line**, because a dropped branch is
    /// a fragment that computes a different program than it was lowered from —
    /// and because the transfer is already in the IR, so this refusal is about the
    /// *emitter*, not about what the language can say.
    ControlFlow { detail: String },
}

impl fmt::Display for SpirvRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpirvRefusal::CrossKernelCall { kernel, at } => write!(
                f,
                "the GPU backend does not emit cross-kernel calls: instruction {at} calls kernel \
                 {kernel}. That needs a call graph across several functions in one module, which \
                 this backend does not do yet."
            ),
            SpirvRefusal::NonIndexParameter { local, at } => write!(
                f,
                "instruction {at} reads parameter {local}, which is not the index. A buffer is \
                 bound to this shader as a storage buffer rather than passed as a value, so there \
                 is no value for a non-index parameter to hold."
            ),
            SpirvRefusal::NonConstantBufferPosition { at } => write!(
                f,
                "instruction {at} uses a buffer position that is not a compile-time constant. A \
                 position is an emission ordinal by construction, so this fragment is malformed."
            ),
            SpirvRefusal::BufferPositionOutOfRange {
                position,
                space,
                bound,
                at,
            } => write!(
                f,
                "instruction {at} names {space} buffer {position}, but this module binds \
                 {bound} of them."
            ),
            SpirvRefusal::UnsupportedIntWidth { bits } => write!(
                f,
                "the fragment was lowered for {bits}-bit integers, which this target does not \
                 represent."
            ),
            SpirvRefusal::MixedElementClasses => write!(
                f,
                "the fragment's declared positions are not all of one class. A module's element \
                 type, the pointer into it and its array stride are one decision baked into the \
                 module, and `Int` and `Float` do not convert, so a fragment that declares both \
                 has no module here."
            ),
            SpirvRefusal::UnsupportedFloatOperator { operator, at } => write!(
                f,
                "instruction {at} applies {operator} to floats, and the language gives a `Float` \
                 `+ - * /` and the four comparisons and nothing else. There is no float operator \
                 to emit for this, and emitting a different one would compute something the \
                 program does not say."
            ),
            SpirvRefusal::MixedClasses { at } => write!(
                f,
                "instruction {at} mixed an integer and a float in one operation. `Int` and `Float` \
                 do not convert in either direction, so nothing here can make the two operands \
                 meet: this is a malformed fragment rather than an unsupported shape."
            ),
            SpirvRefusal::ResultArity { results, left } => write!(
                f,
                "the fragment declares {results} result(s) and its body returns {left} value(s). \
                 A compute shader communicates through its storage buffers, so this backend \
                 requires a fragment whose body returns exactly one value."
            ),
            SpirvRefusal::ControlFlow { detail } => write!(
                f,
                "this emitter does not yet emit a body with control flow: {detail}. The transfer is \
                 in the kernel IR, so this is the emitter's limit rather than something the program \
                 could not say — see `docs/notes/loop-conversion.md` §8."
            ),
        }
    }
}

/// How many buffers a module binds.  A property of the *caller's* buffers, not of
/// the fragment: a fragment names buffers by position but does not say how many
/// exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    /// Buffers reachable through `BufferReadCall`.
    pub inputs: usize,
    /// Buffers reachable through `BufferWriteCall`.
    pub outputs: usize,
}

impl Binding {
    /// How many storage buffers a module with this binding declares.
    pub fn total(self) -> usize {
        self.inputs + self.outputs
    }
}

/// One not-yet-encoded instruction.
struct Inst {
    opcode: u16,
    operands: Vec<u32>,
}

impl Inst {
    fn new(opcode: u16, operands: Vec<u32>) -> Inst {
        Inst { opcode, operands }
    }

    /// Encode as `word count << 16 | opcode`, then the operands.
    fn encode(&self, out: &mut Vec<u32>) {
        out.push(((self.operands.len() as u32 + 1) << 16) | u32::from(self.opcode));
        out.extend_from_slice(&self.operands);
    }
}

/// One lowered value: the SSA id SPIR-V gave it, and which of the module's types
/// that id has.
#[derive(Debug, Clone, Copy)]
struct Slot {
    id: u32,
    kind: Kind,
}

/// The type a slot's id has — and, for the one instruction whose class the IR
/// does not fix, the value it holds.
///
/// # Why the type has to ride on the slot
///
/// **The IR is untyped and this target is not.** A comparison here yields
/// `OpTypeBool` — the shape `OpSelect` wants, and the one place this backend
/// differs from wasm's, whose comparisons yield a narrow integer instead. But
/// the *language* says a comparison yields the `0`/`1` scalar, and a kernel may
/// use it as one: a bitwise operand (`(a < b) & c`), a comparison's own operand
/// (`(a < b) == c`), a stored element, a `Select` arm. In every one of those
/// positions a bool is the wrong type, and a module that mixed them would be
/// rejected by the driver rather than compute something else. So each slot
/// records what it holds, and the two coercions ([`as_class`]/[`as_condition`])
/// are emitted exactly where a position demands the other — which for the common
/// case (`if` over a comparison) is nowhere.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A value of the named class: the module's scalar, or the integer an access
    /// chain's index takes, which is the same type as the scalar in an integer
    /// module and a 32-bit one in a float module.
    Scalar(ScalarClass),
    /// A constant, and the `i64` the body pushed for it. **The class is not
    /// fixed by the IR**: the same `Const(0)` is a buffer position in one place
    /// (the integer `0`) and a float's bit pattern in another (the `f32` whose
    /// bits are zero), and only the position it is consumed in decides. So the
    /// payload rides here until a position reads it, and the constant is
    /// materialised per class on demand — see [`Literals`].
    Literal(i64),
    /// The `OpTypeBool` a comparison yields.
    Condition,
}

/// A slot holding a value of `class`.
const fn scalar(id: u32, class: ScalarClass) -> Slot {
    Slot {
        id,
        kind: Kind::Scalar(class),
    }
}

/// A slot holding a constant of a class the consuming position will decide.
const fn literal(id: u32, value: i64) -> Slot {
    Slot {
        id,
        kind: Kind::Literal(value),
    }
}

/// A slot holding a comparison's `OpTypeBool`.
const fn condition(id: u32) -> Slot {
    Slot {
        id,
        kind: Kind::Condition,
    }
}

/// The two scalar kinds, spelled once so the operator table reads as classes
/// rather than as constructor calls.
const INT_VALUE: Kind = Kind::Scalar(ScalarClass::Int);
const FLOAT_VALUE: Kind = Kind::Scalar(ScalarClass::Float);

impl Slot {
    /// The compile-time constant this slot is, or `None` for a computed value.
    ///
    /// A buffer operation's *position* selects **which** storage-buffer variable
    /// to reach — a compile-time choice — so it is read as a number rather than
    /// as an id. A position is a `Const` computed immediately before the call: a
    /// value that was *computed* is not an ordinal however constant its value
    /// happens to be, and reading one as a position would address a buffer the
    /// caller never named.
    fn constant(self) -> Option<i64> {
        match self.kind {
            Kind::Literal(value) => Some(value),
            _ => None,
        }
    }
}

/// The module-scope constants the body's literals need, one pool per class a
/// literal can be read as.
///
/// Keyed by class *and* value because the two readings of one `i64` are two
/// different SPIR-V constants: a float value is its 32 bits, an integer is the
/// number. The id in [`Slot`] is the reading the push assumed (the module's
/// scalar); a position that wants the other class asks here again, and the
/// constant is emitted once however many positions use it.
#[derive(Default)]
struct Literals {
    /// The `OpConstant` declarations, emitted in the types-and-constants
    /// section: a constant is module-scope, so nothing may put it in the body.
    declarations: Vec<Inst>,
    emitted: HashMap<(ScalarClass, i64), u32>,
}

impl Literals {
    /// The id of a constant of `class` holding `value`, emitting it if this is
    /// the first use.
    fn get(&mut self, class: ScalarClass, value: i64, ids: &Ids, next: &mut u32) -> u32 {
        if let Some(id) = self.emitted.get(&(class, value)) {
            return *id;
        }
        let id = *next;
        *next += 1;
        let mut operands = vec![ids.type_of(class), id];
        // The literal words are the **type's** width, which for the integer class
        // is the module's integer width and not the language's: 64 bits in an
        // integer module, 32 in a float one, where an integer is only ever an
        // element index. `OpTypeInt 32` with two literal words is an invalid
        // instruction, not merely a wide constant.
        let wide = class == ScalarClass::Int && ids.class == ScalarClass::Int;
        if wide {
            // Low word, then high word.
            operands.push(value as u32);
            operands.push((value >> 32) as u32);
        } else {
            // A 32-bit type's literal is one word: the integer's low half, or the
            // float's bits. Which way a producer widened a `u32` into the IR's
            // `i64` — zero- or sign-extended — is not something the IR says, and
            // `as u32` reads either the same way.
            operands.push(value as u32);
        }
        self.declarations.push(Inst::new(op::CONSTANT, operands));
        self.emitted.insert((class, value), id);
        id
    }
}

/// The storage-buffer type chain for **one** class: the element struct, the
/// runtime array over it, the block struct that wraps the array, and the two
/// pointers the body reaches through.
///
/// Vulkan requires a `StorageBuffer` variable to be typed as a struct, or an
/// array of one — a bare runtime array is the older `Uniform` + `BufferBlock`
/// style and is rejected (`VUID-StandaloneSpirv-Uniform-06807`) — and a runtime
/// array may only be the final member of one, so the buffer takes two struct
/// levels: the element struct, and this one holding the array.
///
/// `ptr_elem` is a pointer to one element *in the storage buffer storage class* —
/// what an access chain produces, and what `OpLoad` reads and `OpStore` writes
/// through. Its pointee is the class's own scalar, which is what makes a buffer's
/// element type a fact of the buffer rather than of the module.
#[derive(Clone, Copy)]
struct BufferTypes {
    elem: u32,
    array: u32,
    buffer_struct: u32,
    ptr_array: u32,
    ptr_elem: u32,
}

/// Every module-scope id, allocated before anything is emitted.
struct Ids {
    /// The class the module's *arithmetic* is built for, [`module_class`]'s
    /// answer. It decides the default scalar type and which arithmetic opcodes a
    /// `Bin` reaches — but **not** a buffer's element type, which is that
    /// buffer's own class: see [`Self::chain_of`].
    class: ScalarClass,
    main: u32,
    label: u32,
    void: u32,
    boolean: u32,
    /// The fragment's own integer type, and the type every *integer* value in
    /// the body has: **unsigned** 64-bit in an integer module, because the
    /// language's `Int` is a machine-sized unsigned integer, so it is what a
    /// buffer element holds and what the unsigned opcodes (`OpUDiv`, `OpUMod`,
    /// `OpULessThan`, …) require — SPIR-V checks that operand, so declaring this
    /// signed would make a kernel that divides, takes a remainder or compares
    /// *invalid* rather than wrong.  `spirv-val` rules on that offline; see the
    /// module docs.
    ///
    /// **A float module does not declare it.** Its integer values are 32-bit
    /// element indices, and declaring a 64-bit integer would cost the `Int64`
    /// capability — and, with it, a device feature a float kernel has no use for
    /// (`docs/notes/floating-point.md` §4.4).
    ulong: u32,
    /// The 32-bit float: the scalar in a float module, and declared only there.
    /// SPIR-V's `Float32` is core, so it carries no capability.
    float: u32,
    /// The 32-bit unsigned type. It is the type of an invocation id's component
    /// and of an access chain's member indices in every module, and — in a float
    /// module, where nothing is 64-bit — the type an element index has.
    uint: u32,
    v3uint: u32,
    /// The per-class storage-buffer type chain, keyed by [`ScalarClass::index`].
    ///
    /// **One per class a fragment actually uses**, because the element type, the
    /// array stride and the block struct are all a property of the *buffer's*
    /// class rather than of the module: Vulkan binds a storage buffer against the
    /// stride its own `ArrayStride` declares, so a module that read one `Int`
    /// buffer and wrote one `Float` buffer needs both chains and a variable
    /// typed with the chain of the buffer it names.
    ///
    /// An id nothing defines is legal — the id bound is an upper limit, not a
    /// count — so only the classes a fragment uses are emitted.
    buffer_types: [BufferTypes; ScalarClass::ALL.len()],
    ptr_in: u32,
    fn_ty: u32,
    /// The 32-bit `0` an access chain's member indices are built from, and the
    /// zero a float's *bit pattern* is compared against ([`as_condition`]) — 32
    /// bits in every module, which is what makes it the right zero there even
    /// where the integer class is 64-bit.
    zero: u32,
    /// The scalar `1` and `0` a comparison is materialised into — the operands
    /// of the `OpSelect` [`as_class`] emits — where the position wants the
    /// **integer** class.  In an integer module that class is the module's
    /// scalar and these are its pair; in a float module the integer is the
    /// 32-bit element index, whose `0` is [`Self::zero`].
    one_integer: u32,
    zero_integer: u32,
    /// The same materialisation where the position wants the **float** class:
    /// the `f32` pair, declared in every module.  In a float module these are
    /// the module's scalar pair; in an integer module they are the only floats
    /// it holds, which is what lets a comparison be materialised there at all.
    one_float: u32,
    zero_float: u32,
    gid: u32,
    /// The first of `binding.total()` consecutive storage-buffer variables.
    buffers: u32,
}

impl Ids {
    /// The module's scalar: the type of every value the body computes.
    fn scalar(&self) -> u32 {
        match self.class {
            ScalarClass::Int => self.ulong,
            ScalarClass::Float => self.float,
        }
    }

    /// The type an integer value has — an element index, or an integer operand.
    fn integer(&self) -> u32 {
        match self.class {
            ScalarClass::Int => self.ulong,
            ScalarClass::Float => self.uint,
        }
    }

    /// The type a value of `class` has.
    fn type_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.integer(),
            ScalarClass::Float => self.float,
        }
    }

    /// The scalar `1`/`0` of `class`, for materialising a comparison.
    fn one_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.one_integer,
            ScalarClass::Float => self.one_float,
        }
    }

    fn zero_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.zero_integer,
            ScalarClass::Float => self.zero_float,
        }
    }

    /// The storage-buffer type chain for `class`.
    fn chain_of(&self, class: ScalarClass) -> BufferTypes {
        self.buffer_types[class.index()]
    }
}

/// A NUL-terminated, word-padded literal string, as SPIR-V packs them: four
/// bytes per word, little-endian, zero-padded to a word boundary.
fn spv_string(text: &[u8]) -> Vec<u32> {
    let mut bytes = text.to_vec();
    bytes.push(0);
    bytes
        .chunks(4)
        .map(|chunk| {
            chunk.iter().enumerate().fold(0u32, |word, (shift, byte)| {
                word | u32::from(*byte) << (shift * 8)
            })
        })
        .collect()
}

/// The flattened parameter offset the index occupies.
///
/// A parallel fragment's parameters are its input slots followed by the index, so
/// the index is the **last** leaf. It is recognised structurally rather than
/// passed in, so a caller cannot disagree with a fragment about which parameter
/// is the index.
pub(crate) fn index_local(fragment: &KernelFragment) -> Option<u32> {
    let arity = fragment.param_shape.flat_arity();
    (arity > 0).then(|| (arity - 1) as u32)
}

/// Every parameter leaf's class, in flattening order — the order
/// [`KernelShape::flat_arity`] counts and the entry block's parameters are in.
fn leaf_classes(shape: &KernelShape) -> Vec<ScalarClass> {
    match shape {
        KernelShape::Scalar(class) => vec![*class],
        KernelShape::Tuple(items) => items.iter().flat_map(leaf_classes).collect(),
    }
}

/// The class of **buffer `slot`** — inputs first, then outputs, which is the
/// order a [`Binding`] and every `Buffer{Read,Write}Call` position use.
///
/// **This is what makes a module able to bind buffers of different classes.** A
/// buffer's element type, array stride and block struct are that buffer's own
/// facts, so each one is read from the fragment's per-buffer class lists rather
/// than from a single module-wide answer; a fragment whose lists do not reach a
/// slot falls back to [`module_class`], which is the class its parameters imply.
///
/// `pub(crate)` because the **dispatch path asks it too**: the bytes staged for a
/// buffer are that buffer's class's `byte_width()`, and a fragment with two
/// classes stages two widths. One question, asked by both sides, so a caller and
/// the module it binds cannot disagree about how wide a buffer is.
pub(crate) fn buffer_class_of(
    fragment: &KernelFragment,
    slot: usize,
    fallback: ScalarClass,
) -> ScalarClass {
    fragment
        .input_classes
        .get(slot)
        .or_else(|| {
            fragment
                .output_classes
                .get(slot.checked_sub(fragment.inputs)?)
        })
        .copied()
        .unwrap_or(fallback)
}

/// Every class a fragment's **buffers** hold, in [`ScalarClass::ALL`] order.
///
/// **The set the module declares a type chain for.** A class no buffer holds is
/// not declared: an unused `OpTypeStruct` is legal but it would be a second copy
/// of a rule nobody asked for, and a fragment that declares it has no way to say
/// which of its chains a given binding means.
fn buffer_classes(fragment: &KernelFragment, fallback: ScalarClass) -> Vec<ScalarClass> {
    let used = |class: &ScalarClass| {
        fragment.input_classes.contains(class) || fragment.output_classes.contains(class)
    };
    ScalarClass::ALL
        .into_iter()
        .filter(|class| {
            used(class) || (*class == fallback && fragment.inputs + fragment.outputs == 0)
        })
        .collect()
}

/// The class a fragment's **arithmetic** is built for — the default scalar, and
/// the answer a fragment with no declared buffer class is read as.
///
/// # Where it is read from, and why that order
///
/// The **buffer element classes come first**: they are what the body reads and
/// writes, and they are the positions data crosses the ABI at. A fragment with
/// no declared buffer class at all — a body that only computes — is read off its
/// parameter leaves **except the index**, because this target's index is the
/// invocation id rather than a value of the fragment: a parallel fragment's
/// `param_shape` is `(config, index)` and both leaves are integers whatever its
/// buffers hold.
///
/// A fragment with neither (no buffers, one leaf, that leaf being the index) is
/// an integer module, which is what every fragment was before a class existed.
///
/// **It no longer refuses a fragment whose buffers disagree.** It is one module's
/// *arithmetic* class now, and a buffer's element type is that buffer's own
/// class ([`buffer_class`]); the refusal this function used to end with is gone
/// with the assumption behind it. The first buffer class wins, so a fragment
/// with no arithmetic of its own is built for the class of the first buffer it
/// binds, which is the one its values came from.
pub fn module_class(fragment: &KernelFragment) -> Result<ScalarClass, SpirvRefusal> {
    if let Some(class) = fragment
        .input_classes
        .iter()
        .chain(fragment.output_classes.iter())
        .copied()
        .next()
    {
        return Ok(class);
    }
    let leaves = leaf_classes(&fragment.param_shape);
    let index = index_local(fragment);
    Ok(leaves
        .iter()
        .enumerate()
        .find(|(offset, _)| Some(*offset as u32) != index)
        .map(|(_, class)| *class)
        .unwrap_or(ScalarClass::Int))
}

/// Whether a module for this fragment declares the 64-bit integer type, and so
/// needs a device that offers `shaderInt64`.
///
/// **SPIR-V's `Float32` is core, so a float fragment needs no capability at
/// all**: its index is 32-bit and no value in it is a 64-bit integer.
///
/// **It asks whether a 64-bit integer is used *anywhere*, not whether the module
/// is an integer one.** That is the difference a mixed fragment makes: it reads
/// an `Int` buffer, so its module declares the `Int64` chain and needs the
/// feature, even though its arithmetic default is `Float` and it would have
/// answered `false` before.
pub fn needs_int64(fragment: &KernelFragment) -> Result<bool, SpirvRefusal> {
    let holds_an_integer_buffer = fragment
        .input_classes
        .iter()
        .chain(fragment.output_classes.iter())
        .any(|class| *class == ScalarClass::Int);
    Ok(holds_an_integer_buffer || module_class(fragment)? == ScalarClass::Int)
}

/// Whether every buffer a fragment binds is of one class.
///
/// # Why this is asked at all when the module no longer cares
///
/// **The module side already answers a mixed fragment**: the element type, the
/// array stride, the block struct and the variable's own type are each read off
/// *that buffer's* class, and `spirv-val` accepts the result. What has **not**
/// been made per-buffer is the **host staging**: a dispatch reserves one upload
/// block per host input at `count × class.byte_width()` and offsets the next
/// block by that same width, so a fragment binding an `Int` buffer and a `Float`
/// one would upload eight bytes per `f32` element and read back a wrong number.
///
/// That is a silently wrong answer rather than a slow one, so a mixed fragment is
/// **refused by name here** until staging is per-buffer too. The refusal is in
/// the dispatch path on purpose: the emitter must keep accepting a mixed fragment,
/// because that is the half that is finished and is what `spirv-val` checks.
pub fn buffers_are_uniform(fragment: &KernelFragment) -> bool {
    let mut seen: Option<ScalarClass> = None;
    for class in fragment
        .input_classes
        .iter()
        .chain(fragment.output_classes.iter())
    {
        match seen {
            Some(previous) if previous != *class => return false,
            _ => seen = Some(*class),
        }
    }
    true
}

/// Compile one fragment to SPIR-V words.
pub fn compile(fragment: &KernelFragment, binding: Binding) -> Result<Vec<u32>, SpirvRefusal> {
    if let Err(broken) = fragment.body.validate() {
        return Err(SpirvRefusal::ControlFlow { detail: broken });
    }
    // Until this emitter learns `OpLoopMerge` / `OpBranch` / `OpPhi`, a body with
    // more than one block is refused rather than emitted straight-line.  See
    // `SpirvRefusal::ControlFlow` for why that is the only safe answer.
    if !fragment.body.is_straight_line() {
        return Err(SpirvRefusal::ControlFlow {
            detail: format!(
                "this body has {} block(s) and a transfer",
                fragment.body.blocks.len()
            ),
        });
    }
    if fragment.int_width.bits() != 64 {
        return Err(SpirvRefusal::UnsupportedIntWidth {
            bits: fragment.int_width.bits(),
        });
    }
    // The module's one numeric class, read off the fragment before anything is
    // emitted: it decides the scalar type, the element type and stride, the
    // arithmetic opcodes and the capability list, and [`needs_int64`] reads the
    // same derivation, so a caller and this module cannot disagree about it.
    let class = module_class(fragment)?;
    let index = index_local(fragment).ok_or(SpirvRefusal::ResultArity {
        results: fragment.result_classes.len(),
        left: 0,
    })?;

    // Pass 1 — allocate every module-scope id, then walk the body into an
    // instruction list. The function-local ids come from `next`, which starts
    // after the last module-scope id and ends as the module's id bound.
    //
    // A float module declares no 64-bit integer, so `ulong` is reserved and left
    // undefined there. An id that nothing defines is legal: the id bound is an
    // upper limit, not a count.
    //
    // **The storage-buffer type chain is allocated per class in use**, and this is
    // what lets one module bind an `Int` buffer and a `Float` buffer: five ids per
    // class the fragment's buffers actually hold, in [`ScalarClass::ALL`] order. A
    // class no buffer holds is not allocated, so `Ids::chain_of` is only
    // read for a class [`buffer_classes`] reported.
    let (one_integer, zero_integer, one_float, zero_float, gid) = match class {
        ScalarClass::Int => (17, 18, 19, 20, 21),
        ScalarClass::Float => (19, 16, 17, 18, 20),
    };
    let classes = buffer_classes(fragment, class);
    // Five ids per class, then the two module-wide pointers and the function type.
    let chain_base = 9;
    let fn_ty = chain_base + 5 * classes.len() as u32;
    let ptr_in = fn_ty + 1;
    let mut buffer_types = [BufferTypes {
        elem: 0,
        array: 0,
        buffer_struct: 0,
        ptr_array: 0,
        ptr_elem: 0,
    }; ScalarClass::ALL.len()];
    for (position, used) in classes.iter().enumerate() {
        let base = chain_base + 5 * position as u32;
        buffer_types[used.index()] = BufferTypes {
            elem: base,
            array: base + 1,
            buffer_struct: base + 2,
            ptr_array: base + 3,
            ptr_elem: base + 4,
        };
    }
    let ids = Ids {
        class,
        main: 1,
        label: 2,
        void: 3,
        boolean: 4,
        ulong: 5,
        float: 6,
        uint: 7,
        v3uint: 8,
        buffer_types,
        ptr_in,
        fn_ty,
        zero: 16,
        one_integer,
        zero_integer,
        one_float,
        zero_float,
        gid,
        buffers: gid + 1,
    };
    let mut next = ids.buffers + binding.total() as u32;
    // `OpConstant` is a *module-scope* instruction, so the body's literals are
    // collected here and emitted with the types rather than inside the function.
    let mut literals = Literals::default();
    let mut code: Vec<Inst> = Vec::new();
    // **The map that replaces the operand stack.** Every value the body defines is
    // here once it has been emitted, so an operand is a lookup rather than a pop —
    // and a shared subexpression is emitted once rather than once per use.
    let mut slots: HashMap<ValueId, Slot> = HashMap::new();

    // The index value: the invocation id's x component. It is an **integer**
    // whatever the fragment's parameter leaves say, because this target's index
    // is the invocation id rather than a value of the fragment's domain — a
    // parallel fragment's leaves are `(config, index)` and both are integers
    // however its buffers are classed.
    //
    // An integer module widens it to the fragment's 64-bit `Int`, which is the
    // scalar a body may then use it as (`out[i] = i + i`); an index is never
    // negative, so the widening is exact and the CPU path's index parameter is
    // the same value. A float module leaves it 32-bit: that is all an access
    // chain's index takes, and it keeps a 64-bit integer — and its capability —
    // out of a module that has no other use for one.
    let loaded = next;
    next += 1;
    let component = next;
    next += 1;
    code.push(Inst::new(op::LOAD, vec![ids.v3uint, loaded, ids.gid]));
    code.push(Inst::new(
        op::COMPOSITE_EXTRACT,
        vec![ids.uint, component, loaded, 0],
    ));
    let index_value = match class {
        ScalarClass::Int => {
            let widened = next;
            next += 1;
            code.push(Inst::new(
                op::U_CONVERT,
                vec![ids.ulong, widened, component],
            ));
            widened
        }
        ScalarClass::Float => component,
    };

    // The index parameter is the one entry-block parameter this target can place:
    // it is the invocation id, not a value of the fragment's domain. **A parameter
    // is named by which value it is**, so this is a binding rather than an
    // instruction to interpret, and any other parameter is refused where it is
    // read rather than silently given some id.
    if let Some(index_parameter) = fragment.body.parameters().get(index as usize).copied() {
        slots.insert(index_parameter, scalar(index_value, ScalarClass::Int));
    }

    let entry = &fragment.body.blocks[fragment.body.entry];
    for (at, &definition) in entry.instrs.iter().enumerate() {
        let Some(ValueDef::Instr { op, args, .. }) =
            fragment.body.values.get(definition.0 as usize)
        else {
            continue;
        };
        let op = *op;
        // **Every operand is named by the definition that produced it.**
        let operand = |at: usize| -> Result<Slot, SpirvRefusal> {
            let value = args.get(at).ok_or(SpirvRefusal::ResultArity {
                results: fragment.result_classes.len(),
                left: 0,
            })?;
            if let Some(slot) = slots.get(value).copied() {
                return Ok(slot);
            }
            // **A value that is missing because it is a *parameter* has its own
            // refusal.** Only the index parameter is placed above, so reading
            // another one lands here, and it is the one case the language says by
            // name: on this target a buffer is bound as a storage buffer, so there
            // is no value for a domain parameter to hold. Without this the reader
            // refuses with an arity it did not mean, which is a refusal that names
            // the wrong cause.
            if let Some(local) = fragment
                .body
                .parameters()
                .iter()
                .position(|parameter| parameter == value)
            {
                return Err(SpirvRefusal::NonIndexParameter {
                    local: local as u32,
                    at,
                });
            }
            Err(SpirvRefusal::ResultArity {
                results: fragment.result_classes.len(),
                left: 0,
            })
        };
        let instruction = op;
        match instruction {
            KernelInstr::Const(class, value) => {
                // A constant is emitted once per (class, value) no matter how
                // often the body pushes it: SPIR-V requires every id to be
                // defined exactly once. The push takes the class the
                // *instruction* names, which is the reading a value position
                // wants; a position that wants an integer asks the pool for that
                // reading instead.
                let id = literals.get(class, value, &ids, &mut next);
                slots.insert(definition, literal(id, value));
            }
            KernelInstr::Bin(class, operator) => {
                let rhs = operand(1)?;
                let lhs = operand(0)?;
                let operand_class = bin_class(lhs.kind, rhs.kind, class);
                // A comparison is the one operator whose operands may not be
                // scalars — `(a < b) == c` compares the *scalar* a comparison
                // means — and the one whose result is not one. Every other
                // operator takes and produces a scalar of the class its operands
                // are, which is the integer class when an index and a literal
                // meet and the value class otherwise.
                let lhs = as_class(
                    lhs,
                    operand_class,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let rhs = as_class(
                    rhs,
                    operand_class,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let result = next;
                next += 1;
                // The last element of each row says the operands are compared as
                // **bit patterns**: the language's `==`/`!=` over two values
                // route through `ValueExt::value_eq`, which for a float compares
                // `to_bits`, so `0.0 == -0.0` is `0` and `NaN == NaN` is `1`
                // (`docs/notes/floating-point.md` §3.7). `OpFOrdEqual` is the
                // trap here — right for IEEE and wrong for this language — so a
                // float equality reinterprets both operands and compares the
                // integers, which is `to_bits` exactly.
                let (opcode, result_type, result_kind, as_bits) = match (operand_class, operator) {
                    // An `Int` is unsigned: `OpSDiv`/`OpSRem` would agree below
                    // 2^63 and differ above, silently.  `OpUMod` is the remainder
                    // the language's `%` means (`OpSRem` rounds toward zero,
                    // which is a different function altogether).
                    (ScalarClass::Int, KernelBin::Add) => {
                        (op::I_ADD, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::Sub) => {
                        (op::I_SUB, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::Mul) => {
                        (op::I_MUL, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::Div) => {
                        (op::U_DIV, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::Rem) => {
                        (op::U_MOD, ids.integer(), INT_VALUE, false)
                    }
                    // A comparison yields a bool on this target, which is exactly
                    // what `Select` consumes — the wasm backend's widening to a
                    // scalar has no counterpart here, and no cost.
                    (ScalarClass::Int, KernelBin::Lt) => {
                        (op::U_LESS_THAN, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Int, KernelBin::Gt) => {
                        (op::U_GREATER_THAN, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Int, KernelBin::Leq) => {
                        (op::U_LESS_THAN_EQUAL, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Int, KernelBin::Geq) => (
                        op::U_GREATER_THAN_EQUAL,
                        ids.boolean,
                        Kind::Condition,
                        false,
                    ),
                    (ScalarClass::Int, KernelBin::Eq) => {
                        (op::I_EQUAL, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Int, KernelBin::Neq) => {
                        (op::I_NOT_EQUAL, ids.boolean, Kind::Condition, false)
                    }
                    // The bitwise operators, which over two comparison results
                    // are the language's `and`/`xor`/`or` — the place a
                    // comparison's scalar materialisation is actually paid for.
                    (ScalarClass::Int, KernelBin::BitAnd) => {
                        (op::BITWISE_AND, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::BitOr) => {
                        (op::BITWISE_OR, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Int, KernelBin::BitXor) => {
                        (op::BITWISE_XOR, ids.integer(), INT_VALUE, false)
                    }
                    (ScalarClass::Float, KernelBin::Add) => {
                        (op::F_ADD, ids.float, FLOAT_VALUE, false)
                    }
                    (ScalarClass::Float, KernelBin::Sub) => {
                        (op::F_SUB, ids.float, FLOAT_VALUE, false)
                    }
                    (ScalarClass::Float, KernelBin::Mul) => {
                        (op::F_MUL, ids.float, FLOAT_VALUE, false)
                    }
                    // **`OpFDiv` plainly, and no guard.** A zero divisor is
                    // undefined here and IEEE on wasm, and that divergence is the
                    // recorded price of admitting floats at all: the language does
                    // not specify a kernel's float division and does not promise
                    // one (`docs/notes/floating-point.md` §4.4).
                    (ScalarClass::Float, KernelBin::Div) => {
                        (op::F_DIV, ids.float, FLOAT_VALUE, false)
                    }
                    (ScalarClass::Float, KernelBin::Lt) => {
                        (op::F_LESS_THAN, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Float, KernelBin::Gt) => {
                        (op::F_GREATER_THAN, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Float, KernelBin::Leq) => {
                        (op::F_LESS_THAN_EQUAL, ids.boolean, Kind::Condition, false)
                    }
                    (ScalarClass::Float, KernelBin::Geq) => (
                        op::F_GREATER_THAN_EQUAL,
                        ids.boolean,
                        Kind::Condition,
                        false,
                    ),
                    (ScalarClass::Float, KernelBin::Eq) => {
                        (op::I_EQUAL, ids.boolean, Kind::Condition, true)
                    }
                    (ScalarClass::Float, KernelBin::Neq) => {
                        (op::I_NOT_EQUAL, ids.boolean, Kind::Condition, true)
                    }
                    (ScalarClass::Float, KernelBin::Rem) => {
                        return Err(SpirvRefusal::UnsupportedFloatOperator {
                            operator: "% (remainder)",
                            at,
                        });
                    }
                    (ScalarClass::Float, KernelBin::BitAnd) => {
                        return Err(SpirvRefusal::UnsupportedFloatOperator {
                            operator: "& (bitwise and)",
                            at,
                        });
                    }
                    (ScalarClass::Float, KernelBin::BitOr) => {
                        return Err(SpirvRefusal::UnsupportedFloatOperator {
                            operator: "| (bitwise or)",
                            at,
                        });
                    }
                    (ScalarClass::Float, KernelBin::BitXor) => {
                        return Err(SpirvRefusal::UnsupportedFloatOperator {
                            operator: "^ (bitwise exclusive or)",
                            at,
                        });
                    }
                };
                if as_bits {
                    let left_bits = next;
                    next += 1;
                    let right_bits = next;
                    next += 1;
                    code.push(Inst::new(op::BITCAST, vec![ids.uint, left_bits, lhs.id]));
                    code.push(Inst::new(op::BITCAST, vec![ids.uint, right_bits, rhs.id]));
                    code.push(Inst::new(
                        opcode,
                        vec![result_type, result, left_bits, right_bits],
                    ));
                } else {
                    code.push(Inst::new(opcode, vec![result_type, result, lhs.id, rhs.id]));
                }
                slots.insert(
                    definition,
                    Slot {
                        id: result,
                        kind: result_kind,
                    },
                );
            }
            // The condition a `select` needs.  **Not a no-op here**: wasm's
            // `i32.wrap_i64` narrows an `i64` condition to the `i32` its
            // `select` takes, and this target's `select` takes a *bool*, so the
            // same instruction is where a condition becomes one.  When the
            // condition is already a comparison's bool — which is what the
            // emitter in `lichen-compute` produces for an `if` — it is a no-op,
            // and that is the case the module docs describe.
            KernelInstr::I32WrapI64 => {
                let popped = operand(0)?;
                slots.insert(
                    definition,
                    as_condition(popped, &ids, &mut literals, &mut code, &mut next),
                );
            }
            KernelInstr::Select => {
                let selector = operand(2)?;
                let otherwise = operand(1)?;
                let then = operand(0)?;
                // The arms are the language's scalars (a `select`'s result type
                // is its arms' type, and a scalar is what a lichen value is),
                // and the selector is the bool `select` takes.
                let then = as_class(
                    then,
                    ids.class,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let otherwise = as_class(
                    otherwise,
                    ids.class,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let selector = as_condition(selector, &ids, &mut literals, &mut code, &mut next);
                let result = next;
                next += 1;
                code.push(Inst::new(
                    op::SELECT,
                    vec![ids.scalar(), result, selector.id, then.id, otherwise.id],
                ));
                slots.insert(definition, scalar(result, ids.class));
            }
            // The language's two class crossings.  **The direction is the
            // instruction's**, and that is the whole reason the IR carries the
            // pair: `Int → Float` and `Float → Int` have the same operand shape,
            // so a target that read the direction off the operand would be
            // guessing — and the two targets could guess differently.
            //
            // The operand is the class the conversion is *from*, and a literal or
            // a comparison's `0`/`1` is materialised into it here — the same two
            // positions the rest of this emitter decides a class at.
            KernelInstr::Conv { from, to } => {
                let (from, to) = (from, to);
                let seen = operand(0)?;
                let seen = as_class(seen, from, &ids, &mut literals, &mut code, &mut next, at)?;
                // A crossing between one class and itself is a reclassification:
                // the value already holds the answer, and nothing is emitted.
                if from == to {
                    slots.insert(definition, seen);
                    continue;
                }
                // **Every crossing the language has is representable here, in
                // both module classes.**  The module declares both element types
                // (see the module docs), so the operand's type always has an id:
                // `Int → Float` is `OpConvertUToF` from the module's integer — the
                // 64-bit one in an integer module, the 32-bit element index in a
                // float one — and `Float → Int` is `OpConvertFToU` back to it.
                // Only the 64-bit integer and its `Int64` capability are
                // conditional, which is why a float module's `Int` data is 32-bit
                // and a value past 2³² diverges from the wasm target — the
                // recorded price (`docs/notes/floating-point.md`).
                let result = next;
                next += 1;
                let opcode = match (from, to) {
                    // The unsigned conversions, because an `Int` is unsigned.
                    (ScalarClass::Int, ScalarClass::Float) => op::CONVERT_U_TO_F,
                    (ScalarClass::Float, ScalarClass::Int) => op::CONVERT_F_TO_U,
                    _ => unreachable!("a same-class crossing returned above"),
                };
                code.push(Inst::new(opcode, vec![ids.type_of(to), result, seen.id]));
                slots.insert(definition, scalar(result, to));
            }
            KernelInstr::BufferReadCall(_) => {
                let element = operand(1)?;
                let position = operand(0)?;
                // An access chain's index is an **integer**, so a float in this
                // position is a fragment asking for a conversion the language
                // does not have, and `as_class` refuses it by name.
                let element = as_class(
                    element,
                    ScalarClass::Int,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let slot = buffer_slot(position, at, 0, binding.inputs, "input")?;
                // **A read yields that buffer's element class**, so the value's kind
                // and the type the access chain reaches through are both the
                // buffer's own — which is what lets one module read an `Int`
                // buffer and a `Float` one.
                let element_class = buffer_class_of(fragment, slot, ids.class);
                let chain = ids.chain_of(element_class);
                let pointer = next;
                next += 1;
                let loaded = next;
                next += 1;
                // Three indices: the buffer struct's only member, then the element
                // within that runtime array, then the element struct's only member.
                code.push(Inst::new(
                    op::ACCESS_CHAIN,
                    vec![
                        chain.ptr_elem,
                        pointer,
                        ids.buffers + slot as u32,
                        ids.zero,
                        element.id,
                        ids.zero,
                    ],
                ));
                code.push(Inst::new(
                    op::LOAD,
                    vec![ids.type_of(element_class), loaded, pointer],
                ));
                slots.insert(definition, scalar(loaded, element_class));
            }
            KernelInstr::BufferWriteCall(_) => {
                let value = operand(2)?;
                let element = operand(1)?;
                let position = operand(0)?;
                let slot = buffer_slot(position, at, binding.inputs, binding.outputs, "output")?;
                // **A write stores that buffer's element class**, whichever class
                // the body computed the value in — so a value crossing into a
                // `Float` buffer is materialised into `Float` here, and a value of
                // the *other* class is refused rather than converted.
                let element_class = buffer_class_of(fragment, slot, ids.class);
                let chain = ids.chain_of(element_class);
                let value = as_class(
                    value,
                    element_class,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let element = as_class(
                    element,
                    ScalarClass::Int,
                    &ids,
                    &mut literals,
                    &mut code,
                    &mut next,
                    at,
                )?;
                let pointer = next;
                next += 1;
                code.push(Inst::new(
                    op::ACCESS_CHAIN,
                    vec![
                        chain.ptr_elem,
                        pointer,
                        ids.buffers + slot as u32,
                        ids.zero,
                        element.id,
                        ids.zero,
                    ],
                ));
                code.push(Inst::new(op::STORE, vec![pointer, value.id]));
            }
            KernelInstr::CallKernel(kernel) => {
                return Err(SpirvRefusal::CrossKernelCall { kernel, at });
            }
        }
    }

    // **The returned values are the terminator's list**, so a body's result arity is
    // a fact of the body rather than of whatever happened to be left over.
    let returned = match &entry.terminator {
        Terminator::Return { values } => values.clone(),
        _ => {
            return Err(SpirvRefusal::ControlFlow {
                detail: "a straight-line body ends in a return, and this one does not".to_string(),
            });
        }
    };
    if returned.len() != 1 {
        return Err(SpirvRefusal::ResultArity {
            results: fragment.result_classes.len(),
            left: returned.len(),
        });
    }
    // And the id that result names must have been emitted, which is the same
    // check its arity is: a body whose return is not one of its own values has
    // produced nothing to hand back.
    if returned
        .first()
        .is_none_or(|value| !slots.contains_key(value))
    {
        return Err(SpirvRefusal::ResultArity {
            results: fragment.result_classes.len(),
            left: 0,
        });
    }

    Ok(assemble(
        fragment,
        &ids,
        binding,
        &classes,
        needs_int64(fragment)?,
        &literals.declarations,
        &code,
        next,
    ))
}

/// Write the module in SPIR-V's required section order.
///
/// The order is not a style choice: the entry point must precede the function it
/// names, annotations must precede the types and variables they decorate, and
/// types must precede their use. All of that is legal only because every id was
/// reserved up front.
fn assemble(
    fragment: &KernelFragment,
    ids: &Ids,
    binding: Binding,
    classes: &[ScalarClass],
    needs_int64: bool,
    literals: &[Inst],
    code: &[Inst],
    bound: u32,
) -> Vec<u32> {
    let mut out = vec![SPIRV_MAGIC, SPIRV_VERSION, GENERATOR, bound, 0];

    let emit_all = |out: &mut Vec<u32>, instructions: &[Inst]| {
        for instruction in instructions {
            instruction.encode(out);
        }
    };

    // 1. Capabilities. `Int64` only where the module has a 64-bit integer: a
    // float module's integers are 32-bit element indices and its scalar is
    // `Float32`, which is core, so declaring the capability there would demand a
    // device feature for a type the module never uses — and `needs_int64` says
    // exactly what is declared here.
    let mut capabilities = vec![Inst::new(op::CAPABILITY, vec![capability::SHADER])];
    if ids.class == ScalarClass::Int {
        capabilities.push(Inst::new(op::CAPABILITY, vec![capability::INT64]));
    }
    emit_all(&mut out, &capabilities);

    // 2. The single memory model.
    emit_all(
        &mut out,
        &[Inst::new(
            op::MEMORY_MODEL,
            vec![ADDRESSING_MODEL_LOGICAL, MEMORY_MODEL_GLSL450],
        )],
    );

    // 3. Entry point, and its execution mode. The interface lists the globals
    // the shader reads from the invocation: SPIR-V 1.4 narrowed this list to
    // Input/Output variables, which is what the validator enforces. Storage
    // buffers are reached through the descriptor set instead and are named by
    // their `Binding` decorations.
    let mut interface = vec![EXECUTION_MODEL_GL_COMPUTE, ids.main];
    interface.extend(spv_string(b"main"));
    interface.push(ids.gid);
    emit_all(&mut out, &[Inst::new(op::ENTRY_POINT, interface)]);
    emit_all(
        &mut out,
        &[Inst::new(
            op::EXECUTION_MODE,
            vec![ids.main, EXECUTION_MODE_LOCAL_SIZE, LOCAL_SIZE_X, 1, 1],
        )],
    );

    // 4. Annotations. Decorating a type or variable declared in the *next*
    // section is legal, and is why `OpDecorate` is a separate section at all.
    //
    // The stride is the **buffer's own** element width — eight bytes for the
    // integer ABI's `i64`, four for an `f32` — and it is the *only* place the
    // module states it: the runtime array it decorates is the buffer a dispatch
    // binds. **One stride per class in use**, because a module that binds an
    // `Int` buffer and a `Float` buffer declares both and a dispatch reads the
    // stride off the buffer it is binding rather than off the module.
    let mut annotations = Vec::new();
    for used in classes {
        let types = ids.chain_of(*used);
        annotations.push(Inst::new(
            op::DECORATE,
            vec![types.array, decoration::ARRAY_STRIDE, element_stride(*used)],
        ));
        // A storage buffer's struct member needs its byte offset, and the sole
        // member sits at zero.
        annotations.push(Inst::new(
            op::MEMBER_DECORATE,
            vec![types.elem, 0, decoration::OFFSET, 0],
        ));
        // …and the struct that *contains* the runtime array must say so, or the
        // module does not describe a block-backed resource at all. A `Block`
        // struct has to be explicitly laid out, so its member needs an offset too.
        annotations.push(Inst::new(
            op::DECORATE,
            vec![types.buffer_struct, decoration::BLOCK],
        ));
        annotations.push(Inst::new(
            op::MEMBER_DECORATE,
            vec![types.buffer_struct, 0, decoration::OFFSET, 0],
        ));
    }
    for slot in 0..binding.total() {
        let variable = ids.buffers + slot as u32;
        annotations.push(Inst::new(
            op::DECORATE,
            vec![variable, decoration::DESCRIPTOR_SET, 0],
        ));
        annotations.push(Inst::new(
            op::DECORATE,
            vec![variable, decoration::BINDING, slot as u32],
        ));
    }
    annotations.push(Inst::new(
        op::DECORATE,
        vec![ids.gid, decoration::BUILT_IN, BUILT_IN_GLOBAL_INVOCATION_ID],
    ));
    emit_all(&mut out, &annotations);

    // 5. Types, then the constants (module-scope, so the body's literals belong
    // here and not in the function), then the globals. The order within the
    // section is the spec's: types, constants, global variables.
    //
    // The element struct's only member is **that buffer's own scalar**, which is
    // where "does this buffer hold floats" is decided — per buffer, not per
    // module.
    let mut types = vec![
        Inst::new(op::TYPE_VOID, vec![ids.void]),
        Inst::new(op::TYPE_BOOL, vec![ids.boolean]),
    ];
    // **Both scalar types are declared in every module**, because a body may hold
    // values of either class and cross between them through `Conv`. `Float32` is
    // core SPIR-V and carries no capability, so it is unconditional; the 64-bit
    // integer is what costs `Int64`, and it is declared whenever **anything** in
    // the fragment is 64-bit — an integer buffer counts, which is the difference
    // a mixed fragment makes (`needs_int64` asks the same question).
    types.push(Inst::new(op::TYPE_FLOAT, vec![ids.float, 32]));
    if needs_int64 {
        types.push(Inst::new(op::TYPE_INT, vec![ids.ulong, 64, 0]));
    }
    types.push(Inst::new(op::TYPE_INT, vec![ids.uint, 32, 0]));
    types.push(Inst::new(op::TYPE_VECTOR, vec![ids.v3uint, ids.uint, 3]));
    // One storage-buffer chain per class in use, each closing over its own scalar.
    for used in classes {
        let chain = ids.chain_of(*used);
        let scalar = ids.type_of(*used);
        types.extend([
            Inst::new(op::TYPE_STRUCT, vec![chain.elem, scalar]),
            Inst::new(op::TYPE_RUNTIME_ARRAY, vec![chain.array, chain.elem]),
            Inst::new(op::TYPE_STRUCT, vec![chain.buffer_struct, chain.array]),
            Inst::new(
                op::TYPE_POINTER,
                vec![
                    chain.ptr_array,
                    storage_class::STORAGE_BUFFER,
                    chain.buffer_struct,
                ],
            ),
            Inst::new(
                op::TYPE_POINTER,
                vec![chain.ptr_elem, storage_class::STORAGE_BUFFER, scalar],
            ),
        ]);
    }
    types.extend([
        Inst::new(
            op::TYPE_POINTER,
            vec![ids.ptr_in, storage_class::INPUT, ids.v3uint],
        ),
        Inst::new(op::TYPE_FUNCTION, vec![ids.fn_ty, ids.void]),
    ]);
    emit_all(&mut out, &types);

    let mut constants = vec![Inst::new(op::CONSTANT, vec![ids.uint, ids.zero, 0])];
    // The scalar `1` and `0` a comparison is materialised into — `1.0`/`0.0`
    // over two floats, the integer pair over the module's integer (`i64` in an
    // integer module, `u32` in a float one). Declared unconditionally with the
    // other constants — `OpConstant` is module-scope, and an unused constant is
    // legal — because which body needs them is known only after the walk above.
    //
    // **Both pairs, because both element types are declared in every module**
    // (see the module docs): a comparison materialised into a float position is
    // reachable in an integer module too. The integer `0` a float module needs is
    // `ids.zero` above, so only its `1` is declared here.
    match ids.class {
        ScalarClass::Int => {
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.ulong, ids.one_integer, 1, 0],
            ));
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.ulong, ids.zero_integer, 0, 0],
            ));
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.float, ids.one_float, 1.0f32.to_bits()],
            ));
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.float, ids.zero_float, 0.0f32.to_bits()],
            ));
        }
        ScalarClass::Float => {
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.float, ids.one_float, 1.0f32.to_bits()],
            ));
            constants.push(Inst::new(
                op::CONSTANT,
                vec![ids.float, ids.zero_float, 0.0f32.to_bits()],
            ));
            constants.push(Inst::new(op::CONSTANT, vec![ids.uint, ids.one_integer, 1]));
        }
    }
    constants.extend(literals.iter().map(|literal| Inst {
        opcode: literal.opcode,
        operands: literal.operands.clone(),
    }));
    emit_all(&mut out, &constants);

    let mut globals = vec![Inst::new(
        op::VARIABLE,
        vec![ids.ptr_in, ids.gid, storage_class::INPUT],
    )];
    for slot in 0..binding.total() {
        // **A buffer's variable is typed with its own class's block struct.** This
        // is the line that makes a mixed module bindable: a dispatch binds a
        // descriptor against the type the shader declares for that binding, so the
        // `Int` variable must be the `Int` chain and the `Float` variable the
        // `Float` one.
        let chain = ids.chain_of(buffer_class_of(fragment, slot, ids.class));
        globals.push(Inst::new(
            op::VARIABLE,
            vec![
                chain.ptr_array,
                ids.buffers + slot as u32,
                storage_class::STORAGE_BUFFER,
            ],
        ));
    }
    emit_all(&mut out, &globals);

    // 6. The function, its body, and its end.
    emit_all(
        &mut out,
        &[
            Inst::new(op::FUNCTION, vec![ids.void, ids.main, 0, ids.fn_ty]),
            Inst::new(op::LABEL, vec![ids.label]),
        ],
    );
    emit_all(&mut out, code);
    emit_all(
        &mut out,
        &[
            Inst::new(op::RETURN, vec![]),
            Inst::new(op::FUNCTION_END, vec![]),
        ],
    );
    out
}

/// The class a binary operator runs over, from its two operands.
///
/// A literal follows the other operand: `Const(1)` beside the index is the
/// integer `1` and beside a float is the `f32` whose bits are `1`. With **both**
/// operands literals nothing else can decide, so the module's own class does —
/// which is what makes `2.0 * 3.0` float arithmetic in a float module. The price
/// is that a *pure constant* expression used as an index (`1 + 2`, say) reads as
/// arithmetic over two bit patterns and is refused at the position that wanted
/// an index. Refusing beats the alternative: emitting integer arithmetic over
/// the same payloads would silently be a different number in the other case.
fn bin_class(lhs: Kind, rhs: Kind, module: ScalarClass) -> ScalarClass {
    match (lhs, rhs) {
        (Kind::Scalar(class), _) => class,
        (_, Kind::Scalar(class)) => class,
        _ => module,
    }
}

/// `slot` as a value of `class`, emitting what the change needs.
///
/// Three cases, each a fact about the module rather than a preference:
///
/// * a [`Kind::Literal`] becomes a constant of `class` — `Const` is the one IR
///   instruction whose class the position decides, and [`Literals`] emits each
///   reading of it once;
/// * a [`Kind::Condition`] becomes that class's `1`/`0`, through `OpSelect` over
///   the two constants: there is no `OpConvertBoolToInt` (a bool's stored form
///   is not defined), and over two floats the scalar the language means is
///   `1.0`/`0.0` rather than an integer select;
/// * a [`Kind::Scalar`] of the *other* class has no conversion at all. `Int` and
///   `Float` do not convert in either direction
///   (`docs/notes/floating-point.md` §4.2), so a fragment that asks for one is
///   refused by name rather than turned into a different number.
fn as_class(
    slot: Slot,
    class: ScalarClass,
    ids: &Ids,
    literals: &mut Literals,
    code: &mut Vec<Inst>,
    next: &mut u32,
    at: usize,
) -> Result<Slot, SpirvRefusal> {
    match slot.kind {
        Kind::Scalar(seen) if seen == class => Ok(slot),
        Kind::Scalar(_) => Err(SpirvRefusal::MixedClasses { at }),
        Kind::Literal(value) => Ok(scalar(literals.get(class, value, ids, next), class)),
        Kind::Condition => {
            let result = *next;
            *next += 1;
            code.push(Inst::new(
                op::SELECT,
                vec![
                    ids.type_of(class),
                    result,
                    slot.id,
                    ids.one_of(class),
                    ids.zero_of(class),
                ],
            ));
            Ok(scalar(result, class))
        }
    }
}

/// The `bool` a `select` takes for `slot`, converting a scalar.
///
/// **Non-zero is true**, which is what wasm's `select` means by its `i32`
/// condition — the instruction this one stands in for (`I32WrapI64`) narrows
/// there and converts here, so the two targets agree on what a condition is.
///
/// # For a float, the bit pattern is what is tested
///
/// "The bit pattern is non-zero" is **not** the same question as "the value is
/// not zero": `-0.0` has a non-zero bit pattern, and a `NaN` — whose bit pattern
/// is non-zero by definition — compares unequal to everything, so
/// `OpFOrdNotEqual x, 0.0` answers *false* for it and would send a `NaN` selector
/// down the else arm. So a float is **reinterpreted** as its 32 bits
/// (`OpBitcast`: same width, the same bits, no conversion of the value) and
/// compared as an integer, which is the bit pattern exactly. A literal's payload
/// is already the pattern — it is the `f32`'s bits — so it is compared in its
/// integer reading, with no instruction spent reinterpreting it.
fn as_condition(
    slot: Slot,
    ids: &Ids,
    literals: &mut Literals,
    code: &mut Vec<Inst>,
    next: &mut u32,
) -> Slot {
    match slot.kind {
        Kind::Condition => slot,
        Kind::Scalar(ScalarClass::Int) => {
            let result = *next;
            *next += 1;
            code.push(Inst::new(
                op::I_NOT_EQUAL,
                vec![ids.boolean, result, slot.id, ids.zero_of(ScalarClass::Int)],
            ));
            condition(result)
        }
        Kind::Scalar(ScalarClass::Float) => {
            let bits = *next;
            *next += 1;
            let result = *next;
            *next += 1;
            // The bit pattern is compared in its **32-bit** reading, so the zero
            // is `ids.zero` and not the integer class's: in an integer module that
            // class is 64-bit, and `OpINotEqual` over a `uint` and a `ulong` is an
            // invalid instruction rather than a wide comparison. `ids.zero` is the
            // 32-bit `0` every module declares.
            code.push(Inst::new(op::BITCAST, vec![ids.uint, bits, slot.id]));
            code.push(Inst::new(
                op::I_NOT_EQUAL,
                vec![ids.boolean, result, bits, ids.zero],
            ));
            condition(result)
        }
        Kind::Literal(value) => {
            let bits = literals.get(ScalarClass::Int, value, ids, next);
            let result = *next;
            *next += 1;
            code.push(Inst::new(
                op::I_NOT_EQUAL,
                vec![ids.boolean, result, bits, ids.zero_of(ScalarClass::Int)],
            ));
            condition(result)
        }
    }
}

/// Resolve a buffer position into a slot in the module's buffer list.
///
/// **Reads and writes address different position spaces.** A read names a
/// *config* position, counting from `base` over the inputs; a write names its
/// *emission ordinal*, counting from `base` over the outputs. The two spaces are
/// distinct facts of the IR — `out_pos` is the index function's codomain
/// position, not an index into one combined list — so a write's position `0` is
/// the first **output**, never the first input.
fn buffer_slot(
    position: Slot,
    at: usize,
    base: usize,
    count: usize,
    space: &'static str,
) -> Result<usize, SpirvRefusal> {
    let value = position
        .constant()
        .ok_or(SpirvRefusal::NonConstantBufferPosition { at })?;
    if value < 0 || value as usize >= count {
        return Err(SpirvRefusal::BufferPositionOutOfRange {
            position: value,
            space,
            bound: count,
            at,
        });
    }
    Ok(base + value as usize)
}

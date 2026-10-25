//! The SPIR-V emitter: a [`KernelFragment`] to a SPIR-V compute module.
//!
//! # Invariant
//! A body's output elements are written where the body's own code writes them, a loop
//! body included: the ruling withdrew the refusal a loop write relied on, so a lane
//! that never runs the loop leaves its element as the allocation held
//! (`docs/notes/loop-conversion.md` §6). Why this is hand-written, and where it
//! disagrees with the wasm backend: `docs/notes/lichen-compute-gpu.md`.

use std::collections::{HashMap, HashSet};
use std::fmt;

use lichen_kernel_ir::{
    KernelBin, KernelFragment, KernelId, KernelInstr, LaunchSet, ScalarClass, Slice, Space,
    Terminator, ValueDef, ValueId,
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
    pub const TYPE_ARRAY: u16 = 28;
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
    pub const FUNCTION_PARAMETER: u16 = 55;
    pub const FUNCTION_END: u16 = 56;
    /// The cross-kernel call, into a function this module declares.
    pub const FUNCTION_CALL: u16 = 57;
    /// A float negation, which is exact and needs no decoration of its own.
    pub const FP_NEGATE: u16 = 126;
    pub const PHI: u16 = 245;
    pub const LOOP_MERGE: u16 = 246;
    pub const SELECTION_MERGE: u16 = 247;
    pub const LABEL: u16 = 248;
    pub const BRANCH: u16 = 249;
    pub const BRANCH_CONDITIONAL: u16 = 250;
    pub const RETURN: u16 = 253;
    /// A function that leaves a value behind, which is how every function but
    /// the entry point ends.
    pub const RETURN_VALUE: u16 = 254;
    pub const U_CONVERT: u16 = 113;
    /// `OpConvertFToU` (109) — the `float2int` crossing, truncating toward zero.
    pub const CONVERT_F_TO_U: u16 = 109;
    /// `OpConvertUToF` (112) — the `int2float` crossing.
    ///
    /// # Invariant
    /// The unsigned source is the language's `Int`, so this is the conversion and not `OpConvertSToF`:
    /// the two read the same bits and answer different numbers past the signed range.
    pub const CONVERT_U_TO_F: u16 = 112;
    /// The reinterpretation an integer constant and a float operand meet through.
    pub const BITCAST: u16 = 124;
    /// `OpShiftLeftLogical` (196) — a widening half's bits up into a float word.
    pub const SHIFT_LEFT_LOGICAL: u16 = 196;
    /// `OpShiftRightLogical` (194) — the same shift back down, narrowing.
    pub const SHIFT_RIGHT_LOGICAL: u16 = 194;
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
    /// The ordered float comparisons: false when either operand is `NaN`.
    pub const F_LESS_THAN: u16 = 184;
    pub const F_GREATER_THAN: u16 = 186;
    pub const F_LESS_THAN_EQUAL: u16 = 188;
    pub const F_GREATER_THAN_EQUAL: u16 = 190;
    pub const BITWISE_OR: u16 = 197;
    pub const BITWISE_XOR: u16 = 198;
    pub const BITWISE_AND: u16 = 199;
    /// The import that names an extended instruction set, ahead of `OpMemoryModel`.
    ///
    /// # Invariant
    /// **The first instruction of a module that reaches for an extended set, and the import is
    /// never optional**: `OpExtension` is not a substitute, and `spirv-val` answers `OpExtInst set
    /// Id 1 does not reference an OpExtInstImport result Id` without one.
    pub const EXT_INST_IMPORT: u16 = 11;
    /// An instruction of an imported set, named by its id rather than by an opcode of its own.
    pub const EXT_INST: u16 = 12;
    /// The extension an extended instruction is registered under, ahead of the capabilities'
    /// dependent instructions.
    ///
    /// # Invariant
    /// **Between the capabilities and the extended-set imports**: SPIR-V orders the header's
    /// sections, and `spirv-val` answers `Missing required extension '…'` for an opcode whose
    /// extension the module never named — the capability is not a substitute for this one.
    pub const EXTENSION: u16 = 10;
    /// The hardware integer atomic add: a read-modify-write the device performs itself.
    ///
    /// # Invariant
    /// **A primitive and not a shape this emitter expands**: the language's `write_atomic` on an
    /// `Int` is this instruction, and a load/add/store expansion would answer a contested scatter
    /// with a lost update — the program asked for a primitive and the expansion is a different one.
    pub const ATOMIC_I_ADD: u16 = 234;
    /// `OpAtomicFAddEXT`, the same read-modify-write over a `Float` element.
    ///
    /// # Invariant
    /// An **extended instruction**, so it carries `AtomicFloat32AddEXT` and the
    /// `SPV_EXT_shader_atomic_float_add` extension; `spirv-val` refuses it without both. There is
    /// no core float atomic: SPIR-V 1.0 added integers only.
    pub const ATOMIC_F_ADD: u16 = 6035;
    /// The workgroup barrier: no invocation of the workgroup passes before every one arrives.
    pub const CONTROL_BARRIER: u16 = 224;
    /// The memory barrier: the memory accesses of the scope are ordered against each other.
    pub const MEMORY_BARRIER: u16 = 225;
    /// The read of another lane's value through the exchange network, at subgroup scope.
    pub const GROUP_NON_UNIFORM_SHUFFLE: u16 = 345;
}

/// `SpvCapabilityShader`, `SpvCapabilityInt64`, `SpvCapabilityGroupNonUniformShuffle`.
mod capability {
    pub const SHADER: u32 = 1;
    pub const INT64: u32 = 11;
    /// `SpvCapabilityInt64Atomics`: an `OpAtomicIAdd` over the module's 64-bit integer.
    ///
    /// # Invariant
    /// **`Int64` alone is not enough**: `OpAtomicIAdd` over a 64-bit integer is the one 64-bit
    /// operation the type capability does not cover, so `spirv-val` answers `Opcode 234 requires
    /// one of these capabilities: Int64Atomics`. Declared only where a body emitted one, so a
    /// module that merely *has* a 64-bit integer asks no device for `shaderBufferInt64Atomics`.
    pub const INT64_ATOMICS: u32 = 12;
    /// `SpvCapabilityAtomicFloat32AddEXT`: `OpAtomicFAddEXT` over a 32-bit float.
    ///
    /// # Invariant
    /// **Its own capability rather than `Float32`**: the core float capability covers arithmetic,
    /// and the atomic add is an extended instruction with its own gate, which the Khronos grammar
    /// exprs with `SPV_EXT_shader_atomic_float_add`.
    pub const ATOMIC_FLOAT32_ADD: u32 = 6033;
    /// The 16-bit integer a brain float is stored as.
    ///
    /// # Invariant
    /// Declared **only** when a `BFloat16` buffer is in use: `spirv-val` refuses the
    /// `OpTypeInt 16 0` itself without it, and every module paying for it asks a device for
    /// 16-bit types a kernel that never stores one does not need.
    ///
    /// `StorageBuffer16BitAccess` is deliberately **not** declared: it is a Vulkan *device
    /// feature*, which the module does not carry.
    pub const INT16: u32 = 22;
    /// The exchange network a `Warp` slice reads through.
    ///
    /// # Invariant
    /// Its own capability and not `GroupNonUniform`: `OpGroupNonUniformShuffle` is refused without
    /// it, so a module declaring the parent alone would validate its absence as a missing one only
    /// at the shuffle — or, before that, at the validator.
    pub const GROUP_NON_UNIFORM_SHUFFLE: u32 = 65;
}

/// `SpvStorageClass` values used below.
mod storage_class {
    pub const INPUT: u32 = 1;
    /// The workgroup's own memory: what one lane writes, its siblings read.
    pub const WORKGROUP: u32 = 4;
    pub const PUSH_CONSTANT: u32 = 9;
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
    /// `NoContraction`, core SPIR-V since 1.0 under capability `Shader`.
    pub const NO_CONTRACTION: u32 = 42;
}

/// `SpvBuiltInGlobalInvocationId`.
const BUILT_IN_GLOBAL_INVOCATION_ID: u32 = 28;
/// `SpvScopeWorkgroup` and `SpvMemorySemanticsWorkgroupMemory`.
///
/// # Invariant
/// Both are **values the barrier's operands name as ids**, not literals: at SPIR-V 1.3 the scope and
/// semantics operands are `OpConstant` ids, so each needs its own instruction. Workgroup scope and
/// workgroup-memory semantics are one fact about shared memory — the lanes of a workgroup, and the
/// storage they exchange through (`docs/notes/shared-memory-and-barriers.md` §4).
const SCOPE_WORKGROUP: u32 = 2;
/// `SpvScopeDevice`: the scope an atomic add's memory operand is ordered at.
///
/// # Invariant
/// **Device, not workgroup**: the contention a `write_atomic` answers is between every invocation
/// of the whole dispatch, and a workgroup scope would order the add against one workgroup's lanes
/// while leaving another workgroup's racing write unconstrained. Workgroup scope would be enough
/// for the barrier, which is why it is a different constant.
const SCOPE_DEVICE: u32 = 1;
/// `SpvScopeSubgroup`: the scope a `Warp` exchange's barrier is ordered at.
const SCOPE_SUBGROUP: u32 = 3;
const MEMORY_SEMANTICS_WORKGROUP_MEMORY: u32 = 0x100;
/// `AcquireRelease | WorkgroupMemory`, the semantics a `Warp` expr's barrier is ordered at.
///
/// # Invariant
/// **The acquire/release bits are what make the exchange sound and are not the workgroup
/// barrier's**: the staged value is a register, so ordering it is an acquire/release, and
/// `WorkgroupMemory` alone orders shared memory — which a `Warp` slice has none of. It stays the
/// memory scope because that must not be *narrower* than the execution scope, and `Subgroup` is
/// narrower than it.
const MEMORY_SEMANTICS_ACQUIRE_RELEASE_WORKGROUP_MEMORY: u32 = 0x108;
/// `AcquireRelease`, the semantics a staged **store**'s barrier is ordered at.
///
/// # Invariant
/// **Acquire and release, and no memory class**: the store a staged body keeps is a storage
/// buffer's, and `WorkgroupMemory` orders the shared memory only — so the barrier names every
/// class, and the scope that carries it is the workgroup's, because a device-scope fence needs a
/// memory model this module does not declare.
const MEMORY_SEMANTICS_ACQUIRE_RELEASE: u32 = 0x8;
/// `GLSLstd450Fma`, the one extended instruction this emitter writes.
const GLSL_STD_450_FMA: u32 = 50;
/// The extension `OpAtomicFAddEXT` is registered under in the Khronos grammar.
///
/// # Invariant
/// **The name is the extension's, spelled the way the registry spells it**: the capability
/// `AtomicFloat32AddEXT` names the gate and this names the registration, and `spirv-val` reports
/// a missing one as `Missing required extension 'SPV_EXT_shader_atomic_float_add'`.
const SHADER_ATOMIC_FLOAT_ADD_EXTENSION: &[u8] = b"SPV_EXT_shader_atomic_float_add";
/// The `None` a merge instruction's optional control mask carries.
const CONTROL_NONE: u32 = 0;
/// `SpvExecutionModelGLCompute`, `SpvExecutionModeLocalSize`.
const EXECUTION_MODEL_GL_COMPUTE: u32 = 5;
const EXECUTION_MODE_LOCAL_SIZE: u32 = 17;
/// `SpvAddressingModelLogical`, `SpvMemoryModelGLSL450`.
const ADDRESSING_MODEL_LOGICAL: u32 = 0;
const MEMORY_MODEL_GLSL450: u32 = 1;
/// A non-zero generator magic, as SPIR-V requires.
const GENERATOR: u32 = 0x0030_0000;
/// 1.3 is the first version with the `StorageBuffer` storage class, which lets a
/// buffer be a plain runtime array.
const SPIRV_VERSION: u32 = 0x0001_0300;
const SPIRV_MAGIC: u32 = 0x0723_0203;

/// The bytes one buffer element occupies, per class: the array stride.
///
/// # Invariant
/// Derived, not decided here: what an element occupies is a fact about the class, so a module's
/// `ArrayStride` and the host that hands the buffer over cannot disagree. A literal here would be a
/// second copy of the rule — the drift that made this target read a float buffer at four bytes while
/// the host wrote eight.
fn element_stride(class: ScalarClass) -> u32 {
    class.byte_width() as u32
}

/// The local workgroup size this target schedules.
///
/// # Invariant
/// A run of `count` indices is `ceil(count / LOCAL_SIZE_X)` workgroups, and the entry point's
/// body branches on `index < extent` before any instruction of it, so a surplus lane runs none.
/// It is also the only width this target schedules, so a fragment declaring another is refused by
/// name rather than having its `LocalSize` rounded to this.
pub const LOCAL_SIZE_X: u32 = 64;

/// The lanes one subgroup runs, as this target's device reports them.
///
/// # Invariant
/// The width a `Warp` exchange is legal at, read the way [`LOCAL_SIZE_X`] is: a fact about the
/// device rather than the body, so a slice that does not fit in one is refused by name. **The
/// module cannot state it** — `SubgroupSize` is an execution mode Vulkan 1.3 introduced and this
/// target emits SPIR-V 1.3 — which is why it is the narrow width every subgroup here runs.
pub const SUBGROUP_WIDTH: u32 = 32;

/// Why a fragment cannot be emitted for this target: every variant names its own
/// cause and nothing is approximated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpirvRefusal {
    /// A call to a kernel the supplied launch set does not hold; the message
    /// names which callee is missing.
    CalleeNotInLaunchSet { kernel: KernelId, at: usize },
    /// A call whose argument count is not the callee's own domain.
    CrossKernelArity {
        kernel: KernelId,
        expected: usize,
        given: usize,
        at: usize,
    },
    /// A call to a callee that does not leave exactly one value.
    CrossKernelResults {
        kernel: KernelId,
        results: usize,
        at: usize,
    },
    /// A buffer position that is not a compile-time constant, or a slice ordinal that is not
    /// one.
    NonConstantBufferPosition { at: usize },
    /// A position outside the space it addresses, whether a buffer's or a slice's.
    BufferPositionOutOfRange {
        position: i64,
        space: &'static str,
        bound: usize,
        at: usize,
    },
    /// The fragment places a slice in a space this target cannot honour.
    ///
    /// # Invariant
    /// Refused by name rather than placed in [`Space::Shared`]: a `Register`-placed slice is one
    /// array per lane, so `Workgroup` storage would let a lane see another's elements — a different
    /// program (`docs/notes/shared-memory-and-barriers.md` §5).
    StagingNotOnThisBackend { space: Space, at: usize },
    /// A `Warp`-placed slice that does not hold exactly one element per lane of a subgroup.
    ///
    /// # Invariant
    /// Refused **by name** rather than placed in [`Space::Shared`]: the exchange reads the value a
    /// named lane of the reader's own subgroup staged, so a count that is not one per lane either
    /// names a lane the reader cannot reach or one its siblings also staged — a different program,
    /// not a slower one (`docs/notes/shared-memory-and-barriers.md` §5).
    StagingNotOneElementPerLane {
        /// The slice's element count.
        count: u32,
        /// The lanes one subgroup runs.
        subgroup_width: u32,
        at: usize,
    },
    /// A `Warp` read of a slice this lane never staged a value into.
    ///
    /// # Invariant
    /// The exchange takes each lane's own staged value, so a read with none is a value no lane
    /// ever held — refused by name rather than answered from whatever the register carried
    /// (`docs/notes/shared-memory-and-barriers.md` §5).
    WarpExchangeWithoutAWrite { at: usize },
    /// The fragment declares an integer width this target cannot represent.
    UnsupportedIntWidth { bits: u32 },
    /// The fragment declares a workgroup width this target cannot schedule.
    UnsupportedLaneWidth { lanes: u32 },
    /// An operation over `Float` the language has no form for.
    ///
    /// # Invariant
    /// A `Float` takes `+ - * /` and the four order comparisons and nothing else, so `%` and the
    /// bitwise trio have no opcode: a kernel that runs but computes something nobody wrote is worse
    /// than one that does not run.
    UnsupportedFloatOperator { operator: &'static str, at: usize },
    /// A value in a class this emitter has no SPIR-V type for.
    ///
    /// # Invariant
    /// **The arithmetic gate, and now the only one**: SPIR-V's 16-bit float is an IEEE half, not
    /// a brain float, so nothing rounds in a `BFloat16`'s own width and a `Bin` over the class is
    /// refused by name rather than computed at a width nobody wrote. A `BFloat16` *buffer* is not
    /// this refusal — it is stored as a 16-bit integer and widened to `Float` on the way in.
    UnsupportedClass { class: ScalarClass },
    /// A value and a buffer index of different classes met in one operation.
    MixedClasses {
        at: usize,
        seen: ScalarClass,
        wanted: ScalarClass,
    },
    /// A block parameter's own reads ask it to be two classes at once.
    ///
    /// # Invariant
    /// A phi has one type however its edges were written, so a parameter one read wants `Int` and
    /// another wants `Float` has no type to declare: the classes it was asked to be are carried
    /// rather than the fact alone, because which two met is what tells the fragment's author which
    /// read to fix. A parameter **nothing** reads is not this case — see [`read_class`].
    BlockParameterClassDisagrees {
        /// The parameter, as the body numbers it.
        parameter: ValueId,
        /// The classes its reads asked for, in name order.
        classes: Vec<ScalarClass>,
    },
    /// A `Select`'s two arms are computed in different classes.
    ///
    /// # Invariant
    /// An `OpSelect` has one result type and both arms feed it, so a fragment whose arms disagree
    /// has no type to give the result. The classes are carried rather than the fact alone, because
    /// which two met is what tells the fragment's author which arm to convert; nothing here picks
    /// one arm to type the result by, which would answer a program the body does not state.
    SelectArmClassesDisagree {
        /// The classes the two arms are computed in, in name order.
        classes: Vec<ScalarClass>,
    },
    /// The body computes a transcendental, which this target has no instruction for.
    ///
    /// # Invariant
    /// A transcendental the IR defines is expanded into the primitive instructions, which
    /// `Lower` applies before a fragment is registered, so a body that still holds one was
    /// not built by it. The exponential's reason is this target's GLSL `Exp`, which is
    /// implementation-defined per vendor. `docs/notes/floating-point.md` §6, §7.
    UnexpandedTranscendental { at: usize },
    /// The body does not leave exactly the one value a compute shader needs.
    ResultArity { results: usize, left: usize },
    /// A `write_atomic` whose class no hardware atomic add exists for.
    ///
    /// # Invariant
    /// **Refused rather than emulated, and rather than narrowed**: no extension this target
    /// names has an atomic add at this class's width, and a read-modify-write loop over the
    /// storage integer would answer a different program — one that may lose an update.
    AtomicAddNotAHardwarePrimitive { class: ScalarClass, at: usize },
    /// A body the target has no way to structure.
    ///
    /// # Invariant
    /// Refused rather than emitted: a dropped branch is a fragment that computes a
    /// different program than it was lowered from, and the transfer is already in the
    /// IR, so this is about the emitter rather than about what the language can say.
    ControlFlow { detail: String },
}

impl fmt::Display for SpirvRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpirvRefusal::CalleeNotInLaunchSet { kernel, at } => write!(
                f,
                "instruction {at} calls kernel {kernel}, which the launch set handed to this \
                 emitter does not hold. A cross-kernel call becomes a call into a function this \
                 module declares, so the caller has to assemble the callee into the same set — the \
                 same set the wasm backend links (`docs/notes/lichen-compute-gpu.md`)."
            ),
            SpirvRefusal::CrossKernelArity {
                kernel,
                expected,
                given,
                at,
            } => write!(
                f,
                "instruction {at} calls kernel {kernel}, whose own domain takes {expected} \
                 argument(s), with {given}. The arity is the callee's, not this body's: \
                 `KernelInstr::arity` answers `None` for a call because the IR does not carry it."
            ),
            SpirvRefusal::CrossKernelResults {
                kernel,
                results,
                at,
            } => write!(
                f,
                "instruction {at} calls kernel {kernel}, which leaves {results} value(s). A call \
                 here is typed by the callee's one return value, so a callee that leaves none or \
                 several has no signature to call."
            ),
            SpirvRefusal::NonConstantBufferPosition { at } => write!(
                f,
                "instruction {at} uses a position that is not a compile-time constant. A buffer \
                 position and a slice ordinal are both emission ordinals by construction, so this \
                 fragment is malformed."
            ),
            SpirvRefusal::BufferPositionOutOfRange {
                position,
                space,
                bound,
                at,
            } => write!(
                f,
                "instruction {at} names {space} position {position}, but this module has \
                 {bound} of them."
            ),
            SpirvRefusal::StagingNotOnThisBackend { space, at } => write!(
                f,
                "instruction {at} places a slice in the {space:?} space, which this target does \
                 not emit. `Register` placement is one array per lane and needs `Private` \
                 storage; this emitter declares `Workgroup` storage only, so emitting that here \
                 would let a lane read another's elements — a different program than the \
                 fragment states."
            ),
            SpirvRefusal::StagingNotOneElementPerLane {
                count,
                subgroup_width,
                at,
            } => write!(
                f,
                "instruction {at} stages {count} element(s) in the `Warp` space, and one subgroup \
                 of this target runs {subgroup_width} lane(s). The exchange reads the value a \
                 named lane of the reader's own subgroup staged, so a slice holds exactly one \
                 element per lane: a wider one names a lane the reader cannot reach and a narrower \
                 one a lane its siblings also staged (`docs/notes/shared-memory-and-barriers.md` \
                 §5)."
            ),
            SpirvRefusal::WarpExchangeWithoutAWrite { at } => write!(
                f,
                "instruction {at} reads a `Warp` slice this lane never staged a value into. The \
                 exchange takes each lane's own value rather than a memory the reader may address, \
                 so there is no uninitialised element for a read to name (`docs/notes/shared-\
                 memory-and-barriers.md` §5)."
            ),
            SpirvRefusal::UnsupportedIntWidth { bits } => write!(
                f,
                "the fragment was lowered for {bits}-bit integers, which this target does not \
                 represent."
            ),
            SpirvRefusal::UnsupportedLaneWidth { lanes } => write!(
                f,
                "the fragment was lowered for workgroups of {lanes} lanes, which this target \
                 does not schedule."
            ),
            SpirvRefusal::UnsupportedFloatOperator { operator, at } => write!(
                f,
                "instruction {at} applies {operator} to floats, and the language gives a `Float` \
                 `+ - * /` and the four comparisons and nothing else. There is no float operator \
                 to emit for this, and emitting a different one would compute something the \
                 program does not say."
            ),
            SpirvRefusal::UnsupportedClass { class } => write!(
                f,
                "the body computes in {class:?}, which this emitter has no SPIR-V type for: \
                 SPIR-V's 16-bit float is an IEEE half, not a brain float, so there is nothing to \
                 declare a {class:?} value as and no arithmetic that rounds in its own width."
            ),
            SpirvRefusal::MixedClasses { at, seen, wanted } => write!(
                f,
                "instruction {at} mixed an integer and a float in one operation: a {seen:?} where a {wanted:?} is wanted. `Int` and `Float` \
                 do not convert in either direction, so nothing here can make the two operands \
                 meet: this is a malformed fragment rather than an unsupported shape."
            ),
            SpirvRefusal::BlockParameterClassDisagrees { parameter, classes } => {
                write!(
                    f,
                    "the block parameter {parameter:?} is read as one class in one place and \
                     another in another ({classes:?}). A block parameter is one `OpPhi` and has one \
                     type however its edges were written, and this target does not guess between \
                     them."
                )
            }
            SpirvRefusal::SelectArmClassesDisagree { classes } => {
                write!(
                    f,
                    "a `Select`'s two arms are computed in different classes ({classes:?}). An \
                     `OpSelect` has one result type, `KernelInstr::Select` states no class of its \
                     own, so the only honest answer is the classes the arms are computed in — and \
                     this emitter does not pick one of them to type the result by. Convert one arm, \
                     or write the conditional over values of one class."
                )
            }
            SpirvRefusal::UnexpandedTranscendental { at } => write!(
                f,
                "instruction {at} computes a transcendental, which this target emits only through \
                 its definition in the primitive operators: the lowering applies that definition \
                 before a fragment is registered, so this body was not built by it \
                 (`docs/notes/floating-point.md` §6, §7)."
            ),
            SpirvRefusal::ResultArity { results, left } => write!(
                f,
                "the fragment declares {results} result(s) and its body returns {left} value(s). \
                 A compute shader communicates through its storage buffers, so this backend \
                 requires a fragment whose body returns exactly one value."
            ),
            SpirvRefusal::AtomicAddNotAHardwarePrimitive { class, at } => write!(
                f,
                "instruction {at} adds atomically to a {class:?} buffer, and this target has no \
                 hardware atomic add at that class: `OpAtomicIAdd` covers integers and \
                 `OpAtomicFAddEXT` the 32-bit float, and neither reaches a {class:?}. A \
                 read-modify-write loop over the storage integer would answer a different \
                 program, so this is refused rather than emulated."
            ),
            SpirvRefusal::ControlFlow { detail } => write!(
                f,
                "this emitter cannot structure this body: {detail}. The transfer is in the kernel \
                 IR, so this is the emitter's limit rather than something the program could not say \
                 — see `docs/notes/loop-conversion.md` §8."
            ),
        }
    }
}

/// How many buffers a module binds: a property of the caller's buffers.
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

/// The type a slot's id has, and the value it holds where the IR does not fix it.
///
/// # Invariant
/// The IR is untyped and this target is not: a comparison here yields `OpTypeBool`, while the
/// language says it yields the `0`/`1` scalar, so a bool is the wrong type in a bitwise operand, a
/// comparison's operand, a stored element or a `Select` arm. Each slot records what it holds, and the
/// two coercions are emitted where a position demands the other.
#[derive(Debug, Clone, Copy)]
enum Kind {
    /// A value of the named class.
    Scalar(ScalarClass),
    /// A constant, and the `i64` the body pushed for it.
    ///
    /// # Invariant
    /// The class is not fixed by the IR: the same `Const(0)` is a buffer position in one place and a
    /// float's bit pattern in another, and only the position it is consumed in decides, so the
    /// payload rides here until a position reads it.
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
    /// # Invariant
    /// A buffer operation's *position* selects which storage-buffer variable to reach, so
    /// it is read as a number rather than as an id: a value that was *computed* is not an
    /// ordinal however constant its value is.
    fn constant(self) -> Option<i64> {
        match self.kind {
            Kind::Literal(value) => Some(value),
            _ => None,
        }
    }
}

/// The module-scope constants the body's literals need, one pool per class.
///
/// # Invariant
/// Keyed by class and value, because the two readings of one `i64` are two constants: a float value
/// is its 32 bits, an integer is the number. A position that wants the other class asks again, and
/// the constant is emitted once however many positions use it.
#[derive(Default)]
struct Literals {
    /// The `OpConstant` declarations, emitted in the types-and-constants section.
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
        self.declarations.push(match class {
            ScalarClass::Int => ids.integer_constant(id, value),
            // A float's literal is its 32 bits in one word.
            ScalarClass::Float => Inst::new(op::CONSTANT, vec![ids.float, id, value as u32]),
            // A half is declared as the 32-bit float whose high bits are its own (`Ids::type_of`).
            ScalarClass::BFloat16 => {
                Inst::new(op::CONSTANT, vec![ids.float, id, (value as u32) & !0xFFFF])
            }
        });
        self.emitted.insert((class, value), id);
        id
    }
}

/// The storage-buffer type chain for one class: element struct, runtime array, block
/// struct, two pointers.
///
/// # Invariant
/// Vulkan requires a `StorageBuffer` variable to be typed as a struct, or an array of
/// one, and a runtime array may only be the final member of one — so the chain is two
/// struct levels. `ptr_elem` points at one element in the storage-buffer storage
/// class, and its pointee is that class's own scalar.
#[derive(Clone, Copy)]
struct BufferTypes {
    elem: u32,
    array: u32,
    buffer_struct: u32,
    ptr_array: u32,
    ptr_elem: u32,
}

/// The shared-memory type chain for one slice shape: the slice's `Workgroup` array and its two
/// pointers.
///
/// # Invariant
/// A slice is a **fixed-size array**, not a scalar, and its length is the count the fragment
/// declares for it: `len` is the `OpConstant` that count is declared with, defined before the
/// type that names it, so the array a body is bounded by and the array this module declares are
/// one number. Two slices that differ only in length are two shapes.
#[derive(Clone, Copy)]
struct SharedTypes {
    array: u32,
    ptr_array: u32,
    ptr_elem: u32,
    len: u32,
}

/// Every module-scope id, allocated before anything is emitted.
///
/// # Invariant
/// One cursor hands every module-scope id out, and nothing else may name one.
/// A hand-written range cannot stay correct as the set grows — a second buffer
/// class pushes its type chains into the range the constants were written at, and
/// `spirv-val` answers `Id 16 is defined more than once`.
struct Ids {
    /// The class the module's *arithmetic* is built for — [`module_class`] of the
    /// root; not a buffer's element type.
    class: ScalarClass,
    /// The `GLSL.std.450` import an [`KernelInstr::Fma`] names.
    ///
    /// # Invariant
    /// Taken by the same cursor as every other module-scope id and **declared only when a body
    /// emitted an `OpExtInst`**: an id nothing defines is legal, so a module that never reaches
    /// for the set pays nothing, and one that declares the import without using it pays six words.
    glsl_std_450: u32,
    void: u32,
    boolean: u32,
    /// The fragment's own integer type: unsigned 64-bit in an integer module.
    ///
    /// # Invariant
    /// Unsigned, because the language's `Int` is a machine-sized unsigned integer and the unsigned
    /// opcodes require it — SPIR-V checks that operand, so declaring it signed would make a dividing
    /// kernel invalid rather than wrong. A float module declares no 64-bit integer: its integers are
    /// 32-bit indices, and one would cost the `Int64` capability.
    ulong: u32,
    /// The 32-bit float: the scalar in a float module, and declared only there.
    float: u32,
    /// The 32-bit unsigned type: an invocation id's component, a member index, and a
    /// float module's element index.
    uint: u32,
    /// The 16-bit unsigned type: a `BFloat16` buffer's **element**, and declared only
    /// when one is in use.
    ///
    /// # Invariant
    /// The storage domain and not the value domain: a brain float is the top sixteen bits
    /// of the float it rounds to, so its two bytes are those bits and nothing else. Its
    /// `ArrayStride` is [`ScalarClass::byte_width`], which is what makes the stride and
    /// the element one number rather than two that happen to agree. Declared
    /// unconditionally would cost every module the `Int16` capability.
    half: u32,
    /// The 32-bit `16` a widening or a narrowing shifts by, and declared with [`Self::half`].
    ///
    /// # Invariant
    /// **32 bits whatever the module's own integer class**, because the widening is
    /// `OpUConvert` to `uint` and an `OpShiftRightLogical` naming a 64-bit constant is a
    /// type error. A shift amount is an *id*, so it is one declared constant rather than
    /// a literal operand.
    half_shift: u32,
    v3uint: u32,
    /// The per-class storage-buffer type chain, keyed by [`ScalarClass::index`].
    ///
    /// # Invariant
    /// One chain per class a fragment actually uses: the element type, the array
    /// stride and the block struct are all a property of the *buffer's* class, so a
    /// module that reads one `Int` buffer and writes one `Float` buffer needs both.
    /// An id nothing defines is legal — the id bound is an upper limit, not a count.
    buffer_types: [BufferTypes; ScalarClass::ALL.len()],
    ptr_in: u32,
    /// One `OpTypeFunction` per fragment, because an `OpFunctionCall` is typed
    /// by the callee's signature.
    function_types: Vec<u32>,
    /// One `OpFunction` per fragment, in the launch set's order.
    ///
    /// # Invariant
    /// Position `0` is the entry point, and every function is named before any
    /// body is walked, so a call may name a function emitted later.
    functions: Vec<u32>,
    /// The 32-bit `0` an access chain's indices are built from.
    zero: u32,
    /// The scalar `1` and `0` a comparison is materialised into, in the integer class.
    ///
    /// # Invariant
    /// **Both are declared in every module, whatever `class` is.** A float module's `Int` is its
    /// 32-bit index and it materialises a boolean into that class as readily as an integer module
    /// materialises one into `Float`, so a pool that declared either expr only for its own class
    /// emits an operand no instruction defines.
    one_integer: u32,
    zero_integer: u32,
    /// The same materialisation where the position wants the float class.
    one_float: u32,
    zero_float: u32,
    gid: u32,
    /// The first of `binding.total()` consecutive storage-buffer variables.
    buffers: u32,
    /// The shared-memory type chain, keyed by the slice shape it declares.
    ///
    /// # Invariant
    /// One chain per distinct `Shared`-placed [`Slice`] a launch set stages, so two slices of one
    /// shape are one `OpTypeArray`. The slice's ordinal is not a key: an id nothing defines is
    /// legal, and a shape no slice has leaves no array emitted for it.
    shared_types: HashMap<Slice, SharedTypes>,
    /// The first of each function's own consecutive `Workgroup` variables, in the order the launch
    /// set is emitted.
    ///
    /// # Invariant
    /// **The slice space is the fragment's**: a slice access's ordinal indexes the variables of the
    /// function that reads it, so a callee's staging is not its caller's.
    shared: Vec<u32>,
    /// One `(count, constant id)` expr per distinct slot length, in the order the shapes name them.
    ///
    /// # Invariant
    /// An `OpTypeArray` names its length by id and a length can serve several shapes, so a constant
    /// is declared once per distinct count — emitted in this order, because a map's own order is
    /// not a function of the fragment and a module that differed between runs would not cache.
    shared_lengths: Vec<(u32, u32)>,
    /// The barrier's three operands name these two ids: the scope constant, and the semantics.
    barrier_scope: u32,
    /// The scope an atomic add's memory operand names, declared with [`Self::barrier_scope`].
    device_scope: u32,
    /// The same barrier at the scope a `Warp` exchange is ordered at, with its semantics.
    subgroup_scope: u32,
    /// The semantics that order a register rather than a memory.
    subgroup_semantics: u32,
    barrier_semantics: u32,
    /// The semantics a staged **store**'s barrier is ordered at.
    acquire_release: u32,
    /// The push-constant block's struct, its variable, and a pointer to each class's
    /// member within it.
    ///
    /// # Invariant
    /// The one place a launch's scalar leaves reach the shader: a compute entry point
    /// takes no parameters, so a value the host fixes per dispatch arrives here rather
    /// than through the function's signature.
    push_block: u32,
    push_block_ptr: u32,
    push_var: u32,
    /// A pointer to one member of the block, one per class — at most two, so a run of any
    /// leaf list is built from these.
    push_pointers: [u32; ScalarClass::ALL.len()],
    /// One 32-bit member index per pushed leaf, the indices its access chains name.
    push_index: Vec<u32>,
    /// How many leaves this module pushes: the root's leaves but the index.
    push_members: usize,
}

impl Ids {
    /// The entry point's `OpFunction`: the one the pipeline names `main`.
    fn entry(&self) -> u32 {
        self.functions
            .first()
            .copied()
            .expect("a launch set holds a root, so the module holds an entry function")
    }

    /// The module's scalar: the type of every value the body computes.
    ///
    /// # Invariant
    /// A `BFloat16` module computes in the 32-bit float it takes over the `Float` arm: the body
    /// widens on the way in and narrows on the way out, so no value in it is a brain float. SPIR-V's
    /// 16-bit float is an IEEE half and is never what this answers.
    fn scalar(&self) -> u32 {
        match self.class {
            ScalarClass::Int => self.ulong,
            ScalarClass::Float | ScalarClass::BFloat16 => self.float,
        }
    }

    /// The type an integer value has — an element index, or an integer operand.
    fn integer(&self) -> u32 {
        match self.class {
            ScalarClass::Int => self.ulong,
            ScalarClass::Float | ScalarClass::BFloat16 => self.uint,
        }
    }

    /// The `OpConstant` declaring `id` as the integer `value`, at this module's integer
    /// width.
    ///
    /// # Invariant
    /// The literal words are the type's width: two after the low word in an integer module,
    /// one in a float one, because a float module's `Int` is the 32-bit unsigned its indices
    /// are. One spelling, so the width a pool constant is written at and the width
    /// [`Literals::get`] writes a literal at cannot part company.
    fn integer_constant(&self, id: u32, value: i64) -> Inst {
        let mut operands = vec![self.integer(), id, value as u32];
        // Low word, then high word; a 32-bit type's literal is one word.
        if self.class == ScalarClass::Int {
            operands.push((value >> 32) as u32);
        }
        Inst::new(op::CONSTANT, operands)
    }

    /// The type a value of `class` has.
    ///
    /// # Invariant
    /// **The value domain, not the storage one** — [`Self::storage_type_of`] is the storage
    /// answer. A `BFloat16` is the `Float` here because the body widens on the way in and
    /// narrows on the way out, so every value a body computes is the 32-bit float; a brain
    /// float has no SPIR-V type of its own to compute in.
    fn type_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.integer(),
            ScalarClass::Float | ScalarClass::BFloat16 => self.float,
        }
    }

    /// The type a buffer of `class` **stores** each element as.
    ///
    /// # Invariant
    /// The storage domain, and deliberately not [`Self::type_of`]: a value a body computes
    /// is the `Float` a brain float widens to, but a `BFloat16` buffer's element is the
    /// 16-bit integer its two bytes are. Answering one question with the other's rule is
    /// how a buffer came to be laid out at the width of a value it never holds.
    fn storage_type_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::BFloat16 => self.half,
            _ => self.type_of(class),
        }
    }

    /// The scalar `1`/`0` of `class`, for materialising a comparison.
    fn one_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.one_integer,
            ScalarClass::Float | ScalarClass::BFloat16 => self.one_float,
        }
    }

    fn zero_of(&self, class: ScalarClass) -> u32 {
        match class {
            ScalarClass::Int => self.zero_integer,
            ScalarClass::Float | ScalarClass::BFloat16 => self.zero_float,
        }
    }

    /// The storage-buffer type chain for `class`.
    fn chain_of(&self, class: ScalarClass) -> BufferTypes {
        self.buffer_types[class.index()]
    }

    /// The shared-memory type chain for a slice of this shape.
    fn shared_of(&self, slice: Slice) -> SharedTypes {
        self.shared_types[&slice]
    }
}

/// A NUL-terminated, word-padded literal string, as SPIR-V packs them.
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

/// The flattened parameter offset the index occupies: the last leaf.
pub(crate) fn index_local(fragment: &KernelFragment) -> Option<u32> {
    let arity = fragment.param_shape.flat_arity();
    (arity > 0).then(|| (arity - 1) as u32)
}

/// The bytes one pushed leaf occupies in the block.
///
/// # Invariant
/// The width of the type the emitter gives it, **not** [`ScalarClass::byte_width`]: an
/// integer leaf is `i64` in an integer module and `u32` in a float one, the rule the
/// invocation id follows too. The buffer width would place the next leaf four bytes early.
pub(crate) fn push_width(module: ScalarClass, class: ScalarClass) -> u32 {
    match (module, class) {
        (ScalarClass::Int, ScalarClass::Int) => 8,
        _ => 4,
    }
}

/// The byte offset of each pushed leaf inside the push-constant block, and the block's
/// size.
///
/// # Invariant
/// **One answer for the emitter and the dispatch**: a member's `Offset` decoration and the
/// bytes `cmd_push_constants` writes are the same number written twice, so they are computed
/// once here. A leaf sits at the next offset its own width divides, because SPIR-V requires a
/// member's offset to be a multiple of its type's alignment.
pub(crate) fn push_layout(module: ScalarClass, leaves: &[ScalarClass]) -> (Vec<u32>, u32) {
    let mut offsets = Vec::with_capacity(leaves.len());
    let mut cursor = 0u32;
    for &class in leaves {
        let width = push_width(module, class);
        cursor = cursor.div_ceil(width) * width;
        offsets.push(cursor);
        cursor += width;
    }
    (offsets, cursor)
}

/// The class of buffer `slot` — inputs first, then outputs, the order a [`Binding`] uses.
///
/// # Invariant
/// A buffer's element type, array stride and block struct are that buffer's own facts,
/// so each is read from the fragment's per-buffer class lists rather than from a single
/// module-wide answer, and a slot those lists do not reach falls back to
/// [`module_class`]. `pub(crate)` because the dispatch path asks it too: the bytes
/// staged for a buffer are that buffer's class's `byte_width()`.
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

/// Every class a fragment's buffers hold, in [`ScalarClass::ALL`] order.
///
/// # Invariant
/// The set the module declares a type chain for: a class no buffer holds is not
/// declared, because an unused `OpTypeStruct` is a second copy of a rule nobody asked
/// for, and a fragment that declared it could not say which chain a binding means.
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

/// Every distinct `Shared`-placed slice shape a launch set stages, in the order the fragments name
/// them.
///
/// # Invariant
/// The set the module declares a shared type chain and a length constant for: a launch set with no
/// `Shared` slice declares no `Workgroup` storage, no barrier and no array type. Deduplicated
/// because two slices of one shape are one `OpTypeArray`, and ordered by first use so the ids a
/// module assigns are a function of the fragment.
fn shared_shapes(launch: &LaunchSet<'_>) -> Vec<Slice> {
    let mut seen = HashSet::new();
    let mut shapes = Vec::new();
    for fragment in launch.ordered().iter().copied() {
        for slice in &fragment.slices {
            if slice.space != Space::Shared {
                continue;
            }
            if seen.insert(*slice) {
                shapes.push(*slice);
            }
        }
    }
    shapes
}

/// Whether any fragment in the launch set places a slice for a warp exchange.
///
/// # Invariant
/// The capability is declared once for the module, so it follows the launch set rather than the
/// body being emitted: a module that declared it for a slice no access reached would ask the
/// device for a support it does not use, and a module that omitted it for one it did would fail
/// validation at the shuffle.
fn exchanges_a_warp(launch: &LaunchSet<'_>) -> bool {
    launch.ordered().iter().copied().any(|fragment| {
        fragment
            .slices
            .iter()
            .any(|slice| slice.space == Space::Warp)
    })
}

/// The class a fragment's arithmetic is built for, and the answer for one with no
/// declared buffer class.
///
/// # Invariant
/// The buffer element classes come first: they are what the body reads and writes, and
/// the positions data crosses the ABI at. A fragment with no declared buffer class is
/// read off its parameter leaves **except the index**, and one with neither is an
/// integer module. It no longer refuses a fragment whose buffers disagree — a buffer's
/// element type is that buffer's own class.
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
    let leaves = fragment.param_shape.leaf_classes();
    let index = index_local(fragment);
    Ok(leaves
        .iter()
        .enumerate()
        .find(|(offset, _)| Some(*offset as u32) != index)
        .map(|(_, class)| *class)
        .unwrap_or(ScalarClass::Int))
}

/// Whether a module for this launch set declares the 64-bit integer type, and
/// needs `shaderInt64`.
///
/// # Invariant
/// It asks the **whole set**, not the root: a callee is a function in the same module, so
/// an integer buffer it reads is an integer chain this module declares.
pub fn needs_int64(launch: &LaunchSet<'_>) -> Result<bool, SpirvRefusal> {
    for fragment in launch.ordered().iter().copied() {
        let holds_an_integer_buffer = fragment
            .input_classes
            .iter()
            .chain(fragment.output_classes.iter())
            .any(|class| *class == ScalarClass::Int);
        if holds_an_integer_buffer || module_class(fragment)? == ScalarClass::Int {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The element classes this launch set adds to atomically, deduplicated and in [`ScalarClass::ALL`]
/// order.
///
/// # Invariant
/// **Asks the whole set, for the same reason [`needs_int64`] does**: a callee is a function in the
/// same module, so its atomics are this module's. A `BFloat16` entry is reported as itself rather
/// than dropped, so a caller can name the class it has no primitive for.
pub fn atomic_classes(launch: &LaunchSet<'_>) -> Vec<ScalarClass> {
    let mut found: Vec<ScalarClass> = Vec::new();
    for fragment in launch.ordered().iter().copied() {
        for block in &fragment.body.blocks {
            for definition in &block.instrs {
                let Some(ValueDef::Instr {
                    op: KernelInstr::BufferAtomicAddCall(class),
                    ..
                }) = fragment.body.values.get(definition.0 as usize)
                else {
                    continue;
                };
                if !found.contains(class) {
                    found.push(*class);
                }
            }
        }
    }
    ScalarClass::ALL
        .iter()
        .copied()
        .filter(|class| found.contains(class))
        .collect()
}

/// The capabilities and extended instruction sets a module's bodies reached for, one flag each.
///
/// # Invariant
/// **Recorded by the emission that names it, and read once by [`assemble`]**: each of the three
/// is a declaration the module owes for an opcode it wrote, and `spirv-val` answers every
/// absence by name. A flag the walk does not set is a module that declares nothing extra, which
/// is the point: an integer atomic alone asks no device for `shaderBufferFloat32Atomics`.
#[derive(Default)]
struct Extensions {
    /// Whether any body emitted an `OpExtInst`, and so whether the module declares `GLSL.std.450`.
    glsl_std_450: bool,
    /// Whether any body added atomically to a 64-bit integer, and so whether the module declares
    /// `Int64Atomics`.
    int64_atomics: bool,
    /// Whether a body added atomically to a float, and so whether the module declares
    /// its capability and extension.
    ///
    /// # Invariant
    /// **Both travel together**: the extension registers the opcode, the capability gates it.
    float_add_atomics: bool,
}

impl Extensions {
    /// Record what `instr` needs, if it is something this target has no unconditional capability
    /// for.
    fn note(&mut self, instr: KernelInstr) {
        match instr {
            KernelInstr::Fma => self.glsl_std_450 = true,
            KernelInstr::BufferAtomicAddCall(class) => match class {
                ScalarClass::Int => self.int64_atomics = true,
                ScalarClass::Float => self.float_add_atomics = true,
                // Refused by name during the walk, so the module never reaches here.
                ScalarClass::BFloat16 => {}
            },
            _ => {}
        }
    }
}

/// Compile a launch set to SPIR-V words: one `OpFunction` per fragment, position
/// `0` being the entry point.
pub fn compile(launch: &LaunchSet<'_>, binding: Binding) -> Result<Vec<u32>, SpirvRefusal> {
    let ordered = launch.ordered();
    let root = launch.root();
    // The module's numeric class, read off the root before anything is emitted.
    let class = module_class(root)?;
    let index = index_local(root).ok_or(SpirvRefusal::ResultArity {
        results: root.result_classes.len(),
        left: 0,
    })?;
    // **One cursor hands out every module-scope id**, so no range can collide
    // with a chain.
    let mut taken = 1u32;
    let mut take = |count: u32| {
        let first = taken;
        taken += count;
        first
    };
    let glsl_std_450 = take(1);
    let void = take(1);
    let boolean = take(1);
    let ulong = take(1);
    let float = take(1);
    let uint = take(1);
    // **Taken unconditionally**: an id nothing defines is legal.
    let half = take(1);
    let half_shift = take(1);
    let v3uint = take(1);
    let ptr_in = take(1);
    let zero = take(1);
    let one_integer = take(1);
    let zero_integer = take(1);
    let one_float = take(1);
    let zero_float = take(1);
    let gid = take(1);
    let classes = buffer_classes(root, class);
    // Five ids per class, then one function type and one function per fragment.
    let mut buffer_types = [BufferTypes {
        elem: 0,
        array: 0,
        buffer_struct: 0,
        ptr_array: 0,
        ptr_elem: 0,
    }; ScalarClass::ALL.len()];
    for used in &classes {
        let base = take(5);
        buffer_types[used.index()] = BufferTypes {
            elem: base,
            array: base + 1,
            buffer_struct: base + 2,
            ptr_array: base + 3,
            ptr_elem: base + 4,
        };
    }
    // Every function is named before any body is walked: a call may name one
    // that is emitted later.
    let function_types = take(ordered.len() as u32);
    let functions = take(ordered.len() as u32);
    let buffers = take(binding.total() as u32);
    // Three ids per slice shape, one length constant per distinct count, then each function's own
    // `Workgroup` variables.
    let shared = shared_shapes(launch);
    let mut shared_types: HashMap<Slice, SharedTypes> = HashMap::new();
    let mut length_ids: HashMap<u32, u32> = HashMap::new();
    let mut shared_lengths: Vec<(u32, u32)> = Vec::new();
    for shape in &shared {
        let base = take(3);
        let length = match length_ids.get(&shape.count) {
            Some(&length) => length,
            None => {
                let length = take(1);
                length_ids.insert(shape.count, length);
                shared_lengths.push((shape.count, length));
                length
            }
        };
        shared_types.insert(
            *shape,
            SharedTypes {
                array: base,
                ptr_array: base + 1,
                ptr_elem: base + 2,
                len: length,
            },
        );
    }
    let shared_variables: Vec<u32> = ordered
        .iter()
        .map(|fragment| take(fragment.slices.len() as u32))
        .collect();
    let barrier_scope = take(1);
    let device_scope = take(1);
    let subgroup_scope = take(1);
    let subgroup_semantics = take(1);
    let barrier_semantics = take(1);
    let acquire_release = take(1);
    // The push-constant block's ids: struct, pointer, variable, per-class
    // pointers, and one member index per leaf.
    let push_members = root.scalar_leaves();
    let push_block = take(1);
    let push_block_ptr = take(1);
    let push_var = take(1);
    let push_pointer_base = take(ScalarClass::ALL.len() as u32);
    let push_index: Vec<u32> = (0..push_members).map(|_| take(1)).collect();
    let ids = Ids {
        class,
        glsl_std_450,
        void,
        boolean,
        ulong,
        float,
        uint,
        half,
        half_shift,
        v3uint,
        buffer_types,
        ptr_in,
        function_types: (0..ordered.len())
            .map(|offset| function_types + offset as u32)
            .collect(),
        functions: (0..ordered.len())
            .map(|offset| functions + offset as u32)
            .collect(),
        zero,
        one_integer,
        zero_integer,
        one_float,
        zero_float,
        gid,
        buffers,
        shared_types,
        shared: shared_variables,
        shared_lengths,
        barrier_scope,
        device_scope,
        subgroup_scope,
        subgroup_semantics,
        barrier_semantics,
        acquire_release,
        push_block,
        push_block_ptr,
        push_var,
        push_pointers: std::array::from_fn(|class| push_pointer_base + class as u32),
        push_index,
        push_members,
    };

    // `OpConstant` is module-scope, so the literals are emitted with the types.
    let mut literals = Literals::default();
    // The first function-local id: everything above is module-scope.
    let mut next = taken;
    let mut bodies = Vec::with_capacity(ordered.len());
    // Every float result this module's arithmetic rounds once at, and so must keep.
    let mut exact_results: Vec<u32> = Vec::new();
    // The extended sets the bodies reached for, read once the module is written.
    let mut extensions = Extensions::default();
    for (position, fragment) in ordered.iter().copied().enumerate() {
        bodies.push(emit_function(
            Function {
                root,
                fragment,
                entry: position == 0,
                index,
                binding,
                shared: ids.shared[position],
            },
            &ids,
            launch,
            &mut literals,
            &mut next,
            &mut exact_results,
            &mut extensions,
        )?);
    }

    Ok(assemble(
        launch,
        &ids,
        binding,
        &classes,
        &shared,
        exchanges_a_warp(launch),
        needs_int64(launch)?,
        &literals.declarations,
        &bodies,
        &exact_results,
        &extensions,
        next,
    ))
}

/// One function's facts.
struct Function<'a> {
    /// The fragment whose buffers are bound, and the slices' classes read from.
    root: &'a KernelFragment,
    fragment: &'a KernelFragment,
    /// Whether this is position `0`: the entry point.
    entry: bool,
    /// The root's index parameter, from `index_local`.
    index: u32,
    binding: Binding,
    /// The first of this function's own consecutive `Workgroup` variables.
    shared: u32,
}

/// One function's instructions: its parameters, then its blocks in an order
/// where a definition precedes its uses.
///
/// # Invariant
/// Its ids are this function's own. They start above every module-scope id and
/// run to a cursor the next function starts above, so nothing two functions emit
/// can be one value — and a module-scope id is never reissued as a local one,
/// which is what would make a constant and a local the same id.
fn emit_function(
    function: Function<'_>,
    ids: &Ids,
    launch: &LaunchSet<'_>,
    mut literals: &mut Literals,
    cursor: &mut u32,
    exact_results: &mut Vec<u32>,
    extensions: &mut Extensions,
) -> Result<Vec<Inst>, SpirvRefusal> {
    // This function's own cursor, above every module-scope id and above every
    // earlier function's.
    let mut next = *cursor;
    let Function {
        root,
        fragment,
        entry,
        index,
        binding,
        shared,
    } = function;
    // The float results this body owes a `NoContraction`, per the IR's own decision.
    let exact = fragment.body.exact_float_arithmetic;
    if let Err(broken) = fragment.body.validate() {
        return Err(SpirvRefusal::ControlFlow { detail: broken });
    }
    // **The root's class, not this fragment's**: every function in one module computes in it.
    let module = module_class(root)?;
    if fragment.target.int_width.bits() != 64 {
        return Err(SpirvRefusal::UnsupportedIntWidth {
            bits: fragment.target.int_width.bits(),
        });
    }
    if fragment.target.lane_width.lanes() != LOCAL_SIZE_X {
        return Err(SpirvRefusal::UnsupportedLaneWidth {
            lanes: fragment.target.lane_width.lanes(),
        });
    }
    // The reads a barrier is owed for, one per recorded expr.
    let barrier_reads: HashSet<ValueId> = fragment
        .body
        .barriers
        .iter()
        .map(|expr| expr.read)
        .collect();
    // The entry block's label is the first id this function owns.
    let entry_label = next;
    next += 1;
    // The code the body's first instruction reads is defined here.
    let mut prologue: Vec<Inst> = Vec::new();
    let mut parameters: Vec<Inst> = Vec::new();
    let mut slots: HashMap<ValueId, Slot> = HashMap::new();
    // What each `Warp` slice holds in this lane: the exchange reads a register, not a memory.
    let mut warp_values: HashMap<usize, Slot> = HashMap::new();

    // Every parameter is placed by the one mechanism this target has for it.
    let leaves = fragment.param_shape.leaf_classes();
    for (offset, &value) in fragment.body.parameters().iter().enumerate() {
        let Some(&class) = leaves.get(offset) else {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "this body takes {} parameter(s) but its domain flattens to {}, so the \
                     parameter at position {offset} has nothing to bind to",
                    fragment.body.parameters().len(),
                    leaves.len()
                ),
            });
        };
        let (id, read_as) = if !entry {
            let id = next;
            next += 1;
            parameters.push(Inst::new(
                op::FUNCTION_PARAMETER,
                vec![ids.type_of(class), id],
            ));
            (id, class)
        } else if offset as u32 == index {
            // The index is the invocation, and `Int` whatever the leaf declares.
            let loaded = next;
            next += 1;
            let component = next;
            next += 1;
            prologue.push(Inst::new(op::LOAD, vec![ids.v3uint, loaded, ids.gid]));
            prologue.push(Inst::new(
                op::COMPOSITE_EXTRACT,
                vec![ids.uint, component, loaded, 0],
            ));
            let id = match ids.class {
                ScalarClass::Int => {
                    let widened = next;
                    next += 1;
                    prologue.push(Inst::new(
                        op::U_CONVERT,
                        vec![ids.ulong, widened, component],
                    ));
                    widened
                }
                // **Bound as the `Float` one does**: a brain-float module computes in f32.
                ScalarClass::Float | ScalarClass::BFloat16 => component,
            };
            (id, ScalarClass::Int)
        } else {
            // One member of the block, read once where every use can reach it.
            let pointer = next;
            next += 1;
            let loaded = next;
            next += 1;
            prologue.push(Inst::new(
                op::ACCESS_CHAIN,
                vec![
                    ids.push_pointers[class.index()],
                    pointer,
                    ids.push_var,
                    ids.push_index[offset],
                ],
            ));
            prologue.push(Inst::new(
                op::LOAD,
                vec![ids.type_of(class), loaded, pointer],
            ));
            (loaded, class)
        };
        slots.insert(value, scalar(id, read_as));
    }
    // The prologue reads the block's inputs and nothing of the body, so it runs above the guard.
    let prologue_len = prologue.len();
    let first_label = next;

    // The structure the emitter adds: loop headers and one merge block per merge
    // instruction; see [`plan_body`].
    let plan = plan_body(fragment, entry_label, first_label)?;
    next += plan.fresh_labels;
    // One `OpPhi` list per block, filled as the edges that reach it are emitted.
    let mut blocks: Vec<Block> = plan.nodes.iter().map(|_| Block::default()).collect();

    let mut returns = 0usize;
    for position in 0..plan.order.len() {
        let node = plan.order[position];
        let label = plan.labels[node];
        let mut code: Vec<Inst> = Vec::new();
        if node == plan.entry {
            code.append(&mut prologue);
        }
        match plan.nodes[node] {
            Node::Real(block) => {
                for (at, &definition) in fragment.body.blocks[block].instrs.iter().enumerate() {
                    let Some(ValueDef::Instr { op, args, .. }) =
                        fragment.body.values.get(definition.0 as usize)
                    else {
                        continue;
                    };
                    let op = *op;
                    // **Every operand is named by the definition that produced it.**
                    let operand = |offset: usize| -> Result<Slot, SpirvRefusal> {
                        let value = args.get(offset).ok_or(SpirvRefusal::ResultArity {
                            results: fragment.result_classes.len(),
                            left: 0,
                        })?;
                        resolve_operand(fragment, &slots, *value)
                    };
                    let instruction = op;
                    match instruction {
                        KernelInstr::Const(class, value) => {
                            // One id per (class, value): SPIR-V requires every id defined
                            // exactly once.
                            let id = literals.get(class, value, &ids, &mut next);
                            slots.insert(definition, literal(id, value));
                        }
                        KernelInstr::Bin(class, operator @ (KernelBin::Max | KernelBin::Min)) => {
                            // **A comparison and a select, as the CPU path emits**: the
                            // targets' native maxima answer a NaN operand differently.
                            let rhs = operand(1)?;
                            let lhs = operand(0)?;
                            let operand_class = bin_class(lhs.kind, rhs.kind, class);
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
                            let comparison = match (operand_class, operator) {
                                (ScalarClass::Int, KernelBin::Max) => op::U_GREATER_THAN,
                                (ScalarClass::Int, KernelBin::Min) => op::U_LESS_THAN,
                                (ScalarClass::Float, KernelBin::Max) => op::F_GREATER_THAN,
                                (ScalarClass::Float, KernelBin::Min) => op::F_LESS_THAN,
                                _ => unreachable!("`Max` and `Min` are the only operators here"),
                            };
                            let condition = next;
                            next += 1;
                            code.push(Inst::new(
                                comparison,
                                vec![ids.boolean, condition, lhs.id, rhs.id],
                            ));
                            let result = next;
                            next += 1;
                            code.push(Inst::new(
                                op::SELECT,
                                vec![
                                    ids.type_of(operand_class),
                                    result,
                                    condition,
                                    lhs.id,
                                    rhs.id,
                                ],
                            ));
                            if exact && operand_class == ScalarClass::Float {
                                exact_results.push(result);
                            }
                            slots.insert(definition, scalar(result, operand_class));
                        }
                        KernelInstr::Bin(class, operator) => {
                            let rhs = operand(1)?;
                            let lhs = operand(0)?;
                            let operand_class = bin_class(lhs.kind, rhs.kind, class);
                            // A comparison's operands and result need not be scalars, so
                            // both are materialised here.
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
                            // Float `==`/`!=` compares **bit patterns**, so both operands are
                            // reinterpreted (`docs/notes/floating-point.md` §3.7).
                            let (opcode, result_type, result_kind, as_bits) =
                                match (operand_class, operator) {
                                    // An `Int` is unsigned: the signed opcodes agree below
                                    // 2^63 and diverge above, silently.
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
                                    // A comparison yields a bool here, which is what `Select`
                                    // consumes: no widening, and no cost.
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
                                    // The bitwise operators over two comparisons are the
                                    // language's `and`/`xor`/`or`.
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
                                    // **`OpFDiv` plainly, and no guard**: a zero divisor is
                                    // undefined here and IEEE on wasm.
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
                                    // A bf16 has no opcode in any width SPIR-V rounds a
                                    // brain float in, so the class is refused by name.
                                    (ScalarClass::BFloat16, _) => {
                                        return Err(SpirvRefusal::UnsupportedClass {
                                            class: operand_class,
                                        });
                                    }
                                    (_, KernelBin::Max) | (_, KernelBin::Min) => {
                                        unreachable!(
                                            "`Max`/`Min` is emitted before this table is read"
                                        )
                                    }
                                };
                            if as_bits {
                                let left_bits = next;
                                next += 1;
                                let right_bits = next;
                                next += 1;
                                code.push(Inst::new(
                                    op::BITCAST,
                                    vec![ids.uint, left_bits, lhs.id],
                                ));
                                code.push(Inst::new(
                                    op::BITCAST,
                                    vec![ids.uint, right_bits, rhs.id],
                                ));
                                code.push(Inst::new(
                                    opcode,
                                    vec![result_type, result, left_bits, right_bits],
                                ));
                            } else {
                                code.push(Inst::new(
                                    opcode,
                                    vec![result_type, result, lhs.id, rhs.id],
                                ));
                            }
                            slots.insert(
                                definition,
                                Slot {
                                    id: result,
                                    kind: result_kind,
                                },
                            );
                            // Every float result, not a narrower set a reading blamed.
                            if exact && matches!(result_kind, Kind::Scalar(ScalarClass::Float)) {
                                exact_results.push(result);
                            }
                        }
                        // **Not a no-op here**: this target's `select` takes a bool, so this
                        // is where a condition becomes one.
                        KernelInstr::I32WrapI64 => {
                            let popped = operand(0)?;
                            slots.insert(
                                definition,
                                as_condition(popped, &ids, &mut literals, &mut code, &mut next),
                            );
                        }
                        // **The three operands in the instruction's own order**:
                        // `args[0]*args[1] + args[2]`.
                        KernelInstr::Fma => {
                            let added = operand(2)?;
                            let multiplier = operand(1)?;
                            let multiplicand = operand(0)?;
                            let added = as_class(
                                added,
                                ScalarClass::Float,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let multiplier = as_class(
                                multiplier,
                                ScalarClass::Float,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let multiplicand = as_class(
                                multiplicand,
                                ScalarClass::Float,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let result = next;
                            next += 1;
                            code.push(Inst::new(
                                op::EXT_INST,
                                vec![
                                    ids.float,
                                    result,
                                    ids.glsl_std_450,
                                    GLSL_STD_450_FMA,
                                    multiplicand.id,
                                    multiplier.id,
                                    added.id,
                                ],
                            ));
                            extensions.note(KernelInstr::Fma);
                            slots.insert(definition, scalar(result, ScalarClass::Float));
                            // **Every float result, as `Bin` records them.**
                            if exact {
                                exact_results.push(result);
                            }
                        }
                        KernelInstr::Negate => {
                            let negated = as_class(
                                operand(0)?,
                                ScalarClass::Float,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let result = next;
                            next += 1;
                            code.push(Inst::new(
                                op::FP_NEGATE,
                                vec![ids.float, result, negated.id],
                            ));
                            slots.insert(definition, scalar(result, ScalarClass::Float));
                            // A negation rounds nothing, so this decoration is inert; kept anyway.
                            if exact {
                                exact_results.push(result);
                            }
                        }
                        KernelInstr::Exp
                        | KernelInstr::Sqrt
                        | KernelInstr::Floor
                        | KernelInstr::Ln
                        | KernelInstr::Sin
                        | KernelInstr::Cos => {
                            return Err(SpirvRefusal::UnexpandedTranscendental { at });
                        }
                        KernelInstr::Select => {
                            let selector = operand(2)?;
                            let otherwise = operand(1)?;
                            let then = operand(0)?;
                            // **The arms' own class**, which the module never decides.
                            let arms =
                                select_class(fragment, launch, module, args, &mut Vec::new())?;
                            let then = as_class(
                                then,
                                arms,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let otherwise = as_class(
                                otherwise,
                                arms,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            let selector =
                                as_condition(selector, &ids, &mut literals, &mut code, &mut next);
                            let result = next;
                            next += 1;
                            code.push(Inst::new(
                                op::SELECT,
                                vec![
                                    ids.type_of(arms),
                                    result,
                                    selector.id,
                                    then.id,
                                    otherwise.id,
                                ],
                            ));
                            slots.insert(definition, scalar(result, arms));
                        }
                        // **The direction is the instruction's**, which is why the IR carries
                        // the expr: both crossings have one operand shape.
                        KernelInstr::Conv { from, to } => {
                            let (from, to) = (from, to);
                            let seen = operand(0)?;
                            let seen = as_class(
                                seen,
                                from,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            // A crossing between one class and itself is a reclassification,
                            // and emits nothing.
                            if from == to {
                                slots.insert(definition, seen);
                                continue;
                            }
                            // **Every crossing is representable here**, because the module
                            // declares both element types.
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
                            // An access chain's index is an integer, so a float here is refused
                            // by name.
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
                            // **A read yields that buffer's element class**, read
                            // off the root: a slot is one module-scope variable.
                            let element_class = buffer_class_of(root, slot, ids.class);
                            let chain = ids.chain_of(element_class);
                            let pointer = next;
                            next += 1;
                            let loaded = next;
                            next += 1;
                            // Three indices: the block struct's member, the element, the
                            // element struct's member.
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
                            // The load is typed by the **storage** element, which for a
                            // brain float is the 16-bit integer rather than the float.
                            code.push(Inst::new(
                                op::LOAD,
                                vec![ids.storage_type_of(element_class), loaded, pointer],
                            ));
                            // **The widening is the boundary**: what the body holds is the
                            // `Float`, so no bf16 value exists to compute in.
                            let (id, read_as) = match element_class {
                                ScalarClass::BFloat16 => (
                                    widen_a_half(&ids, loaded, &mut code, &mut next),
                                    ScalarClass::Float,
                                ),
                                _ => (loaded, element_class),
                            };
                            slots.insert(definition, scalar(id, read_as));
                        }
                        KernelInstr::OutputReadCall(_) => {
                            // **The barrier between the store and the read**: the write is a
                            // storage buffer's, so the fence is over every class.
                            if barrier_reads.contains(&definition) {
                                code.push(Inst::new(
                                    op::CONTROL_BARRIER,
                                    vec![
                                        ids.barrier_scope,
                                        ids.barrier_scope,
                                        ids.barrier_semantics,
                                    ],
                                ));
                                code.push(Inst::new(
                                    op::MEMORY_BARRIER,
                                    vec![ids.barrier_scope, ids.acquire_release],
                                ));
                            }
                            let element = operand(1)?;
                            let position = operand(0)?;
                            let element = as_class(
                                element,
                                ScalarClass::Int,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            // **The output space**: the slot an output ordinal names, past
                            // the inputs.
                            let slot = buffer_slot(
                                position,
                                at,
                                binding.inputs,
                                binding.outputs,
                                "output",
                            )?;
                            let element_class = buffer_class_of(root, slot, ids.class);
                            let chain = ids.chain_of(element_class);
                            let pointer = next;
                            next += 1;
                            let loaded = next;
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
                            code.push(Inst::new(
                                op::LOAD,
                                vec![ids.storage_type_of(element_class), loaded, pointer],
                            ));
                            let (id, read_as) = match element_class {
                                ScalarClass::BFloat16 => (
                                    widen_a_half(&ids, loaded, &mut code, &mut next),
                                    ScalarClass::Float,
                                ),
                                _ => (loaded, element_class),
                            };
                            slots.insert(definition, scalar(id, read_as));
                        }
                        KernelInstr::BufferAtomicAddCall(class) => {
                            if class == ScalarClass::BFloat16 {
                                return Err(SpirvRefusal::AtomicAddNotAHardwarePrimitive {
                                    class,
                                    at,
                                });
                            }
                            let value = operand(2)?;
                            let element = operand(1)?;
                            let position = operand(0)?;
                            let slot = buffer_slot(
                                position,
                                at,
                                binding.inputs,
                                binding.outputs,
                                "output",
                            )?;
                            // **The plain write's address, word for word**: only the fed
                            // instruction differs.
                            let element_class = buffer_class_of(root, slot, ids.class);
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
                            // Required and unused: the result is required by SPIR-V, so it is
                            // allocated to keep the bound right, no slot pushed.
                            let result = next;
                            next += 1;
                            let opcode = match class {
                                ScalarClass::Int => op::ATOMIC_I_ADD,
                                _ => op::ATOMIC_F_ADD,
                            };
                            code.push(Inst::new(
                                opcode,
                                vec![
                                    ids.storage_type_of(element_class),
                                    result,
                                    pointer,
                                    ids.device_scope,
                                    ids.zero,
                                    value.id,
                                ],
                            ));
                            // **Recorded by the emission, like every other capability**.
                            extensions.note(KernelInstr::BufferAtomicAddCall(class));
                        }
                        KernelInstr::BufferWriteCall(_) => {
                            let value = operand(2)?;
                            let element = operand(1)?;
                            let position = operand(0)?;
                            let slot = buffer_slot(
                                position,
                                at,
                                binding.inputs,
                                binding.outputs,
                                "output",
                            )?;
                            // **A write stores that buffer's element class**, read
                            // off the root as above.
                            let element_class = buffer_class_of(root, slot, ids.class);
                            let chain = ids.chain_of(element_class);
                            // **Narrowed on the way out**, so the value is taken as the
                            // `Float` the body holds rather than as its own class.
                            let stored_class = match element_class {
                                ScalarClass::BFloat16 => ScalarClass::Float,
                                _ => element_class,
                            };
                            let value = as_class(
                                value,
                                stored_class,
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
                            // **The store is typed by the storage element**, so a brain
                            // float is narrowed here rather than stored as a float.
                            let stored = match element_class {
                                ScalarClass::BFloat16 => {
                                    narrow_a_half(&ids, value.id, &mut code, &mut next)
                                }
                                _ => value.id,
                            };
                            code.push(Inst::new(op::STORE, vec![pointer, stored]));
                        }
                        KernelInstr::SliceWriteCall(_) => {
                            let index = operand(1)?;
                            let (ordinal, shape) = slice_shape(fragment, shared, operand(0)?, at)?;
                            let value = as_class(
                                operand(2)?,
                                shape.class,
                                &ids,
                                &mut literals,
                                &mut code,
                                &mut next,
                                at,
                            )?;
                            match shape.space {
                                Space::Warp => {
                                    // **Nothing is emitted**: the value is the lane's own, and the
                                    // read is what names it across lanes.
                                    warp_values.insert(ordinal, value);
                                }
                                Space::Register => {
                                    return Err(SpirvRefusal::StagingNotOnThisBackend {
                                        space: shape.space,
                                        at,
                                    });
                                }
                                Space::Shared => {
                                    let (pointer, element_class) = slice_pointer(
                                        &ids,
                                        shared,
                                        shape,
                                        ordinal,
                                        index,
                                        at,
                                        &mut code,
                                        &mut literals,
                                        &mut next,
                                    )?;
                                    // **The version this leaves is not materialised**: its only
                                    // consumer is a read's third operand, which emission ignores.
                                    let value = as_class(
                                        value,
                                        element_class,
                                        &ids,
                                        &mut literals,
                                        &mut code,
                                        &mut next,
                                        at,
                                    )?;
                                    code.push(Inst::new(op::STORE, vec![pointer, value.id]));
                                }
                            }
                        }
                        KernelInstr::SliceReadCall(_) => {
                            let index = operand(1)?;
                            let (ordinal, shape) = slice_shape(fragment, shared, operand(0)?, at)?;
                            match shape.space {
                                Space::Warp => {
                                    // Immediately before the read, at the scope the
                                    // exchange is confined to (§9).
                                    if barrier_reads.contains(&definition) {
                                        code.push(Inst::new(
                                            op::CONTROL_BARRIER,
                                            vec![
                                                ids.subgroup_scope,
                                                ids.subgroup_scope,
                                                ids.subgroup_semantics,
                                            ],
                                        ));
                                    }
                                    let exchanged = exchange_through_a_warp(
                                        warp_values.get(&ordinal).copied(),
                                        shape,
                                        index,
                                        &ids,
                                        &mut literals,
                                        &mut code,
                                        &mut next,
                                        at,
                                    )?;
                                    slots.insert(definition, exchanged);
                                }
                                Space::Register => {
                                    return Err(SpirvRefusal::StagingNotOnThisBackend {
                                        space: shape.space,
                                        at,
                                    });
                                }
                                Space::Shared => {
                                    // Immediately before the read's definition
                                    // (`docs/notes/shared-memory-and-barriers.md` §9).
                                    if barrier_reads.contains(&definition) {
                                        code.push(Inst::new(
                                            op::CONTROL_BARRIER,
                                            vec![
                                                ids.barrier_scope,
                                                ids.barrier_scope,
                                                ids.barrier_semantics,
                                            ],
                                        ));
                                    }
                                    let (pointer, element_class) = slice_pointer(
                                        &ids,
                                        shared,
                                        shape,
                                        ordinal,
                                        index,
                                        at,
                                        &mut code,
                                        &mut literals,
                                        &mut next,
                                    )?;
                                    let loaded = next;
                                    next += 1;
                                    code.push(Inst::new(
                                        op::LOAD,
                                        vec![ids.type_of(element_class), loaded, pointer],
                                    ));
                                    slots.insert(definition, scalar(loaded, element_class));
                                }
                            }
                        }
                        // **The callee's own facts**: the set carries its arity,
                        // which is its domain's flattening.
                        KernelInstr::CallKernel(callee) => {
                            let Some((position, target)) = launch.callee(callee) else {
                                return Err(SpirvRefusal::CalleeNotInLaunchSet {
                                    kernel: callee,
                                    at,
                                });
                            };
                            let leaves = target.param_shape.leaf_classes();
                            if args.len() != leaves.len() {
                                return Err(SpirvRefusal::CrossKernelArity {
                                    kernel: callee,
                                    expected: leaves.len(),
                                    given: args.len(),
                                    at,
                                });
                            }
                            let Some(&result_class) = target.result_classes.first() else {
                                return Err(SpirvRefusal::CrossKernelResults {
                                    kernel: callee,
                                    results: target.result_classes.len(),
                                    at,
                                });
                            };
                            let result = next;
                            next += 1;
                            let mut call =
                                vec![ids.type_of(result_class), result, ids.functions[position]];
                            for (offset, class) in leaves.iter().enumerate() {
                                // **A leaf's class is its own**: the position
                                // coerces the argument.
                                let argument = operand(offset)?;
                                call.push(
                                    as_class(
                                        argument,
                                        *class,
                                        &ids,
                                        &mut literals,
                                        &mut code,
                                        &mut next,
                                        at,
                                    )?
                                    .id,
                                );
                            }
                            code.push(Inst::new(op::FUNCTION_CALL, call));
                            slots.insert(definition, scalar(result, result_class));
                        }
                    }
                }
                Emitter {
                    fragment,
                    entry,
                    ids: &ids,
                    plan: &plan,
                    launch,
                    module,
                    blocks: &mut blocks,
                    slots: &mut slots,
                    literals: &mut literals,
                    next: &mut next,
                    returns: &mut returns,
                }
                .terminator(node, block, &mut code)?;
            }
            Node::LoopMerge { .. } | Node::PassThrough { .. } => {
                let target = plan.targets[node][0];
                let args = blocks[node].params.clone();
                Emitter {
                    fragment,
                    entry,
                    ids: &ids,
                    plan: &plan,
                    launch,
                    module,
                    blocks: &mut blocks,
                    slots: &mut slots,
                    literals: &mut literals,
                    next: &mut next,
                    returns: &mut returns,
                }
                .record(target, label, args, &mut code, 0)?;
                code.push(Inst::new(op::BRANCH, vec![plan.labels[target]]));
            }
        }
        blocks[node].code = code;
    }

    // A function with no `OpReturn` has no way out, and SPIR-V requires one.
    if returns == 0 {
        return Err(SpirvRefusal::ControlFlow {
            detail: "no path through the body returns; a shader's function has to end".to_string(),
        });
    }
    let mut body = parameters;
    let guard = tail_guard(
        fragment,
        TailInputs {
            entry,
            index,
            leaves: &leaves,
            slots: &slots,
            prologue: prologue_len,
        },
        &mut next,
    );
    body.extend(function_body(&plan, &mut blocks, &ids, guard.as_ref()));
    // The next function starts above everything this one handed out.
    *cursor = next;
    Ok(body)
}

/// What the tail guard is decided from: this function's place in the launch and the leaves it bound.
struct TailInputs<'a> {
    entry: bool,
    index: u32,
    leaves: &'a [ScalarClass],
    slots: &'a HashMap<ValueId, Slot>,
    prologue: usize,
}

/// The `index < extent` guard the entry point's body is wrapped in, or `None` without one.
///
/// # Invariant
/// Member `0` is the extent and is `Int` — what a run whose first leaf is not is refused for —
/// and the index is the last leaf, so both operands share one integer type. The condition is
/// **uniformly true on a staged launch**: such a launch is refused unless its count is a whole
/// number of workgroups, so no surplus lane exists and none leaves the body early at a barrier.
fn tail_guard(
    fragment: &KernelFragment,
    inputs: TailInputs<'_>,
    cursor: &mut u32,
) -> Option<TailGuard> {
    let TailInputs {
        entry,
        index,
        leaves,
        slots,
        prologue,
    } = inputs;
    // A fragment whose only leaf is its index has no member `0` naming an extent to compare.
    if !entry || index == 0 || leaves.first() != Some(&ScalarClass::Int) {
        return None;
    }
    let lane = slots[&fragment.body.parameters()[index as usize]];
    let extent = slots[&fragment.body.parameters()[0]];
    let condition = *cursor;
    *cursor += 1;
    let enter = *cursor;
    *cursor += 1;
    let merge = *cursor;
    *cursor += 1;
    Some(TailGuard {
        condition,
        less_than: vec![lane.id, extent.id],
        enter,
        merge,
        prologue,
    })
}

/// The two blocks the tail guard adds, and how much of the body's entry block it takes.
struct TailGuard {
    /// The `%bool` holding `index < extent`.
    condition: u32,
    /// `OpULessThan`'s operands after its result: the index, then the extent.
    less_than: Vec<u32>,
    /// The label the guard itself runs in, ahead of the body's own entry block.
    enter: u32,
    /// The selection's merge block, which a surplus lane returns from.
    merge: u32,
    /// How much of the entry block's own code reads its inputs and runs ahead of the branch.
    prologue: usize,
}

/// The slot a value reads through, or the body's own refusal for a value nothing
/// defined.
///
/// # Invariant
/// Every parameter is placed before the body is walked, so an operand with no slot is a
/// value no instruction produced — which the body already validated against.
fn resolve_operand(
    fragment: &KernelFragment,
    slots: &HashMap<ValueId, Slot>,
    value: ValueId,
) -> Result<Slot, SpirvRefusal> {
    slots.get(&value).copied().ok_or(SpirvRefusal::ResultArity {
        results: fragment.result_classes.len(),
        left: 0,
    })
}

/// The type an id of `kind` has.
fn type_of(ids: &Ids, kind: Kind) -> u32 {
    match kind {
        Kind::Scalar(class) => ids.type_of(class),
        Kind::Condition => ids.boolean,
        // A parameter's kind is resolved when its first edge arrives, so a literal
        // never carries a phi.
        Kind::Literal(_) => ids.scalar(),
    }
}

/// The kind a block parameter has: an `OpPhi` has one type however its edges were
/// written.
///
/// # Invariant
/// A literal is not a class of its own, so `resolved` answers `None` for one and the caller asks
/// [`value_class`] instead: an edge's kind says nothing, and the module's class is what [`type_of`]
/// falls back to for a value no definition states a class for.
fn resolved(kind: Kind) -> Option<Kind> {
    match kind {
        Kind::Literal(_) => None,
        other => Some(other),
    }
}

/// The class a block of the body computes `parameter` in, or `None` when nothing about it says.
///
/// # Invariant
/// The class comes from the block's own instruction list, through [`operand_class`], the rule each
/// emission uses. Two reads stating two classes are
/// [`SpirvRefusal::BlockParameterClassDisagrees`], because a phi has one type. `None` is not a
/// refusal: no read computes it in any class, so no type it is given is observed.
fn read_class(
    fragment: &KernelFragment,
    launch: &LaunchSet<'_>,
    module: ScalarClass,
    block: usize,
    parameter: ValueId,
    visiting: &mut Vec<ValueId>,
) -> Result<Option<ScalarClass>, SpirvRefusal> {
    // **A cycle of reads**: a parameter already under resolution answers `None`.
    if visiting.contains(&parameter) {
        return Ok(None);
    }
    visiting.push(parameter);
    let stated = stated_class(fragment, launch, module, block, parameter, visiting);
    visiting.pop();
    stated
}

/// The loop of [`read_class`], with `parameter` already marked as under resolution.
fn stated_class(
    fragment: &KernelFragment,
    launch: &LaunchSet<'_>,
    module: ScalarClass,
    block: usize,
    parameter: ValueId,
    visiting: &mut Vec<ValueId>,
) -> Result<Option<ScalarClass>, SpirvRefusal> {
    let mut stated: Option<ScalarClass> = None;
    for &definition in &fragment.body.blocks[block].instrs {
        let Some(ValueDef::Instr { op, args, .. }) =
            fragment.body.values.get(definition.0 as usize)
        else {
            continue;
        };
        for (position, &operand) in args.iter().enumerate() {
            if operand != parameter {
                continue;
            }
            let class = match operand_class(launch, op, position) {
                Some(class) => class,
                // **A `Select` arm**: the class of the value this position holds.
                None if matches!(op, KernelInstr::Select) && position != 2 => {
                    select_class(fragment, launch, module, args, visiting)?
                }
                None => continue,
            };
            match stated {
                Some(seen) if seen != class => {
                    let mut classes = vec![seen, class];
                    classes.sort_by_key(|class| class.index());
                    classes.dedup();
                    return Err(SpirvRefusal::BlockParameterClassDisagrees { parameter, classes });
                }
                Some(_) => {}
                None => stated = Some(class),
            }
        }
    }
    Ok(stated)
}

/// The class the body computes `value` in, from the body's own declarations.
///
/// # Invariant
/// A value's class is its **definition's**: an instruction states the class its emission reads and
/// writes in, and a call the callee's **result** class — what both emissions type the slot it
/// leaves by, not its first argument's. A parameter's is what its reads say ([`read_class`]) or
/// what its edges carry where nothing reads it. `visiting` bounds that walk.
fn value_class(
    fragment: &KernelFragment,
    launch: &LaunchSet<'_>,
    module: ScalarClass,
    value: ValueId,
    visiting: &mut Vec<ValueId>,
) -> Result<ScalarClass, SpirvRefusal> {
    let Some(definition) = fragment.body.values.get(value.0 as usize) else {
        return Ok(module);
    };
    // **A constant states its own class**, which no position can reclassify.
    if let ValueDef::Instr {
        op: KernelInstr::Const(class, _),
        ..
    } = definition
    {
        return Ok(*class);
    }
    let ValueDef::BlockParam { block, index } = definition else {
        let ValueDef::Instr { op, args, .. } = definition else {
            return Ok(module);
        };
        // **A value is in the class its own definition states**; a `Select` and a call state none.
        if matches!(op, KernelInstr::Select) {
            return select_class(fragment, launch, module, args, visiting);
        }
        // **A brain-float read is a `Float`**; a call states the callee's result class.
        let stated = match op {
            KernelInstr::BufferReadCall(ScalarClass::BFloat16) => Some(ScalarClass::Float),
            KernelInstr::CallKernel(callee) => launch
                .callee(*callee)
                .and_then(|(_, target)| target.result_classes.first().copied()),
            _ => op.own_class(),
        };
        return Ok(stated.unwrap_or(module));
    };
    let (block, index) = (*block, *index as usize);
    if let Some(class) = read_class(fragment, launch, module, block, value, visiting)? {
        return Ok(class);
    }
    if !visiting.contains(&value) {
        visiting.push(value);
        let mut stated: Option<ScalarClass> = None;
        for carried in incoming_edges(fragment, block) {
            let Some(&argument) = carried.get(index) else {
                continue;
            };
            let class = value_class(fragment, launch, module, argument, visiting)?;
            match stated {
                Some(seen) if seen != class => {
                    let mut classes = vec![seen, class];
                    classes.sort_by_key(|class| class.index());
                    classes.dedup();
                    return Err(SpirvRefusal::BlockParameterClassDisagrees {
                        parameter: value,
                        classes,
                    });
                }
                Some(_) => {}
                None => stated = Some(class),
            }
        }
        visiting.pop();
        if let Some(class) = stated {
            return Ok(class);
        }
    }
    Ok(module)
}

/// Every edge into `block`, as the value it carries per position of the block's parameter list.
fn incoming_edges(fragment: &KernelFragment, block: usize) -> Vec<&[ValueId]> {
    let mut edges = Vec::new();
    for candidate in &fragment.body.blocks {
        let carried = match &candidate.terminator {
            Terminator::Return { .. } => None,
            Terminator::Br(br) => (br.target == block).then_some(br.args.as_slice()),
            Terminator::CondBr {
                if_true, if_false, ..
            } => {
                if if_true.target == block {
                    Some(if_true.args.as_slice())
                } else if if_false.target == block {
                    Some(if_false.args.as_slice())
                } else {
                    None
                }
            }
        };
        if let Some(carried) = carried {
            edges.push(carried);
        }
    }
    edges
}

/// The class the operand at `position` of `op` is computed in, or `None` when `op` fixes none.
///
/// # Invariant
/// One arm per operand position: an operator's operands are the class it names; a read's index
/// and a write's position are access-chain indices, so `Int` whatever the buffer holds; a slice
/// access's third operand is a version the emission ignores; a conversion's operand is the class it
/// comes from; a `Select` states none, so its arms are [`select_class`]'s to read.
fn operand_class(launch: &LaunchSet<'_>, op: &KernelInstr, position: usize) -> Option<ScalarClass> {
    match *op {
        KernelInstr::Bin(class, _) => Some(class),
        // **All three positions are `Float`**, the one class the instruction names.
        KernelInstr::Fma => Some(ScalarClass::Float),
        KernelInstr::Negate => Some(ScalarClass::Float),
        KernelInstr::Conv { from, .. } => (position == 0).then_some(from),
        // **A `Select` arm**: a fact about the value in the position, not the op.
        KernelInstr::Select => None,
        KernelInstr::BufferReadCall(_) => (position == 0).then_some(ScalarClass::Int),
        // **An output read's position is an output ordinal**, and an ordinal is an index.
        KernelInstr::OutputReadCall(_) => (position == 0).then_some(ScalarClass::Int),
        KernelInstr::SliceReadCall(_) => matches!(position, 0 | 1).then_some(ScalarClass::Int),
        KernelInstr::BufferWriteCall(class) | KernelInstr::BufferAtomicAddCall(class) => {
            (position == 2).then_some(class)
        }
        // The ordinal and the slot are indices, and a float body's slot is an `Int` scalar.
        KernelInstr::SliceWriteCall(class) => match position {
            0 | 1 => Some(ScalarClass::Int),
            2 => Some(class),
            _ => None,
        },
        KernelInstr::CallKernel(callee) => {
            let (_, target) = launch.callee(callee)?;
            target.param_shape.leaf_classes().get(position).copied()
        }
        KernelInstr::Const(_, _)
        | KernelInstr::I32WrapI64
        | KernelInstr::Exp
        | KernelInstr::Sqrt
        | KernelInstr::Floor
        | KernelInstr::Ln
        | KernelInstr::Sin
        | KernelInstr::Cos => None,
    }
}

/// The class a `Select`'s two arms are computed in, which is the class its result has.
///
/// # Invariant
/// `KernelInstr::Select` states no class, so the arms are the only evidence and each is read
/// through [`value_class`] — an integer index stays one whatever the module computes. `visiting` is
/// the phi's own, which also bounds an expr of arms that feed each other's class. Two arms of
/// different classes are [`SpirvRefusal::SelectArmClassesDisagree`]: an `OpSelect` has one
/// result type.
fn select_class(
    fragment: &KernelFragment,
    launch: &LaunchSet<'_>,
    module: ScalarClass,
    args: &[ValueId],
    visiting: &mut Vec<ValueId>,
) -> Result<ScalarClass, SpirvRefusal> {
    let [then, otherwise, ..] = args else {
        return Ok(module);
    };
    let then = value_class(fragment, launch, module, *then, visiting)?;
    let otherwise = value_class(fragment, launch, module, *otherwise, visiting)?;
    if then != otherwise {
        let mut classes = vec![then, otherwise];
        classes.sort_by_key(|class| class.index());
        classes.dedup();
        return Err(SpirvRefusal::SelectArmClassesDisagree { classes });
    }
    Ok(then)
}

/// One emitted block.
///
/// # Invariant
/// `params` has one slot per declared parameter and `edges` one expr per
/// predecessor, which is what an `OpPhi` needs; both are filled in as the
/// predecessors' terminators are emitted, so a block entered from a block written
/// later still receives its expr.
#[derive(Default)]
struct Block {
    /// One `OpPhi` result per declared parameter.
    params: Vec<Slot>,
    /// `(predecessor label, the values that edge passes)`, one expr per edge.
    edges: Vec<(u32, Vec<Slot>)>,
    /// Everything after the phis, the terminator included.
    code: Vec<Inst>,
}

impl Block {
    fn phis(&self, ids: &Ids) -> Vec<Inst> {
        let mut phis = Vec::with_capacity(self.params.len());
        for (offset, param) in self.params.iter().enumerate() {
            let mut operands = vec![type_of(ids, param.kind), param.id];
            for (from, values) in &self.edges {
                operands.push(values[offset].id);
                operands.push(*from);
            }
            phis.push(Inst::new(op::PHI, operands));
        }
        phis
    }
}

/// One block of the emitted control-flow graph.
#[derive(Clone, Copy)]
enum Node {
    /// A block of the IR body, at this index.
    Real(usize),
    /// A loop's merge block: the header's exit edge, handing the loop's values to
    /// the block the header named.
    LoopMerge { header: usize, exit: usize },
    /// A selection's pass-through, for a join the selection does not dominate.
    PassThrough { header: usize, join: usize },
}

/// The structure the emitter adds to a body, and the order its blocks are written.
struct Plan {
    nodes: Vec<Node>,
    labels: Vec<u32>,
    arities: Vec<usize>,
    /// Each node's successors, in the order its terminator names them.
    targets: Vec<Vec<usize>>,
    /// The merge block of each node whose terminator branches two ways.
    merges: Vec<Option<usize>>,
    /// The block each loop header returns from, where the node is a loop header.
    loop_bodies: Vec<Option<usize>>,
    /// The order the blocks are written in, entry first.
    order: Vec<usize>,
    entry: usize,
    /// How many label ids the plan took.
    fresh_labels: u32,
}

/// Lay a body out as structured SPIR-V control flow.
///
/// # Invariant
/// A loop is recognised structurally, because SSA has no `While`: a block is a loop
/// header when an edge on the active depth-first path returns to it, and that is the
/// same rule that makes it the dominator of everything the loop holds. A `CondBr` that
/// is not a loop header is a selection, whose merge block is the immediate
/// post-dominator of its arms.
fn plan_body(
    fragment: &KernelFragment,
    entry_label: u32,
    first_label: u32,
) -> Result<Plan, SpirvRefusal> {
    let body = &fragment.body;
    let count = body.blocks.len();
    let entry = body.entry;
    let successors: Vec<Vec<usize>> = body
        .blocks
        .iter()
        .map(|block| match &block.terminator {
            Terminator::Return { .. } => Vec::new(),
            Terminator::Br(br) => vec![br.target],
            Terminator::CondBr {
                if_true, if_false, ..
            } => vec![if_true.target, if_false.target],
        })
        .collect();

    let reachable = reachable_from(count, &successors, entry);
    if let Some(dead) = reachable.iter().position(|seen| !seen) {
        return Err(SpirvRefusal::ControlFlow {
            detail: format!(
                "block {dead} is not reachable from the entry, so nothing arrives at it and it has \
                 no phi"
            ),
        });
    }
    let dominates = dominators(count, &successors, entry);
    let after = immediate_post_dominators(count, &successors);
    let headers = loop_headers(count, &successors, entry);
    if headers[entry] {
        return Err(SpirvRefusal::ControlFlow {
            detail: "the entry block is a loop header, and its parameters are the function's own \
                     rather than a phi"
                .to_string(),
        });
    }

    let mut nodes: Vec<Node> = (0..count).map(Node::Real).collect();
    let mut arities: Vec<usize> = body.blocks.iter().map(|block| block.params.len()).collect();
    let mut merges: Vec<Option<usize>> = vec![None; count];
    let mut loop_bodies: Vec<Option<usize>> = vec![None; count];
    let mut loops: Vec<(usize, usize)> = Vec::new();
    // `(header, join, pass-through)` for every selection that needs one.
    let mut pass_throughs: Vec<(usize, usize, usize)> = Vec::new();

    for header in 0..count {
        if !headers[header] {
            continue;
        }
        let Terminator::CondBr {
            if_true, if_false, ..
        } = &body.blocks[header].terminator
        else {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "block {header} is a loop header and tests nothing, so its loop has no exit block"
                ),
            });
        };
        let (one, zero) = (if_true.target, if_false.target);
        // **Which branch stays in the loop is the one that can reach the header again**.
        let (body_entry, exit) = match (
            reaches(&successors, one, header),
            reaches(&successors, zero, header),
        ) {
            (true, false) => (one, zero),
            (false, true) => (zero, one),
            (true, true) => {
                return Err(SpirvRefusal::ControlFlow {
                    detail: format!(
                        "both of block {header}'s branches lead back to it, so its loop never leaves"
                    ),
                });
            }
            (false, false) => {
                return Err(SpirvRefusal::ControlFlow {
                    detail: format!("block {header} is a loop header with no path back to it"),
                });
            }
        };
        // `OpLoopMerge`'s continue target must differ from the header and be dominated
        // by it.
        if body_entry == header || !dominates[body_entry][header] {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "block {header}'s loop body is entered without passing its header, so it has no \
                     continue target"
                ),
            });
        }
        let merge = nodes.len();
        nodes.push(Node::LoopMerge { header, exit });
        arities.push(body.blocks[exit].params.len());
        merges[header] = Some(merge);
        loop_bodies[header] = Some(body_entry);
        loops.push((header, merge));
    }

    for header in 0..count {
        if headers[header] {
            continue;
        }
        let Terminator::CondBr {
            if_true, if_false, ..
        } = &body.blocks[header].terminator
        else {
            continue;
        };
        if if_true.target == if_false.target {
            continue;
        }
        // The join is where both arms meet again: the immediate post-dominator.
        let Some(join) = after[header] else {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "block {header} is a selection whose two arms never meet again, so it has no \
                     merge block"
                ),
            });
        };
        if join == count {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "block {header} is a selection whose arms meet only by leaving the function — a \
                     `return` or an exit from an enclosing loop — so it has no merge block"
                ),
            });
        }
        // **A selection's merge block must be dominated by the selection**, or it gets a
        // pass-through.
        if dominates[join][header] {
            merges[header] = Some(join);
        } else {
            let pass = nodes.len();
            nodes.push(Node::PassThrough { header, join });
            arities.push(body.blocks[join].params.len());
            merges[header] = Some(pass);
            pass_throughs.push((header, join, pass));
        }
    }

    let mut targets: Vec<Vec<usize>> = Vec::with_capacity(nodes.len());
    for node in 0..nodes.len() {
        let resolved = match nodes[node] {
            Node::Real(block) => match &body.blocks[block].terminator {
                Terminator::Return { .. } => Vec::new(),
                Terminator::Br(br) => {
                    vec![redirect(
                        &nodes,
                        &successors,
                        &dominates,
                        &pass_throughs,
                        node,
                        br.target,
                    )]
                }
                Terminator::CondBr {
                    if_true, if_false, ..
                } => match (loop_bodies[node], merges[node]) {
                    (Some(body_entry), Some(merge)) => vec![
                        redirect(
                            &nodes,
                            &successors,
                            &dominates,
                            &pass_throughs,
                            node,
                            body_entry,
                        ),
                        merge,
                    ],
                    (None, Some(_)) => vec![
                        redirect(
                            &nodes,
                            &successors,
                            &dominates,
                            &pass_throughs,
                            node,
                            if_true.target,
                        ),
                        redirect(
                            &nodes,
                            &successors,
                            &dominates,
                            &pass_throughs,
                            node,
                            if_false.target,
                        ),
                    ],
                    _ => vec![redirect(
                        &nodes,
                        &successors,
                        &dominates,
                        &pass_throughs,
                        node,
                        if_true.target,
                    )],
                },
            },
            Node::LoopMerge { exit, .. } => {
                vec![redirect(
                    &nodes,
                    &successors,
                    &dominates,
                    &pass_throughs,
                    node,
                    exit,
                )]
            }
            Node::PassThrough { join, .. } => {
                vec![redirect(
                    &nodes,
                    &successors,
                    &dominates,
                    &pass_throughs,
                    node,
                    join,
                )]
            }
        };
        targets.push(resolved);
    }

    let order =
        depth_first(nodes.len(), &targets, entry).ok_or_else(|| SpirvRefusal::ControlFlow {
            detail: "a block of the emitted graph is unreachable, so it has no arrival".to_string(),
        })?;
    // A write inside a loop body is emitted where the body writes it
    // (`docs/notes/loop-conversion.md` §6).
    let mut labels = vec![0u32; nodes.len()];
    labels[entry] = entry_label;
    let mut fresh_labels = 0u32;
    for node in 0..nodes.len() {
        if node == entry {
            continue;
        }
        labels[node] = first_label + fresh_labels;
        fresh_labels += 1;
    }
    Ok(Plan {
        nodes,
        labels,
        arities,
        targets,
        merges,
        loop_bodies,
        order,
        entry,
        fresh_labels,
    })
}

/// Resolve a declared target through the pass-throughs an edge crosses when it leaves a
/// selection.
fn redirect(
    nodes: &[Node],
    successors: &[Vec<usize>],
    dominates: &[Vec<bool>],
    pass_throughs: &[(usize, usize, usize)],
    from: usize,
    declared: usize,
) -> usize {
    // A block the emitter added sits where its header does, so the selections
    // enclosing it are the header's.
    let origin = match nodes[from] {
        Node::Real(block) => block,
        Node::LoopMerge { header, .. } | Node::PassThrough { header, .. } => header,
    };
    let mut best: Option<(usize, usize)> = None;
    for &(header, join, pass) in pass_throughs {
        if join != declared || pass == from {
            continue;
        }
        if !inside_selection(successors, header, join, origin) {
            continue;
        }
        // **The innermost enclosing selection**: a header dominating the one chosen is
        // deeper.
        match best {
            Some((seen, _)) if !dominates[seen][header] => {}
            _ => best = Some((header, pass)),
        }
    }
    best.map_or(declared, |(_, pass)| pass)
}

/// Whether `block` is inside the selection `header` → `join`: reached from the
/// header without passing through the join.
fn inside_selection(successors: &[Vec<usize>], header: usize, join: usize, block: usize) -> bool {
    if block == join {
        return false;
    }
    let mut seen = vec![false; successors.len()];
    seen[header] = true;
    let mut stack = vec![header];
    while let Some(node) = stack.pop() {
        if node == block {
            return true;
        }
        // The join is the selection's exit, so nothing beyond it is inside.
        if node == join {
            continue;
        }
        for &next in &successors[node] {
            if !seen[next] {
                seen[next] = true;
                stack.push(next);
            }
        }
    }
    false
}

/// Whether `to` is reachable from `from`.
fn reaches(successors: &[Vec<usize>], from: usize, to: usize) -> bool {
    reachable_from(successors.len(), successors, from)[to]
}

fn reachable_from(count: usize, successors: &[Vec<usize>], from: usize) -> Vec<bool> {
    let mut seen = vec![false; count];
    seen[from] = true;
    let mut stack = vec![from];
    while let Some(node) = stack.pop() {
        for &next in &successors[node] {
            if !seen[next] {
                seen[next] = true;
                stack.push(next);
            }
        }
    }
    seen
}

/// The dominator sets: `dominates[node][other]` says `other` dominates `node`.
///
/// # Invariant
/// Every node but the root is reachable from it, so the intersection over its
/// predecessors is never empty.
fn dominators(count: usize, successors: &[Vec<usize>], entry: usize) -> Vec<Vec<bool>> {
    let mut predecessors = vec![Vec::new(); count];
    for (node, next) in successors.iter().enumerate() {
        for &target in next {
            predecessors[target].push(node);
        }
    }
    let mut dom = vec![vec![true; count]; count];
    dom[entry] = (0..count).map(|node| node == entry).collect();
    loop {
        let mut changed = false;
        for node in 0..count {
            if node == entry {
                continue;
            }
            let mut next = vec![true; count];
            for &previous in &predecessors[node] {
                for other in 0..count {
                    next[other] &= dom[previous][other];
                }
            }
            next[node] = true;
            if next != dom[node] {
                dom[node] = next;
                changed = true;
            }
        }
        if !changed {
            return dom;
        }
    }
}

/// The immediate post-dominator of each node, over the graph with one virtual exit
/// every return reaches.
fn immediate_post_dominators(count: usize, successors: &[Vec<usize>]) -> Vec<Option<usize>> {
    let exit = count;
    let mut reversed: Vec<Vec<usize>> = vec![Vec::new(); count + 1];
    for (node, next) in successors.iter().enumerate() {
        if next.is_empty() {
            reversed[exit].push(node);
        } else {
            for &target in next {
                reversed[target].push(node);
            }
        }
    }
    let post = dominators(count + 1, &reversed, exit);
    (0..count)
        .map(|node| {
            // The virtual exit post-dominates everything that returns, so it is the
            // fallback answer.
            (0..count)
                .filter(|candidate| *candidate != node && post[node][*candidate])
                // The immediate one is post-dominated by every other strict
                // post-dominator, so it is the one with the most of them.
                .max_by_key(|candidate| post[*candidate].iter().filter(|seen| **seen).count())
                .or_else(|| post[node][exit].then_some(exit))
        })
        .collect()
}

/// Which blocks are loop headers: an edge that returns to a block on the active
/// depth-first path.
fn loop_headers(count: usize, successors: &[Vec<usize>], entry: usize) -> Vec<bool> {
    let mut headers = vec![false; count];
    let mut active = vec![false; count];
    let mut visited = vec![false; count];
    let mut path: Vec<(usize, usize)> = vec![(entry, 0)];
    visited[entry] = true;
    active[entry] = true;
    while let Some(frame) = path.last_mut() {
        let node = frame.0;
        if frame.1 < successors[node].len() {
            let target = successors[node][frame.1];
            frame.1 += 1;
            if active[target] {
                headers[target] = true;
            } else if !visited[target] {
                visited[target] = true;
                active[target] = true;
                path.push((target, 0));
            }
        } else {
            active[node] = false;
            path.pop();
        }
    }
    headers
}

/// The nodes in depth-first pre-order from `entry`, or `None` if some node is not
/// reached.
///
/// # Invariant
/// Pre-order is what makes a definition precede its uses: a dominator is an ancestor in
/// the depth-first tree, so it is written first.
fn depth_first(count: usize, successors: &[Vec<usize>], entry: usize) -> Option<Vec<usize>> {
    let mut order = Vec::with_capacity(count);
    let mut visited = vec![false; count];
    let mut stack = vec![entry];
    visited[entry] = true;
    while let Some(node) = stack.pop() {
        order.push(node);
        for &target in successors[node].iter().rev() {
            if !visited[target] {
                visited[target] = true;
                stack.push(target);
            }
        }
    }
    (order.len() == count).then_some(order)
}

/// The IR values a node receives, or none for a block the emitter added.
fn received(fragment: &KernelFragment, node: usize) -> &[ValueId] {
    match fragment.body.blocks.get(node) {
        Some(block) => &block.params,
        None => &[],
    }
}

/// The block-level emission: recording edges, and each block's terminator.
struct Emitter<'a> {
    fragment: &'a KernelFragment,
    /// Whether this function is the entry point, which returns nothing.
    entry: bool,
    ids: &'a Ids,
    plan: &'a Plan,
    /// The launch set, so a call's operands read the callee's own leaves.
    launch: &'a LaunchSet<'a>,
    /// The module's numeric class, which a value no body computes in any class is given.
    module: ScalarClass,
    blocks: &'a mut Vec<Block>,
    slots: &'a mut HashMap<ValueId, Slot>,
    literals: &'a mut Literals,
    next: &'a mut u32,
    returns: &'a mut usize,
}

impl Emitter<'_> {
    fn operand(&self, value: ValueId) -> Result<Slot, SpirvRefusal> {
        resolve_operand(self.fragment, self.slots, value)
    }

    fn arguments(&self, values: &[ValueId]) -> Result<Vec<Slot>, SpirvRefusal> {
        values.iter().map(|&value| self.operand(value)).collect()
    }

    fn condition(&mut self, value: ValueId, code: &mut Vec<Inst>) -> Result<Slot, SpirvRefusal> {
        let slot = self.operand(value)?;
        Ok(as_condition(slot, self.ids, self.literals, code, self.next))
    }

    /// The kind the value at `offset` of plan node `to` is computed in, given the edge's `arg`.
    ///
    /// # Invariant
    /// The body decides it, through [`value_class`]. **The edge's own kind is never the answer** —
    /// that is what a literal would become — and an edge already carrying a class only has to agree
    /// with the body's, a disagreement being [`SpirvRefusal::MixedClasses`]. A plan node the emitter
    /// added holds no IR values and falls back to the module's class.
    fn value_kind(
        &self,
        to: usize,
        offset: usize,
        arg: &Slot,
        at: usize,
    ) -> Result<Kind, SpirvRefusal> {
        // **A plan node is not an IR block**: a block the plan added stands for another and holds
        // no IR values of its own.
        let Some(block) = self.ir_block(to) else {
            return Ok(match resolved(arg.kind) {
                Some(kind) => kind,
                None => Kind::Scalar(self.module),
            });
        };
        let Some(&value) = received(self.fragment, block).get(offset) else {
            return Ok(Kind::Scalar(self.module));
        };
        let class = value_class(
            self.fragment,
            self.launch,
            self.module,
            value,
            &mut Vec::new(),
        )?;
        if let Some(Kind::Scalar(seen)) = resolved(arg.kind)
            && seen != class
        {
            return Err(SpirvRefusal::MixedClasses {
                at,
                seen,
                wanted: class,
            });
        }
        Ok(Kind::Scalar(class))
    }

    /// The IR block plan node `node` stands for, or `None` for a block the plan added.
    ///
    /// # Invariant
    /// A `LoopMerge` or a `PassThrough` carries the arity of the block it stands for and is not a
    /// block of the body, so a question about the body's own values is asked of `None` rather than
    /// of whatever real block happens to share its number.
    fn ir_block(&self, node: usize) -> Option<usize> {
        match self.plan.nodes.get(node) {
            Some(Node::Real(block)) => Some(*block),
            _ => None,
        }
    }

    /// Record the edge `from → to` and the values it passes.
    ///
    /// # Invariant
    /// A parameter's kind is the body's fact: an edge spells a `Literal`, which no class fixes, so
    /// the kind comes from [`value_class`] over the target's own values and the edge's kind only
    /// checks against it. The first edge fixes one kind per parameter, because an `OpPhi` has one
    /// type, and every later edge is coerced to it inside the predecessor's own block.
    fn record(
        &mut self,
        to: usize,
        from: u32,
        mut args: Vec<Slot>,
        code: &mut Vec<Inst>,
        at: usize,
    ) -> Result<(), SpirvRefusal> {
        let arity = self.plan.arities[to];
        if args.len() != arity {
            return Err(SpirvRefusal::ControlFlow {
                detail: format!(
                    "an edge into block {} passes {} value(s) and it takes {arity}",
                    self.plan.labels[to],
                    args.len()
                ),
            });
        }
        if arity > 0 && self.blocks[to].params.is_empty() {
            let mut params = Vec::with_capacity(arity);
            for (offset, arg) in args.iter().enumerate() {
                let id = *self.next;
                *self.next += 1;
                let kind = self.value_kind(to, offset, arg, at)?;
                params.push(Slot { id, kind });
            }
            for (offset, value) in received(self.fragment, to).iter().enumerate() {
                self.slots.insert(*value, params[offset]);
            }
            self.blocks[to].params = params;
        } else if arity > 0 {
            for (offset, arg) in args.iter_mut().enumerate() {
                let kind = self.blocks[to].params[offset].kind;
                *arg = match kind {
                    Kind::Condition => as_condition(*arg, self.ids, self.literals, code, self.next),
                    Kind::Scalar(class) => {
                        as_class(*arg, class, self.ids, self.literals, code, self.next, at)?
                    }
                    Kind::Literal(_) => *arg,
                };
            }
        }
        // **One `OpPhi` expr per predecessor**, so two edges out of one block are
        // one expr and cannot say which values arrived.
        if let Some(seen) = self.blocks[to].edges.iter().find(|(pred, _)| *pred == from) {
            if seen.1.iter().zip(args.iter()).any(|(a, b)| a.id != b.id) {
                return Err(SpirvRefusal::ControlFlow {
                    detail: format!(
                        "two edges out of block {from} into block {} pass different values, and one \
                         phi expr per predecessor cannot say which",
                        self.plan.labels[to]
                    ),
                });
            }
            return Ok(());
        }
        self.blocks[to].edges.push((from, args));
        Ok(())
    }

    fn terminator(
        &mut self,
        node: usize,
        block: usize,
        code: &mut Vec<Inst>,
    ) -> Result<(), SpirvRefusal> {
        let at = self.fragment.body.blocks[block].instrs.len();
        let label = self.plan.labels[node];
        match self.fragment.body.blocks[block].terminator.clone() {
            Terminator::Return { values } => {
                // **The returned values are the terminator's list**, so the result arity is
                // the body's fact.
                if values.len() != 1 || !self.slots.contains_key(&values[0]) {
                    return Err(SpirvRefusal::ResultArity {
                        results: self.fragment.result_classes.len(),
                        left: values.len(),
                    });
                }
                *self.returns += 1;
                // **The entry point returns nothing**; every other function
                // leaves the value its caller reads.
                if self.entry {
                    code.push(Inst::new(op::RETURN, vec![]));
                    return Ok(());
                }
                let Some(&result_class) = self.fragment.result_classes.first() else {
                    return Err(SpirvRefusal::ResultArity {
                        results: 0,
                        left: 1,
                    });
                };
                // Placed in the class the function's return type names.
                let value = as_class(
                    self.slots[&values[0]],
                    result_class,
                    self.ids,
                    self.literals,
                    code,
                    self.next,
                    at,
                )?;
                code.push(Inst::new(op::RETURN_VALUE, vec![value.id]));
            }
            Terminator::Br(br) => {
                let target = self.plan.targets[node][0];
                let args = self.arguments(&br.args)?;
                self.record(target, label, args, code, at)?;
                code.push(Inst::new(op::BRANCH, vec![self.plan.labels[target]]));
            }
            Terminator::CondBr {
                cond,
                if_true,
                if_false,
            } => {
                let condition = self.condition(cond, code)?;
                let targets = self.plan.targets[node].clone();
                let exit_first = self.plan.loop_bodies[node]
                    .is_some_and(|body_entry| if_true.target != body_entry);
                if let Some(body_entry) = self.plan.loop_bodies[node] {
                    let (body, exit) = if if_true.target == body_entry {
                        (if_true, if_false)
                    } else {
                        (if_false, if_true)
                    };
                    let body_args = self.arguments(&body.args)?;
                    let exit_args = self.arguments(&exit.args)?;
                    // Both edges first: a coercion a phi needs belongs before the
                    // merge instruction, which is second-to-last in the block.
                    self.record(targets[0], label, body_args, code, at)?;
                    self.record(targets[1], label, exit_args, code, at)?;
                    code.push(Inst::new(
                        op::LOOP_MERGE,
                        vec![
                            self.plan.labels[targets[1]],
                            self.plan.labels[body_entry],
                            CONTROL_NONE,
                        ],
                    ));
                } else if if_true.target == if_false.target {
                    // One arrival, so the branch is unconditional — and it says
                    // nothing the condition could have chosen between.
                    if if_true.args != if_false.args {
                        return Err(SpirvRefusal::ControlFlow {
                            detail: format!(
                                "block {block} branches to one block with two different value \
                                 lists, and an unconditional branch cannot say which"
                            ),
                        });
                    }
                    let args = self.arguments(&if_true.args)?;
                    self.record(targets[0], label, args, code, at)?;
                    code.push(Inst::new(op::BRANCH, vec![self.plan.labels[targets[0]]]));
                    return Ok(());
                } else {
                    for (position, br) in [&if_true, &if_false].into_iter().enumerate() {
                        let args = self.arguments(&br.args)?;
                        self.record(targets[position], label, args, code, at)?;
                    }
                    let merge = self.plan.merges[node];
                    let merge = merge.expect("a selection with two reachable arms has a merge");
                    code.push(Inst::new(
                        op::SELECTION_MERGE,
                        vec![self.plan.labels[merge], CONTROL_NONE],
                    ));
                }
                // A loop header's `targets` are `[body, exit]`, and the arms name the labels.
                let (on_true, on_false) = if exit_first {
                    (targets[1], targets[0])
                } else {
                    (targets[0], targets[1])
                };
                code.push(Inst::new(
                    op::BRANCH_CONDITIONAL,
                    vec![
                        condition.id,
                        self.plan.labels[on_true],
                        self.plan.labels[on_false],
                    ],
                ));
            }
        }
        Ok(())
    }
}

/// Every block of the emitted function in the plan's order: an `OpLabel`, the
/// block's `OpPhi`s, then its code.
///
/// # Invariant
/// **The guard leads the function and the body keeps its own labels**: a surplus lane branches
/// around the whole body, so the only thing that moves is the prologue, and every `OpPhi` still
/// names the block that actually reaches it. The merge block is reached from nowhere but the
/// branch, which is what makes it the post-dominator a `OpSelectionMerge` must name.
fn function_body(
    plan: &Plan,
    blocks: &mut [Block],
    ids: &Ids,
    guard: Option<&TailGuard>,
) -> Vec<Inst> {
    let mut function = Vec::new();
    for &node in &plan.order {
        let label = plan.labels[node];
        let Some(guard) = guard.filter(|_| node == plan.entry) else {
            function.push(Inst::new(op::LABEL, vec![label]));
            function.extend(blocks[node].phis(ids));
            function.append(&mut blocks[node].code);
            continue;
        };
        let mut less_than = vec![ids.boolean, guard.condition];
        less_than.extend_from_slice(&guard.less_than);
        let prologue = blocks[node]
            .code
            .drain(..guard.prologue)
            .collect::<Vec<_>>();
        function.push(Inst::new(op::LABEL, vec![guard.enter]));
        function.extend(prologue);
        function.push(Inst::new(op::U_LESS_THAN, less_than));
        function.push(Inst::new(
            op::SELECTION_MERGE,
            vec![guard.merge, CONTROL_NONE],
        ));
        function.push(Inst::new(
            op::BRANCH_CONDITIONAL,
            vec![guard.condition, label, guard.merge],
        ));
        function.push(Inst::new(op::LABEL, vec![label]));
        function.extend(blocks[node].phis(ids));
        function.append(&mut blocks[node].code);
        function.push(Inst::new(op::LABEL, vec![guard.merge]));
        function.push(Inst::new(op::RETURN, vec![]));
    }
    function
}

/// Write the module in SPIR-V's required section order.
///
/// # Invariant
/// The order is not a style choice: an entry point must precede the function it names, annotations
/// the types they decorate, types their use — all legal only because every id was reserved up front.
/// One `OpFunction` per fragment in the set's order, position `0` being the entry point.
fn assemble(
    launch: &LaunchSet<'_>,
    ids: &Ids,
    binding: Binding,
    classes: &[ScalarClass],
    shared: &[Slice],
    warp: bool,
    needs_int64: bool,
    literals: &[Inst],
    bodies: &[Vec<Inst>],
    exact_results: &[u32],
    extensions: &Extensions,
    bound: u32,
) -> Vec<u32> {
    let root = launch.root();
    let lane_width = root.target.lane_width.lanes();
    let mut out = vec![SPIRV_MAGIC, SPIRV_VERSION, GENERATOR, bound, 0];

    let emit_all = |out: &mut Vec<u32>, instructions: &[Inst]| {
        for instruction in instructions {
            instruction.encode(out);
        }
    };

    // 1. Capabilities: `Int64`, `Int64Atomics`, `AtomicFloat32AddEXT`, `Int16`, and the
    // `Warp` exchange's.
    let bf16_buffered = classes.contains(&ScalarClass::BFloat16);
    let mut capabilities = vec![Inst::new(op::CAPABILITY, vec![capability::SHADER])];
    if ids.class == ScalarClass::Int {
        capabilities.push(Inst::new(op::CAPABILITY, vec![capability::INT64]));
    }
    if extensions.int64_atomics {
        capabilities.push(Inst::new(op::CAPABILITY, vec![capability::INT64_ATOMICS]));
    }
    if extensions.float_add_atomics {
        capabilities.push(Inst::new(
            op::CAPABILITY,
            vec![capability::ATOMIC_FLOAT32_ADD],
        ));
    }
    if bf16_buffered {
        capabilities.push(Inst::new(op::CAPABILITY, vec![capability::INT16]));
    }
    if warp {
        capabilities.push(Inst::new(
            op::CAPABILITY,
            vec![capability::GROUP_NON_UNIFORM_SHUFFLE],
        ));
    }
    emit_all(&mut out, &capabilities);

    // 2. The extension the atomic float add is registered under.
    if extensions.float_add_atomics {
        // **No result id, unlike the import below**: `OpExtension` names a string alone.
        emit_all(
            &mut out,
            &[Inst::new(
                op::EXTENSION,
                spv_string(SHADER_ATOMIC_FLOAT_ADD_EXTENSION),
            )],
        );
    }

    // 3. The extended sets a body reached for — **ahead of the memory model**.
    if extensions.glsl_std_450 {
        let mut operands = vec![ids.glsl_std_450];
        operands.extend(spv_string(b"GLSL.std.450"));
        emit_all(&mut out, &[Inst::new(op::EXT_INST_IMPORT, operands)]);
    }

    // 4. The single memory model.
    emit_all(
        &mut out,
        &[Inst::new(
            op::MEMORY_MODEL,
            vec![ADDRESSING_MODEL_LOGICAL, MEMORY_MODEL_GLSL450],
        )],
    );

    // 5. Entry point and its execution mode. The interface lists only Input/Output
    // globals, which is all SPIR-V 1.4 allows.

    // Storage buffers are named by their `Binding` decorations, not listed here.
    let mut interface = vec![EXECUTION_MODEL_GL_COMPUTE, ids.entry()];
    interface.extend(spv_string(b"main"));
    interface.push(ids.gid);
    emit_all(&mut out, &[Inst::new(op::ENTRY_POINT, interface)]);
    emit_all(
        &mut out,
        &[Inst::new(
            op::EXECUTION_MODE,
            vec![ids.entry(), EXECUTION_MODE_LOCAL_SIZE, lane_width, 1, 1],
        )],
    );

    // 6. Annotations: the stride is the **buffer's own** element width, stated here and
    // nowhere else.
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
        // …and the struct containing the runtime array must say so, with its member's
        // offset.
        annotations.push(Inst::new(
            op::DECORATE,
            vec![types.buffer_struct, decoration::BLOCK],
        ));
        annotations.push(Inst::new(
            op::MEMBER_DECORATE,
            vec![types.buffer_struct, 0, decoration::OFFSET, 0],
        ));
    }
    // One `Offset` per leaf, from the layout the dispatch writes into.
    let pushed: Vec<ScalarClass> = root.param_shape.leaf_classes()[..ids.push_members].to_vec();
    if ids.push_members > 0 {
        let (offsets, _) = push_layout(ids.class, &pushed);
        for (member, offset) in offsets.into_iter().enumerate() {
            annotations.push(Inst::new(
                op::MEMBER_DECORATE,
                vec![ids.push_block, member as u32, decoration::OFFSET, offset],
            ));
        }
        annotations.push(Inst::new(
            op::DECORATE,
            vec![ids.push_block, decoration::BLOCK],
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
    // **One `NoContraction` per float result, and no operand**: three words each.
    for &result in exact_results {
        annotations.push(Inst::new(
            op::DECORATE,
            vec![result, decoration::NO_CONTRACTION],
        ));
    }
    emit_all(&mut out, &annotations);

    // 7. Types, then the constants (module-scope, so the body's literals belong here),
    // then the globals.
    let mut types = vec![
        Inst::new(op::TYPE_VOID, vec![ids.void]),
        Inst::new(op::TYPE_BOOL, vec![ids.boolean]),
    ];
    // **Both scalar types are declared**: `Float32` carries no capability, the
    // 64-bit integer costs `Int64`.
    types.push(Inst::new(op::TYPE_FLOAT, vec![ids.float, 32]));
    if needs_int64 {
        types.push(Inst::new(op::TYPE_INT, vec![ids.ulong, 64, 0]));
    }
    types.push(Inst::new(op::TYPE_INT, vec![ids.uint, 32, 0]));
    // **Only where a brain float is stored**: without `Int16` the validator refuses the type.
    if bf16_buffered {
        types.push(Inst::new(op::TYPE_INT, vec![ids.half, 16, 0]));
    }
    types.push(Inst::new(op::TYPE_VECTOR, vec![ids.v3uint, ids.uint, 3]));
    // One storage-buffer chain per class in use, each closing over its own scalar.
    for used in classes {
        let chain = ids.chain_of(*used);
        let scalar = ids.storage_type_of(*used);
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
    types.extend([Inst::new(
        op::TYPE_POINTER,
        vec![ids.ptr_in, storage_class::INPUT, ids.v3uint],
    )]);
    // A `Shared` slice's `Workgroup` array is as long as the slice itself declares, never the lane
    // width.
    for &(count, length) in &ids.shared_lengths {
        types.push(Inst::new(op::CONSTANT, vec![ids.uint, length, count]));
    }
    for shape in shared {
        let chain = ids.shared_of(*shape);
        let scalar = ids.type_of(shape.class);
        types.extend([
            Inst::new(op::TYPE_ARRAY, vec![chain.array, scalar, chain.len]),
            Inst::new(
                op::TYPE_POINTER,
                vec![chain.ptr_array, storage_class::WORKGROUP, chain.array],
            ),
            Inst::new(
                op::TYPE_POINTER,
                vec![chain.ptr_elem, storage_class::WORKGROUP, scalar],
            ),
        ]);
    }
    // One block member per leaf, in the parameter's own order.
    if ids.push_members > 0 {
        let mut block = vec![ids.push_block];
        block.extend(pushed.iter().map(|class| ids.type_of(*class)));
        types.push(Inst::new(op::TYPE_STRUCT, block));
        types.push(Inst::new(
            op::TYPE_POINTER,
            vec![
                ids.push_block_ptr,
                storage_class::PUSH_CONSTANT,
                ids.push_block,
            ],
        ));
        for class in ScalarClass::ALL {
            types.push(Inst::new(
                op::TYPE_POINTER,
                vec![
                    ids.push_pointers[class.index()],
                    storage_class::PUSH_CONSTANT,
                    ids.type_of(class),
                ],
            ));
        }
    }
    // **One function type per fragment**: `OpFunctionCall` is typed by the
    // callee's.
    for (position, fragment) in launch.ordered().iter().copied().enumerate() {
        let ty = ids.function_types[position];
        if position == 0 {
            types.push(Inst::new(op::TYPE_FUNCTION, vec![ty, ids.void]));
            continue;
        }
        let mut operands = vec![ty];
        operands.extend(
            fragment
                .param_shape
                .leaf_classes()
                .iter()
                .map(|class| ids.type_of(*class)),
        );
        operands.push(
            fragment
                .result_classes
                .first()
                .map(|class| ids.type_of(*class))
                // A callee that leaves other than one value is refused above.
                .unwrap_or(ids.void),
        );
        types.push(Inst::new(op::TYPE_FUNCTION, operands));
    }
    emit_all(&mut out, &types);

    let mut constants = vec![Inst::new(op::CONSTANT, vec![ids.uint, ids.zero, 0])];
    // The 16 both crossings shift by, declared **only** with the half type it belongs to.
    if bf16_buffered {
        constants.push(Inst::new(op::CONSTANT, vec![ids.uint, ids.half_shift, 16]));
    }
    // One 32-bit constant per pushed leaf, the member index its access chain names.
    constants.extend(
        ids.push_index
            .iter()
            .enumerate()
            .map(|(member, &id)| Inst::new(op::CONSTANT, vec![ids.uint, id, member as u32])),
    );
    // The scalar `1` and `0` a comparison is materialised into, over both classes.

    // Both exprs, unconditionally: either module class asks for either expr.
    constants.extend([
        Inst::new(
            op::CONSTANT,
            vec![ids.float, ids.one_float, 1.0f32.to_bits()],
        ),
        Inst::new(
            op::CONSTANT,
            vec![ids.float, ids.zero_float, 0.0f32.to_bits()],
        ),
        ids.integer_constant(ids.one_integer, 1),
        ids.integer_constant(ids.zero_integer, 0),
    ]);
    constants.extend(literals.iter().map(|literal| Inst {
        opcode: literal.opcode,
        operands: literal.operands.clone(),
    }));
    // The scope an atomic add's memory operand names, declared only where one was emitted.
    if extensions.int64_atomics || extensions.float_add_atomics {
        constants.push(Inst::new(
            op::CONSTANT,
            vec![ids.uint, ids.device_scope, SCOPE_DEVICE],
        ));
    }
    // The barrier's scope and semantics: the ids its operands name, at each of the two scopes
    // a staged expr is ordered at.
    if !shared.is_empty() || warp || holds_a_staged_store(launch.ordered()) {
        constants.extend([
            Inst::new(
                op::CONSTANT,
                vec![ids.uint, ids.barrier_scope, SCOPE_WORKGROUP],
            ),
            Inst::new(
                op::CONSTANT,
                vec![ids.uint, ids.subgroup_scope, SCOPE_SUBGROUP],
            ),
            Inst::new(
                op::CONSTANT,
                vec![
                    ids.uint,
                    ids.barrier_semantics,
                    MEMORY_SEMANTICS_WORKGROUP_MEMORY,
                ],
            ),
            Inst::new(
                op::CONSTANT,
                vec![
                    ids.uint,
                    ids.subgroup_semantics,
                    MEMORY_SEMANTICS_ACQUIRE_RELEASE_WORKGROUP_MEMORY,
                ],
            ),
        ]);
    }
    // **A staged store's barrier** is over a storage buffer, not the shared memory.
    if holds_a_staged_store(launch.ordered()) {
        constants.push(Inst::new(
            op::CONSTANT,
            vec![
                ids.uint,
                ids.acquire_release,
                MEMORY_SEMANTICS_ACQUIRE_RELEASE,
            ],
        ));
    }
    emit_all(&mut out, &constants);

    let mut globals = vec![Inst::new(
        op::VARIABLE,
        vec![ids.ptr_in, ids.gid, storage_class::INPUT],
    )];
    for slot in 0..binding.total() {
        // **A buffer's variable is typed with its own class's block struct**, which is
        // what makes a mixed module bindable.
        let chain = ids.chain_of(buffer_class_of(root, slot, ids.class));
        globals.push(Inst::new(
            op::VARIABLE,
            vec![
                chain.ptr_array,
                ids.buffers + slot as u32,
                storage_class::STORAGE_BUFFER,
            ],
        ));
    }
    if ids.push_members > 0 {
        globals.push(Inst::new(
            op::VARIABLE,
            vec![
                ids.push_block_ptr,
                ids.push_var,
                storage_class::PUSH_CONSTANT,
            ],
        ));
    }
    // **Shared memory is not bound**: a `Workgroup` variable gets no descriptor set or binding.
    for (position, fragment) in launch.ordered().iter().copied().enumerate() {
        for (ordinal, slice) in fragment.slices.iter().enumerate() {
            // **The ordinal is not renumbered**: an access names `slices[ordinal]`, so a gap stays
            // a gap.
            if slice.space != Space::Shared {
                continue;
            }
            globals.push(Inst::new(
                op::VARIABLE,
                vec![
                    ids.shared_of(*slice).ptr_array,
                    ids.shared[position] + ordinal as u32,
                    storage_class::WORKGROUP,
                ],
            ));
        }
    }
    emit_all(&mut out, &globals);

    // 8. Every function, its body and its end, in the launch set's order.
    for (position, body) in bodies.iter().enumerate() {
        let result = if position == 0 {
            ids.void
        } else {
            launch
                .ordered()
                .get(position)
                .and_then(|fragment| fragment.result_classes.first())
                .map(|class| ids.type_of(*class))
                .unwrap_or(ids.void)
        };
        emit_all(
            &mut out,
            &[Inst::new(
                op::FUNCTION,
                vec![
                    result,
                    ids.functions[position],
                    CONTROL_NONE,
                    ids.function_types[position],
                ],
            )],
        );
        emit_all(&mut out, body);
        emit_all(&mut out, &[Inst::new(op::FUNCTION_END, vec![])]);
    }
    out
}

/// The class a binary operator runs over, from its two operands.
///
/// # Invariant
/// A literal follows the other operand — `Const(1)` beside the index is the integer `1`, beside a
/// float the `f32` whose bits are `1` — and with both operands literals the module's own class
/// decides, which is what makes `2.0 * 3.0` float arithmetic. The price is that a pure constant
/// expression used as an index reads as arithmetic over two bit patterns and is refused there.
fn bin_class(lhs: Kind, rhs: Kind, module: ScalarClass) -> ScalarClass {
    match (lhs, rhs) {
        (Kind::Scalar(class), _) => class,
        (_, Kind::Scalar(class)) => class,
        _ => module,
    }
}

/// `slot` as a value of `class`, emitting what the change needs.
///
/// # Invariant
/// Three cases, each a fact about the module: a literal becomes a constant of `class`, the one IR
/// instruction whose class the position decides; a condition becomes that class's `1`/`0` through
/// `OpSelect`, since there is no `OpConvertBoolToInt`; and a scalar of the other class is refused by
/// name rather than converted.
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
        Kind::Scalar(seen) => Err(SpirvRefusal::MixedClasses {
            at,
            seen,
            wanted: class,
        }),
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
/// # Invariant
/// Non-zero is true, which is what wasm's `select` means by its condition. For a float the bit
/// pattern is what is tested, not the value: `-0.0` has a non-zero pattern and a `NaN` compares
/// unequal to everything, so the float is bitcast and compared as an integer.
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
        Kind::Scalar(ScalarClass::Float) | Kind::Scalar(ScalarClass::BFloat16) => {
            let bits = *next;
            *next += 1;
            let result = *next;
            *next += 1;
            // The pattern is compared in its 32-bit reading, so the zero is
            // `ids.zero`.
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

/// Whether a launch set's bodies hold a **staged store**: a barrier over a buffer.
///
/// # Invariant
/// **The read names it**: a staged store is a recorded barrier whose read is an
/// [`KernelInstr::OutputReadCall`], and the module declares the barrier's constants only when
/// one is there — so a module without one does not carry a semantics constant nothing names.
fn holds_a_staged_store(ordered: &[&KernelFragment]) -> bool {
    ordered.iter().any(|fragment| {
        fragment.body.barriers.iter().any(|expr| {
            matches!(
                fragment.body.values.get(expr.read.0 as usize),
                Some(ValueDef::Instr {
                    op: KernelInstr::OutputReadCall(_),
                    ..
                })
            )
        })
    })
}

/// Resolve a buffer position into a slot in the module's buffer list.
///
/// # Invariant
/// Reads and writes address different position spaces: a read names a config position counting from
/// `base` over the inputs, a write its emission ordinal counting from `base` over the outputs. So a
/// write's position `0` is the first output, never the first input.
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

/// The slice an access names, and its own shape.
///
/// # Invariant
/// The ordinal is a compile-time ordinal into `fragment`'s **own** `slices` — the rule a buffer
/// position follows — and it is what every placement decision is read off: the space says which of
/// them the access takes, and **a `Warp` one is measured before any access reaches it**, so a slice
/// too wide for the exchange is refused by whichever access comes first.
fn slice_shape(
    fragment: &KernelFragment,
    base: u32,
    slice: Slot,
    at: usize,
) -> Result<(usize, Slice), SpirvRefusal> {
    let variable = buffer_slot(slice, at, base as usize, fragment.slices.len(), "slice")?;
    let shape = fragment.slices[variable - base as usize];
    if shape.space == Space::Warp && shape.count != SUBGROUP_WIDTH {
        return Err(SpirvRefusal::StagingNotOneElementPerLane {
            count: shape.count,
            subgroup_width: SUBGROUP_WIDTH,
            at,
        });
    }
    Ok((variable - base as usize, shape))
}

/// The pointer a slice access names, and the class of the element it holds, with the access chain
/// emitted.
///
/// # Invariant
/// The index operand indexes the array that slice's own count declares, and a space this target
/// does not emit is refused rather than placed in `Workgroup` storage — which is why a `Warp`
/// access never reaches here, carrying no address to place.
fn slice_pointer(
    ids: &Ids,
    base: u32,
    shape: Slice,
    variable: usize,
    index: Slot,
    at: usize,
    code: &mut Vec<Inst>,
    literals: &mut Literals,
    next: &mut u32,
) -> Result<(u32, ScalarClass), SpirvRefusal> {
    let class = shape.class;
    let chain = match shape.space {
        Space::Shared => ids.shared_of(shape),
        Space::Warp | Space::Register => {
            return Err(SpirvRefusal::StagingNotOnThisBackend {
                space: shape.space,
                at,
            });
        }
    };
    let index = as_class(index, ScalarClass::Int, ids, literals, code, next, at)?;
    let pointer = *next;
    *next += 1;
    code.push(Inst::new(
        op::ACCESS_CHAIN,
        vec![chain.ptr_elem, pointer, base + variable as u32, index.id],
    ));
    Ok((pointer, class))
}

/// `slot` as the 32-bit unsigned a shuffle names the lane it exchanges from by.
///
/// # Invariant
/// The `Id` operand is a 32-bit integer scalar and this module's `Int` is 64 bits wide, so an
/// index read as `Int` is narrowed here rather than named — a 64-bit operand is not a lane id at
/// all, and `spirv-val` reads the difference as a missing type rather than a missing capability.
fn as_lane_id(
    slot: Slot,
    ids: &Ids,
    literals: &mut Literals,
    code: &mut Vec<Inst>,
    next: &mut u32,
    at: usize,
) -> Result<u32, SpirvRefusal> {
    let value = as_class(slot, ScalarClass::Int, ids, literals, code, next, at)?;
    if ids.integer() == ids.uint {
        return Ok(value.id);
    }
    let narrowed = *next;
    *next += 1;
    code.push(Inst::new(op::U_CONVERT, vec![ids.uint, narrowed, value.id]));
    Ok(narrowed)
}

/// The value one lane stages into a `Warp` slice, and what the read of it emits.
///
/// # Invariant
/// **The value is the lane's own**: a write emits nothing, because the exchange reads each lane's
/// register rather than an address, so the staged value is what the next read shuffles from. The
/// subgroup barrier is not emitted here — a read states the scope it is ordered at only when a
/// recorded expr is owed for it, which is what keeps an unordered read from carrying one.
fn exchange_through_a_warp(
    held: Option<Slot>,
    shape: Slice,
    index: Slot,
    ids: &Ids,
    literals: &mut Literals,
    code: &mut Vec<Inst>,
    next: &mut u32,
    at: usize,
) -> Result<Slot, SpirvRefusal> {
    let staged = held.ok_or(SpirvRefusal::WarpExchangeWithoutAWrite { at })?;
    let source = as_lane_id(index, ids, literals, code, next, at)?;
    let exchanged = *next;
    *next += 1;
    code.push(Inst::new(
        op::GROUP_NON_UNIFORM_SHUFFLE,
        vec![
            ids.type_of(shape.class),
            exchanged,
            // **The scope is an operand, not an implied default**: it is what confines the
            // exchange to the reader's own subgroup.
            ids.subgroup_scope,
            staged.id,
            source,
        ],
    ));
    Ok(scalar(exchanged, shape.class))
}

/// The `Float` a `BFloat16` buffer element is handed to the body as.
///
/// # Invariant
/// A brain float is the **top sixteen bits** of the float it rounds to, so the widening is
/// `OpUConvert` to the 32-bit unsigned, a logical shift up by sixteen, and an `OpBitcast` —
/// a bitcast, not a conversion, which would answer the number rather than the bits. The result
/// is typed `Float` on purpose: a body never holds a `BFloat16`, so no bf16 arithmetic is reachable.
fn widen_a_half(ids: &Ids, half: u32, code: &mut Vec<Inst>, next: &mut u32) -> u32 {
    let widened = *next;
    *next += 1;
    let shifted = *next;
    *next += 1;
    let result = *next;
    *next += 1;
    code.push(Inst::new(op::U_CONVERT, vec![ids.uint, widened, half]));
    code.push(Inst::new(
        op::SHIFT_LEFT_LOGICAL,
        vec![ids.uint, shifted, widened, ids.half_shift],
    ));
    code.push(Inst::new(op::BITCAST, vec![ids.float, result, shifted]));
    result
}

/// The 16-bit integer a `Float` is stored into a `BFloat16` buffer as.
///
/// # Invariant
/// **The mirror of [`widen_a_half`]**, and the expr is the only place a brain float changes
/// width: the shift drops the low sixteen bits rather than rounding them, because the target is
/// exactly the high half the value came from.
fn narrow_a_half(ids: &Ids, value: u32, code: &mut Vec<Inst>, next: &mut u32) -> u32 {
    let bits = *next;
    *next += 1;
    let shifted = *next;
    *next += 1;
    let narrowed = *next;
    *next += 1;
    code.push(Inst::new(op::BITCAST, vec![ids.uint, bits, value]));
    code.push(Inst::new(
        op::SHIFT_RIGHT_LOGICAL,
        vec![ids.uint, shifted, bits, ids.half_shift],
    ));
    code.push(Inst::new(op::U_CONVERT, vec![ids.half, narrowed, shifted]));
    narrowed
}

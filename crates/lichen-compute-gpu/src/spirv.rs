//! The SPIR-V emitter: a [`KernelFragment`] to a SPIR-V compute module.
//!
//! # Why this is hand-written
//!
//! The IR has a handful of value-producing operations and two buffer
//! operations, so the whole emitter is a small stack machine over
//! [`KernelInstr`]. A builder crate would not remove the part that actually goes
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
//! [`as_scalar`]/[`as_condition`]). Same IR, same semantics, two spellings.

use std::collections::HashMap;
use std::fmt;

use lichen_kernel_ir::{KernelBin, KernelFragment, KernelInstr};

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
    pub const I_ADD: u16 = 128;
    pub const I_SUB: u16 = 130;
    pub const I_MUL: u16 = 132;
    pub const U_DIV: u16 = 134;
    pub const U_MOD: u16 = 137;
    pub const SELECT: u16 = 169;
    pub const I_EQUAL: u16 = 170;
    pub const I_NOT_EQUAL: u16 = 171;
    pub const U_GREATER_THAN: u16 = 172;
    pub const U_GREATER_THAN_EQUAL: u16 = 174;
    pub const U_LESS_THAN: u16 = 176;
    pub const U_LESS_THAN_EQUAL: u16 = 178;
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

/// The bytes one `i64` buffer element occupies: the array stride, and the same
/// number the CPU path uses for its `Vec<i64>`.  That shared number is what makes
/// the two backends' results comparable element for element.
const ELEMENT_STRIDE: u32 = 8;

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
    /// The body does not leave exactly the one value a compute shader needs.
    ResultArity { results: usize, left: usize },
    /// The stack did not balance: an instruction popped more than it pushed.
    UnbalancedStack { at: usize },
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
            SpirvRefusal::ResultArity { results, left } => write!(
                f,
                "the fragment declares {results} result(s) and its body leaves {left} value(s) on \
                 the stack. A compute shader communicates through its storage buffers, so this \
                 backend requires a fragment whose body is one balanced value."
            ),
            SpirvRefusal::UnbalancedStack { at } => write!(
                f,
                "instruction {at} popped from an empty stack; the lowered body is not a balanced \
                 stack program."
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

/// A stack operand: the SSA id of a value, the constant it denotes when the
/// instruction that produced it was a `Const`, and whether its type is this
/// target's `bool` rather than the fragment's 64-bit integer.
///
/// The constant is tracked beside the id because it serves a *different*
/// purpose. A buffer operation's position selects **which** storage-buffer
/// variable to reach — a compile-time choice, so it has to be read off the stack
/// as a number. The element index is the other stack value and *is* dynamic, and
/// goes to `OpAccessChain` as an id.
///
/// # Why the third field exists
///
/// **The IR is untyped and this target is not.** A comparison here yields
/// `OpTypeBool` — the shape `OpSelect` wants, and the one place this backend
/// differs from wasm's, whose comparisons yield a narrow integer instead. But
/// the *language* says a comparison yields the `0`/`1` scalar, and a kernel may
/// use it as one: a bitwise operand (`(a < b) & c`), a comparison's own operand
/// (`(a < b) == c`), a stored element, a `Select` arm. In every one of those
/// positions a bool is the wrong type, and a module that mixed them would be
/// rejected by the driver rather than compute something else. So each slot
/// records which of the two it holds, and the two coercions
/// ([`as_scalar`]/[`as_condition`]) are emitted exactly where a position demands
/// the other — which for the common case (`if` over a comparison) is nowhere.
type Slot = (u32, Option<i64>, bool);

/// A slot holding the fragment's 64-bit scalar.
const fn scalar(id: u32) -> Slot {
    (id, None, false)
}

/// A slot holding a comparison's `OpTypeBool`.
const fn condition(id: u32) -> Slot {
    (id, None, true)
}

/// Every module-scope id, allocated before anything is emitted.
struct Ids {
    main: u32,
    label: u32,
    void: u32,
    boolean: u32,
    /// The fragment's own integer type: **unsigned** 64-bit, because the
    /// language's `Int` is a machine-sized unsigned integer.  It is what every
    /// value in the body has, what a buffer element holds, and the type the
    /// unsigned opcodes (`OpUDiv`, `OpUMod`, `OpULessThan`, …) require — SPIR-V
    /// checks that operand, so declaring this signed would make a kernel that
    /// divides, takes a remainder or compares *invalid* rather than wrong.
    /// `spirv-val` rules on that offline; see the module docs.
    ulong: u32,
    /// The 32-bit unsigned type of an invocation id's component, and of an
    /// access chain's member indices.
    uint: u32,
    v3uint: u32,
    /// The single-member struct a storage buffer's element type must be.
    ///
    /// Vulkan requires a `StorageBuffer` variable to be typed as a struct, or an
    /// array of one — a bare runtime array is the older `Uniform` + `BufferBlock`
    /// style and is rejected (`VUID-StandaloneSpirv-Uniform-06807`).
    elem: u32,
    array: u32,
    /// The struct that *wraps* the runtime array.  Vulkan requires a
    /// `StorageBuffer` variable to be typed as a struct, and a runtime array may
    /// only be the final member of one, so the buffer takes two struct levels:
    /// the element struct, and this one holding the array.
    buffer_struct: u32,
    ptr_in: u32,
    ptr_array: u32,
    /// A pointer to one element of a storage buffer, in the storage buffer
    /// storage class — what an access chain produces.
    ptr_ulong: u32,
    fn_ty: u32,
    /// The 32-bit `0` an access chain's member indices are built from.
    zero: u32,
    /// The 64-bit `1` and `0` a comparison is materialised into a scalar with —
    /// the operands of the `OpSelect` [`as_scalar`] emits.
    one_ulong: u32,
    zero_ulong: u32,
    gid: u32,
    /// The first of `binding.total()` consecutive storage-buffer variables.
    buffers: u32,
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
fn index_local(fragment: &KernelFragment) -> Option<u32> {
    let arity = fragment.param_shape.flat_arity();
    (arity > 0).then(|| (arity - 1) as u32)
}

/// Compile one fragment to SPIR-V words.
pub fn compile(fragment: &KernelFragment, binding: Binding) -> Result<Vec<u32>, SpirvRefusal> {
    if fragment.int_width.bits() != 64 {
        return Err(SpirvRefusal::UnsupportedIntWidth {
            bits: fragment.int_width.bits(),
        });
    }
    let index = index_local(fragment).ok_or(SpirvRefusal::ResultArity {
        results: fragment.results,
        left: 0,
    })?;

    // Pass 1 — allocate every module-scope id, then walk the body into an
    // instruction list. The function-local ids come from `next`, which starts
    // after the last module-scope id and ends as the module's id bound.
    let ids = Ids {
        main: 1,
        label: 2,
        void: 3,
        boolean: 4,
        ulong: 5,
        uint: 6,
        v3uint: 7,
        elem: 8,
        array: 9,
        buffer_struct: 10,
        ptr_in: 11,
        ptr_array: 12,
        ptr_ulong: 13,
        fn_ty: 14,
        zero: 15,
        one_ulong: 16,
        zero_ulong: 17,
        gid: 18,
        buffers: 19,
    };
    let mut next = ids.buffers + binding.total() as u32;
    let mut constants: HashMap<i64, u32> = HashMap::new();
    // `OpConstant` is a *module-scope* instruction, so the body's literals are
    // collected here and emitted with the types rather than inside the function.
    let mut literal_decls: Vec<Inst> = Vec::new();
    let mut code: Vec<Inst> = Vec::new();
    let mut stack: Vec<Slot> = Vec::new();

    // The index value: the invocation id's x component, widened to the fragment's
    // 64-bit integer. An index is never negative, so the widening is exact and
    // the CPU path's index parameter is the same value.
    let index_value = next;
    next += 1;
    {
        let loaded = next;
        next += 1;
        let component = next;
        next += 1;
        code.push(Inst::new(op::LOAD, vec![ids.v3uint, loaded, ids.gid]));
        code.push(Inst::new(
            op::COMPOSITE_EXTRACT,
            vec![ids.uint, component, loaded, 0],
        ));
        code.push(Inst::new(
            op::U_CONVERT,
            vec![ids.ulong, index_value, component],
        ));
    }

    for (at, instruction) in fragment.body.iter().enumerate() {
        match instruction {
            KernelInstr::Const(value) => {
                // A 64-bit constant is emitted once no matter how often the body
                // pushes it: SPIR-V requires every id to be defined exactly once.
                let id = if let Some(id) = constants.get(value) {
                    *id
                } else {
                    let id = next;
                    next += 1;
                    literal_decls.push(Inst::new(
                        op::CONSTANT,
                        vec![ids.ulong, id, *value as u32, (*value >> 32) as u32],
                    ));
                    constants.insert(*value, id);
                    id
                };
                stack.push((id, Some(*value), false));
            }
            KernelInstr::Bin(operator) => {
                let rhs = pop(&mut stack, at)?;
                let lhs = pop(&mut stack, at)?;
                // A comparison is the one operator whose operands may not be
                // scalars — `(a < b) == c` compares the *scalar* a comparison
                // means — and the one whose result is not one.  Every other
                // operator takes and produces the fragment's 64-bit integer.
                let (lhs, rhs) = (
                    as_scalar(lhs, &ids, &mut code, &mut next),
                    as_scalar(rhs, &ids, &mut code, &mut next),
                );
                let result = next;
                next += 1;
                let (opcode, result_type, result_is_condition) = match operator {
                    KernelBin::Add => (op::I_ADD, ids.ulong, false),
                    KernelBin::Sub => (op::I_SUB, ids.ulong, false),
                    KernelBin::Mul => (op::I_MUL, ids.ulong, false),
                    // An `Int` is unsigned: `OpSDiv`/`OpSRem` would agree below
                    // 2^63 and differ above, silently.  `OpUMod` is the
                    // remainder the language's `%` means (`OpSRem` rounds
                    // toward zero, which is a different function altogether).
                    KernelBin::Div => (op::U_DIV, ids.ulong, false),
                    KernelBin::Rem => (op::U_MOD, ids.ulong, false),
                    // A comparison yields a bool on this target, which is exactly
                    // what `Select` consumes — the wasm backend's widening to a
                    // scalar has no counterpart here, and no cost.
                    KernelBin::Lt => (op::U_LESS_THAN, ids.boolean, true),
                    KernelBin::Gt => (op::U_GREATER_THAN, ids.boolean, true),
                    KernelBin::Leq => (op::U_LESS_THAN_EQUAL, ids.boolean, true),
                    KernelBin::Geq => (op::U_GREATER_THAN_EQUAL, ids.boolean, true),
                    KernelBin::Eq => (op::I_EQUAL, ids.boolean, true),
                    KernelBin::Neq => (op::I_NOT_EQUAL, ids.boolean, true),
                    // The bitwise operators, which over two comparison results
                    // are the language's `and`/`xor`/`or` — the place a
                    // comparison's scalar materialisation is actually paid for.
                    KernelBin::BitAnd => (op::BITWISE_AND, ids.ulong, false),
                    KernelBin::BitOr => (op::BITWISE_OR, ids.ulong, false),
                    KernelBin::BitXor => (op::BITWISE_XOR, ids.ulong, false),
                };
                code.push(Inst::new(opcode, vec![result_type, result, lhs.0, rhs.0]));
                stack.push(if result_is_condition {
                    condition(result)
                } else {
                    scalar(result)
                });
            }
            KernelInstr::LocalGet(local) => {
                if *local != index {
                    return Err(SpirvRefusal::NonIndexParameter { local: *local, at });
                }
                stack.push(scalar(index_value));
            }
            // The condition a `select` needs.  **Not a no-op here**: wasm's
            // `i32.wrap_i64` narrows an `i64` condition to the `i32` its
            // `select` takes, and this target's `select` takes a *bool*, so the
            // same instruction is where an `i64` condition becomes one.  When
            // the condition is already a comparison's bool — which is what the
            // emitter in `lichen-compute` produces for an `if` — it is a no-op,
            // and that is the case the module docs describe.
            KernelInstr::I32WrapI64 => {
                let popped = pop(&mut stack, at)?;
                stack.push(as_condition(popped, &ids, &mut code, &mut next));
            }
            KernelInstr::Select => {
                let selector = pop(&mut stack, at)?;
                let otherwise = pop(&mut stack, at)?;
                let then = pop(&mut stack, at)?;
                // The arms are the language's scalars (a `select`'s result type
                // is its arms' type, and a scalar is what a lichen value is),
                // and the selector is the bool `select` takes.
                let then = as_scalar(then, &ids, &mut code, &mut next);
                let otherwise = as_scalar(otherwise, &ids, &mut code, &mut next);
                let selector = as_condition(selector, &ids, &mut code, &mut next);
                let result = next;
                next += 1;
                code.push(Inst::new(
                    op::SELECT,
                    vec![ids.ulong, result, selector.0, then.0, otherwise.0],
                ));
                stack.push(scalar(result));
            }
            KernelInstr::BufferReadCall => {
                let element = pop(&mut stack, at)?;
                let position = pop(&mut stack, at)?;
                let element = as_scalar(element, &ids, &mut code, &mut next);
                let slot = buffer_slot(position, at, 0, binding.inputs, "input")?;
                let pointer = next;
                next += 1;
                let loaded = next;
                next += 1;
                // Three indices: the buffer struct's only member, then the element
                // within that runtime array, then the element struct's only member.
                code.push(Inst::new(
                    op::ACCESS_CHAIN,
                    vec![
                        ids.ptr_ulong,
                        pointer,
                        ids.buffers + slot as u32,
                        ids.zero,
                        element.0,
                        ids.zero,
                    ],
                ));
                code.push(Inst::new(op::LOAD, vec![ids.ulong, loaded, pointer]));
                stack.push(scalar(loaded));
            }
            KernelInstr::BufferWriteCall => {
                let value = pop(&mut stack, at)?;
                let element = pop(&mut stack, at)?;
                let position = pop(&mut stack, at)?;
                // A buffer element is a 64-bit scalar whatever the body computed
                // it as, so a comparison stored into one is materialised here.
                let value = as_scalar(value, &ids, &mut code, &mut next);
                let element = as_scalar(element, &ids, &mut code, &mut next);
                let slot = buffer_slot(position, at, binding.inputs, binding.outputs, "output")?;
                let pointer = next;
                next += 1;
                code.push(Inst::new(
                    op::ACCESS_CHAIN,
                    vec![
                        ids.ptr_ulong,
                        pointer,
                        ids.buffers + slot as u32,
                        ids.zero,
                        element.0,
                        ids.zero,
                    ],
                ));
                code.push(Inst::new(op::STORE, vec![pointer, value.0]));
            }
            KernelInstr::CallKernel(kernel) => {
                return Err(SpirvRefusal::CrossKernelCall {
                    kernel: *kernel,
                    at,
                });
            }
        }
    }

    if stack.len() != 1 {
        return Err(SpirvRefusal::ResultArity {
            results: fragment.results,
            left: stack.len(),
        });
    }

    Ok(assemble(&ids, binding, &literal_decls, &code, next))
}

/// Write the module in SPIR-V's required section order.
///
/// The order is not a style choice: the entry point must precede the function it
/// names, annotations must precede the types and variables they decorate, and
/// types must precede their use. All of that is legal only because every id was
/// reserved up front.
fn assemble(ids: &Ids, binding: Binding, literals: &[Inst], code: &[Inst], bound: u32) -> Vec<u32> {
    let mut out = vec![SPIRV_MAGIC, SPIRV_VERSION, GENERATOR, bound, 0];

    let emit_all = |out: &mut Vec<u32>, instructions: &[Inst]| {
        for instruction in instructions {
            instruction.encode(out);
        }
    };

    // 1. Capabilities. `Int64` is unconditional: this emitter's only numeric type
    // is the fragment's 64-bit integer, and a fragment using it needs the
    // capability whether or not the particular values happen to be small.
    emit_all(
        &mut out,
        &[
            Inst::new(op::CAPABILITY, vec![capability::SHADER]),
            Inst::new(op::CAPABILITY, vec![capability::INT64]),
        ],
    );

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
    let mut annotations = vec![Inst::new(
        op::DECORATE,
        vec![ids.array, decoration::ARRAY_STRIDE, ELEMENT_STRIDE],
    )];
    // A storage buffer's struct member needs its byte offset, and the sole
    // member sits at zero.
    annotations.push(Inst::new(
        op::MEMBER_DECORATE,
        vec![ids.elem, 0, decoration::OFFSET, 0],
    ));
    // …and the struct that *contains* the runtime array must say so, or the
    // module does not describe a block-backed resource at all. A `Block` struct
    // has to be explicitly laid out, so its member needs an offset too.
    annotations.push(Inst::new(
        op::DECORATE,
        vec![ids.buffer_struct, decoration::BLOCK],
    ));
    annotations.push(Inst::new(
        op::MEMBER_DECORATE,
        vec![ids.buffer_struct, 0, decoration::OFFSET, 0],
    ));
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
    let types = vec![
        Inst::new(op::TYPE_VOID, vec![ids.void]),
        Inst::new(op::TYPE_BOOL, vec![ids.boolean]),
        // The fragment's own 64-bit integer is **unsigned** (signedness `0`):
        // the language's `Int` is, and the unsigned opcodes a kernel divides,
        // takes a remainder and compares with require it. See [`Ids::ulong`].
        Inst::new(op::TYPE_INT, vec![ids.ulong, 64, 0]),
        Inst::new(op::TYPE_INT, vec![ids.uint, 32, 0]),
        Inst::new(op::TYPE_VECTOR, vec![ids.v3uint, ids.uint, 3]),
        Inst::new(op::TYPE_STRUCT, vec![ids.elem, ids.ulong]),
        Inst::new(op::TYPE_RUNTIME_ARRAY, vec![ids.array, ids.elem]),
        Inst::new(op::TYPE_STRUCT, vec![ids.buffer_struct, ids.array]),
        Inst::new(
            op::TYPE_POINTER,
            vec![ids.ptr_in, storage_class::INPUT, ids.v3uint],
        ),
        Inst::new(
            op::TYPE_POINTER,
            vec![
                ids.ptr_array,
                storage_class::STORAGE_BUFFER,
                ids.buffer_struct,
            ],
        ),
        Inst::new(
            op::TYPE_POINTER,
            vec![ids.ptr_ulong, storage_class::STORAGE_BUFFER, ids.ulong],
        ),
        Inst::new(op::TYPE_FUNCTION, vec![ids.fn_ty, ids.void]),
    ];
    emit_all(&mut out, &types);

    let mut constants = vec![
        Inst::new(op::CONSTANT, vec![ids.uint, ids.zero, 0]),
        // The i64 `1` and `0` a comparison is materialised into a scalar with.
        // Declared unconditionally with the other constants — `OpConstant` is
        // module-scope, and an unused constant is legal — because which body
        // needs them is known only after the walk above.
        Inst::new(op::CONSTANT, vec![ids.ulong, ids.one_ulong, 1, 0]),
        Inst::new(op::CONSTANT, vec![ids.ulong, ids.zero_ulong, 0, 0]),
    ];
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
        globals.push(Inst::new(
            op::VARIABLE,
            vec![
                ids.ptr_array,
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

/// Pop one operand, or refuse.
fn pop(stack: &mut Vec<Slot>, at: usize) -> Result<Slot, SpirvRefusal> {
    stack.pop().ok_or(SpirvRefusal::UnbalancedStack { at })
}

/// The fragment's 64-bit scalar for `slot`, materialising a comparison's bool.
///
/// The language says a comparison yields `0`/`1`, so a bool that reaches a
/// scalar position has to become that scalar — `OpSelect` over the two i64
/// constants is the conversion (there is no `OpConvertBoolToInt`; a bool's
/// stored form is not defined).
///
/// **A bool already popped as a scalar is not the same thing as one that never
/// was**, which is why the flag rides on the slot rather than being re-derived:
/// the IR is untyped, so "was this a comparison" is knowledge only this walk
/// has.
fn as_scalar(slot: Slot, ids: &Ids, code: &mut Vec<Inst>, next: &mut u32) -> Slot {
    let (id, constant, is_condition) = slot;
    if !is_condition {
        return slot;
    }
    let result = *next;
    *next += 1;
    code.push(Inst::new(
        op::SELECT,
        vec![ids.ulong, result, id, ids.one_ulong, ids.zero_ulong],
    ));
    (result, constant, false)
}

/// The `bool` a `select` takes for `slot`, converting a scalar.
///
/// **Non-zero is true**, which is what wasm's `select` means by its `i32`
/// condition — the instruction this one stands in for (`I32WrapI64`) narrows
/// there and converts here, so the two targets agree on what a condition is.
fn as_condition(slot: Slot, ids: &Ids, code: &mut Vec<Inst>, next: &mut u32) -> Slot {
    let (id, constant, is_condition) = slot;
    if is_condition {
        return slot;
    }
    let result = *next;
    *next += 1;
    code.push(Inst::new(
        op::I_NOT_EQUAL,
        vec![ids.boolean, result, id, ids.zero_ulong],
    ));
    (result, constant, true)
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
        .1
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

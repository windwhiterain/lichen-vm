//! The artifact's own schema: the JSON spelling of a fused kernel.
//! See `docs/notes/graph-fusion.md` §5.
//!
//! # Invariant
//! Every type here mirrors an IR type and names it as the JSON spells it, so a
//! reader never invents a value the text did not carry. `Plan` and its parts are
//! also a placement pack's spelling (`lichen-language`), so one plan text serves
//! both artifacts and neither can drift from the other.

use facet::Facet;

/// One scalar class, as the artifact spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
#[facet(rename_all = "lowercase")]
pub enum ScalarClass {
    /// The IR's `Int`.
    Int,
    /// The IR's `Float`.
    Float,
}

/// The integer width a fragment was lowered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
pub enum IntWidth {
    /// The IR's `IntWidth::I64`.
    #[facet(rename = "i64")]
    I64,
    /// The 32-bit width no IR variant names yet.
    #[facet(rename = "i32")]
    I32,
}

/// Where a slice's elements live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
#[facet(rename_all = "lowercase")]
pub enum Space {
    /// One array per lane.
    Register,
    /// One array per workgroup.
    Shared,
}

/// A binary operator of the kernel-safe subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(u8)]
#[facet(rename_all = "lowercase")]
pub enum KernelBin {
    /// `add`.
    Add,
    /// `sub`.
    Sub,
    /// `mul`.
    Mul,
    /// `div`.
    Div,
    /// `rem`.
    Rem,
    /// `max`.
    Max,
    /// `min`.
    Min,
    /// `lt`.
    Lt,
    /// `gt`.
    Gt,
    /// `leq`.
    Leq,
    /// `geq`.
    Geq,
    /// `eq`.
    Eq,
    /// `neq`.
    Neq,
    /// `bit_and`.
    #[facet(rename = "bit_and")]
    BitAnd,
    /// `bit_or`.
    #[facet(rename = "bit_or")]
    BitOr,
    /// `bit_xor`.
    #[facet(rename = "bit_xor")]
    BitXor,
}

/// A named value's number, spelled as a bare JSON number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(transparent)]
#[facet(transparent)]
pub struct ValueId(pub u32);

/// One abstract instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub enum KernelInstr {
    /// Push a constant.
    #[facet(rename = "const")]
    Const {
        /// The class the bits are read in.
        class: ScalarClass,
        /// The constant's bits.
        bits: i64,
    },
    /// A binary operator, in the class it names.
    #[facet(rename = "bin")]
    Bin {
        /// The class the operator works in.
        class: ScalarClass,
        /// The operator.
        op: KernelBin,
    },
    /// Convert a value to the condition width a `select` needs.
    #[facet(rename = "i32_wrap_i64")]
    I32WrapI64,
    /// A three-operand select.
    #[facet(rename = "select")]
    Select,
    /// The exponential of one `Float`.
    #[facet(rename = "exp")]
    Exp,
    /// The square root of one `Float`.
    #[facet(rename = "sqrt")]
    Sqrt,
    /// The largest integral `Float` not above one `Float`.
    #[facet(rename = "floor")]
    Floor,
    /// A cross-kernel call.
    #[facet(rename = "call_kernel")]
    CallKernel {
        /// The callee's registry id.
        kernel: u64,
    },
    /// A class conversion.
    #[facet(rename = "conv")]
    Conv {
        /// The class the operand is.
        from: ScalarClass,
        /// The class it leaves behind.
        to: ScalarClass,
    },
    /// Read one element of one input buffer.
    #[facet(rename = "buffer_read_call")]
    BufferReadCall {
        /// The buffer's element class.
        class: ScalarClass,
    },
    /// Write one element of one output buffer.
    #[facet(rename = "buffer_write_call")]
    BufferWriteCall {
        /// The buffer's element class.
        class: ScalarClass,
    },
    /// Write one element of one slice.
    #[facet(rename = "slice_write_call")]
    SliceWriteCall {
        /// The slice's element class.
        class: ScalarClass,
    },
    /// Read one element of one slice.
    #[facet(rename = "slice_read_call")]
    SliceReadCall {
        /// The slice's element class.
        class: ScalarClass,
    },
}

/// A branch and the values it hands over.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Br {
    /// The block control arrives at.
    pub target: u64,
    /// The values handed over, one per parameter of the target.
    pub args: Vec<ValueId>,
}

/// Where a block goes when its instructions have run.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub enum Terminator {
    /// Leave the function, carrying these values.
    #[facet(rename = "return")]
    Return {
        /// The values returned.
        values: Vec<ValueId>,
    },
    /// Arrive at `target`.
    #[facet(rename = "br")]
    Br(Br),
    /// Two-way branch on `cond`.
    #[facet(rename = "cond_br")]
    CondBr {
        /// The condition, an `Int` `0`/`1` scalar.
        cond: ValueId,
        /// Where a true condition goes.
        if_true: Br,
        /// Where a false condition goes.
        if_false: Br,
    },
}

/// What a value is.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub enum ValueDef {
    /// A value a block receives.
    #[facet(rename = "block_param")]
    BlockParam {
        /// The block that receives it.
        block: u64,
    },
    /// A computation.
    #[facet(rename = "instr")]
    Instr {
        /// The operator.
        op: KernelInstr,
        /// The values it reads.
        args: Vec<ValueId>,
        /// The classes it leaves behind, per result.
        classes: Vec<ScalarClass>,
    },
}

/// One basic block.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct BasicBlock {
    /// The values this block receives.
    pub params: Vec<ValueId>,
    /// The values this block computes, in the emitter's order.
    pub instrs: Vec<ValueId>,
    /// Where the block goes.
    pub terminator: Terminator,
}

/// A separated write/read pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Barrier {
    /// The slice write that must have happened.
    pub write: ValueId,
    /// The read that must observe it.
    pub read: ValueId,
}

/// A fragment's body.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Body {
    /// The value table, in `ValueId` order: a value's position **is** its identity.
    pub values: Vec<ValueDef>,
    /// The blocks.
    pub blocks: Vec<BasicBlock>,
    /// Which block control starts in.
    pub entry: u64,
    /// The pairs a barrier is owed for.
    pub barriers: Vec<Barrier>,
}

/// A fragment's parameter domain.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub enum Shape {
    /// One scalar leaf.
    #[facet(rename = "scalar")]
    Scalar(ScalarClass),
    /// A tuple, flattened in order.
    #[facet(rename = "tuple")]
    Tuple(Vec<Shape>),
}

/// The role of every field of a parameter struct.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Roles {
    /// Scalar parameter paths, in order.
    pub scalars: Vec<Vec<u64>>,
    /// Input buffer payload paths, in order.
    pub inputs: Vec<Vec<u64>>,
    /// Output buffer payload paths, in order.
    pub outputs: Vec<Vec<u64>>,
    /// The class each input buffer's parameter states, per `inputs`.
    pub input_classes: Vec<Option<ScalarClass>>,
    /// The class each output buffer's parameter states, per `outputs`.
    pub output_classes: Vec<Option<ScalarClass>>,
}

/// The facts a fragment declares about its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Target {
    /// The width the body's integers were lowered to mean.
    pub int_width: IntWidth,
    /// The lane count one workgroup runs.
    pub lane_width: u32,
}

/// One memory slice a fragment declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Slice {
    /// The class of one element.
    pub class: ScalarClass,
    /// How many elements the slice holds.
    pub count: u32,
    /// Where the elements live.
    pub space: Space,
}

/// One field of a mark's payload.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct MarkField {
    /// The field's name, or `null` for a bare text.
    pub name: Option<String>,
    /// The field's value, as text.
    pub text: String,
}

/// One mark: the value it rode and the payload it carried.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Mark {
    /// The body value the marked expression computed.
    pub value: ValueId,
    /// The payload, one entry per field.
    pub payload: Vec<MarkField>,
}

/// A compiled kernel-callable unit.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Fragment {
    /// The parameter domain.
    pub param_shape: Shape,
    /// Where the parameter's leaves sit.
    pub roles: Roles,
    /// The lowered body.
    pub body: Body,
    /// How many input buffers the fragment reads.
    pub inputs: u64,
    /// How many output buffers it writes.
    pub outputs: u64,
    /// The element class of each input buffer, in order.
    pub input_classes: Vec<ScalarClass>,
    /// The element class of each output buffer, in order.
    pub output_classes: Vec<ScalarClass>,
    /// The class of each result, in order.
    pub result_classes: Vec<ScalarClass>,
    /// The target facts the fragment declares.
    pub target: Target,
    /// Each slice the body addresses, by ordinal.
    pub slices: Vec<Slice>,
    /// The marks expressions in the body carried.
    pub marks: Vec<Mark>,
}

/// The interface a kernel implements.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Claim {
    /// The interface's name.
    pub interface: String,
    /// The roles the interface requires.
    pub roles: Vec<String>,
}

/// One staged value's placement.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Staging {
    /// The mark the body published for the value.
    pub name: String,
    /// Where the value lives.
    pub space: Space,
    /// The group width this placement is checked against.
    pub group_width: u32,
}

/// Where a region's staged values live.
#[derive(Debug, Clone, Default, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Plan {
    /// One entry per value the plan stages.
    pub staging: Vec<Staging>,
}

/// A fused kernel, as the text spells it.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
#[facet(deny_unknown_fields)]
pub struct Artifact {
    /// The fragment.
    pub fragment: Fragment,
    /// The interface it is interned with, or `null`.
    pub claim: Option<Claim>,
    /// The plan it was checked against.
    pub plan: Plan,
}

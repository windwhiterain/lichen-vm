//! The two exhaustive conversions between the artifact and the IR, and the
//! refusal a text earns when it names nothing.
//!
//! # Invariant
//! Every IR variant is matched on both sides with no catch-all, so a variant
//! added to `lichen-kernel-ir` is a compile error here rather than a silent
//! omission from the text.

use crate::fused::schema;
use crate::fused::{FusedKernel, FusedRefusal};
use lichen_kernel_ir as ir;

/// Where a conversion found a name that holds nothing, and what it named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NotAnIrValue {
    /// The path the offending entry sits at.
    pub path: String,
    /// What was found there.
    pub found: String,
}

impl NotAnIrValue {
    /// A refusal for `path`, naming what was found there.
    fn at(path: impl Into<String>, found: impl Into<String>) -> Self {
        NotAnIrValue {
            path: path.into(),
            found: found.into(),
        }
    }
}

impl From<NotAnIrValue> for FusedRefusal {
    fn from(broken: NotAnIrValue) -> Self {
        FusedRefusal::NotAnIrValue {
            path: Some(broken.path),
            found: broken.found,
        }
    }
}

/// A `usize` that fits the field it is written in, or the refusal naming it.
fn wide(at: &str, value: u64) -> Result<usize, NotAnIrValue> {
    usize::try_from(value).map_err(|_| {
        NotAnIrValue::at(
            at,
            format!("the number {value}, which is wider than this machine's index"),
        )
    })
}

/// The value table's length, which every `ValueId` in a body is checked against.
struct Table {
    values: usize,
}

impl Table {
    /// The number a `ValueId` names, or the refusal saying it names nothing.
    ///
    /// # Invariant
    /// The check is the table's own length, so an operand cannot reach past it:
    /// a conversion that indexed instead would panic where a hand-edited text is
    /// the input.
    fn id(&self, at: &str, value: schema::ValueId) -> Result<ir::ValueId, NotAnIrValue> {
        if usize::try_from(value.0).map_or(true, |index| index >= self.values) {
            return Err(NotAnIrValue::at(
                at,
                format!(
                    "value {}, and this body's table holds {} value(s)",
                    value.0, self.values
                ),
            ));
        }
        Ok(ir::ValueId(value.0))
    }

    /// Every value in `values`, in order.
    fn ids(&self, at: &str, values: &[schema::ValueId]) -> Result<Vec<ir::ValueId>, NotAnIrValue> {
        values
            .iter()
            .enumerate()
            .map(|(position, value)| self.id(&format!("{at}[{position}]"), *value))
            .collect()
    }
}

impl From<schema::ScalarClass> for ir::ScalarClass {
    fn from(class: schema::ScalarClass) -> Self {
        match class {
            schema::ScalarClass::Int => ir::ScalarClass::Int,
            schema::ScalarClass::Float => ir::ScalarClass::Float,
            schema::ScalarClass::BFloat16 => ir::ScalarClass::BFloat16,
        }
    }
}

impl From<ir::ScalarClass> for schema::ScalarClass {
    fn from(class: ir::ScalarClass) -> Self {
        match class {
            ir::ScalarClass::Int => schema::ScalarClass::Int,
            ir::ScalarClass::Float => schema::ScalarClass::Float,
            ir::ScalarClass::BFloat16 => schema::ScalarClass::BFloat16,
        }
    }
}

/// The integer width a fragment declares, or the refusal naming a width no IR
/// variant holds.
///
/// # Invariant
/// A width is a target fact the whole body was lowered against, so one the IR
/// cannot name is refused rather than rounded to a width it can.
fn int_width(at: &str, width: schema::IntWidth) -> Result<ir::IntWidth, NotAnIrValue> {
    match width {
        schema::IntWidth::I64 => Ok(ir::IntWidth::I64),
        schema::IntWidth::I32 => Err(NotAnIrValue::at(
            at,
            "the width `i32`, which the kernel IR names no variant for",
        )),
    }
}

impl From<ir::IntWidth> for schema::IntWidth {
    fn from(width: ir::IntWidth) -> Self {
        match width {
            ir::IntWidth::I64 => schema::IntWidth::I64,
        }
    }
}

impl From<schema::Space> for ir::Space {
    fn from(space: schema::Space) -> Self {
        match space {
            schema::Space::Register => ir::Space::Register,
            schema::Space::Warp => ir::Space::Warp,
            schema::Space::Shared => ir::Space::Shared,
        }
    }
}

impl From<ir::Space> for schema::Space {
    fn from(space: ir::Space) -> Self {
        match space {
            ir::Space::Register => schema::Space::Register,
            ir::Space::Warp => schema::Space::Warp,
            ir::Space::Shared => schema::Space::Shared,
        }
    }
}

impl From<schema::KernelBin> for ir::KernelBin {
    fn from(op: schema::KernelBin) -> Self {
        match op {
            schema::KernelBin::Add => ir::KernelBin::Add,
            schema::KernelBin::Sub => ir::KernelBin::Sub,
            schema::KernelBin::Mul => ir::KernelBin::Mul,
            schema::KernelBin::Div => ir::KernelBin::Div,
            schema::KernelBin::Rem => ir::KernelBin::Rem,
            schema::KernelBin::Max => ir::KernelBin::Max,
            schema::KernelBin::Min => ir::KernelBin::Min,
            schema::KernelBin::Lt => ir::KernelBin::Lt,
            schema::KernelBin::Gt => ir::KernelBin::Gt,
            schema::KernelBin::Leq => ir::KernelBin::Leq,
            schema::KernelBin::Geq => ir::KernelBin::Geq,
            schema::KernelBin::Eq => ir::KernelBin::Eq,
            schema::KernelBin::Neq => ir::KernelBin::Neq,
            schema::KernelBin::BitAnd => ir::KernelBin::BitAnd,
            schema::KernelBin::BitOr => ir::KernelBin::BitOr,
            schema::KernelBin::BitXor => ir::KernelBin::BitXor,
        }
    }
}

impl From<ir::KernelBin> for schema::KernelBin {
    fn from(op: ir::KernelBin) -> Self {
        match op {
            ir::KernelBin::Add => schema::KernelBin::Add,
            ir::KernelBin::Sub => schema::KernelBin::Sub,
            ir::KernelBin::Mul => schema::KernelBin::Mul,
            ir::KernelBin::Div => schema::KernelBin::Div,
            ir::KernelBin::Rem => schema::KernelBin::Rem,
            ir::KernelBin::Max => schema::KernelBin::Max,
            ir::KernelBin::Min => schema::KernelBin::Min,
            ir::KernelBin::Lt => schema::KernelBin::Lt,
            ir::KernelBin::Gt => schema::KernelBin::Gt,
            ir::KernelBin::Leq => schema::KernelBin::Leq,
            ir::KernelBin::Geq => schema::KernelBin::Geq,
            ir::KernelBin::Eq => schema::KernelBin::Eq,
            ir::KernelBin::Neq => schema::KernelBin::Neq,
            ir::KernelBin::BitAnd => schema::KernelBin::BitAnd,
            ir::KernelBin::BitOr => schema::KernelBin::BitOr,
            ir::KernelBin::BitXor => schema::KernelBin::BitXor,
        }
    }
}

impl From<ir::MarkField> for schema::MarkField {
    fn from(field: ir::MarkField) -> Self {
        schema::MarkField {
            name: field.name,
            text: field.text,
        }
    }
}

impl From<schema::MarkField> for ir::MarkField {
    fn from(field: schema::MarkField) -> Self {
        ir::MarkField {
            name: field.name,
            text: field.text,
        }
    }
}

impl From<ir::Slice> for schema::Slice {
    fn from(slice: ir::Slice) -> Self {
        schema::Slice {
            class: slice.class.into(),
            count: slice.count,
            space: slice.space.into(),
        }
    }
}

impl From<schema::Slice> for ir::Slice {
    fn from(slice: schema::Slice) -> Self {
        ir::Slice {
            class: slice.class.into(),
            count: slice.count,
            space: slice.space.into(),
        }
    }
}

impl From<ir::KernelClaim> for schema::Claim {
    fn from(claim: ir::KernelClaim) -> Self {
        schema::Claim {
            interface: claim.interface,
            roles: claim.roles,
        }
    }
}

impl From<schema::Claim> for ir::KernelClaim {
    fn from(claim: schema::Claim) -> Self {
        ir::KernelClaim {
            interface: claim.interface,
            roles: claim.roles,
        }
    }
}

impl From<ir::Staging> for schema::Staging {
    fn from(staging: ir::Staging) -> Self {
        schema::Staging {
            name: staging.name,
            space: staging.space.into(),
            group_width: staging.group_width,
        }
    }
}

impl From<schema::Staging> for ir::Staging {
    fn from(staging: schema::Staging) -> Self {
        ir::Staging {
            name: staging.name,
            space: staging.space.into(),
            group_width: staging.group_width,
        }
    }
}

impl From<ir::Plan> for schema::Plan {
    fn from(plan: ir::Plan) -> Self {
        schema::Plan {
            staging: plan.staging.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<schema::Plan> for ir::Plan {
    fn from(plan: schema::Plan) -> Self {
        ir::Plan {
            staging: plan.staging.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ir::KernelShape> for schema::Shape {
    fn from(shape: ir::KernelShape) -> Self {
        match shape {
            ir::KernelShape::Scalar(class) => schema::Shape::Scalar(class.into()),
            ir::KernelShape::Tuple(items) => {
                schema::Shape::Tuple(items.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl From<schema::Shape> for ir::KernelShape {
    fn from(shape: schema::Shape) -> Self {
        match shape {
            schema::Shape::Scalar(class) => ir::KernelShape::Scalar(class.into()),
            schema::Shape::Tuple(items) => {
                ir::KernelShape::Tuple(items.into_iter().map(Into::into).collect())
            }
        }
    }
}

impl From<ir::KernelRoles> for schema::Roles {
    fn from(roles: ir::KernelRoles) -> Self {
        schema::Roles {
            scalars: roles
                .scalars
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as u64).collect())
                .collect(),
            inputs: roles
                .inputs
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as u64).collect())
                .collect(),
            outputs: roles
                .outputs
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as u64).collect())
                .collect(),
            input_classes: roles
                .input_classes
                .into_iter()
                .map(|class| class.map(schema::ScalarClass::from))
                .collect(),
            output_classes: roles
                .output_classes
                .into_iter()
                .map(|class| class.map(schema::ScalarClass::from))
                .collect(),
        }
    }
}

impl From<schema::Roles> for ir::KernelRoles {
    fn from(roles: schema::Roles) -> Self {
        let output_count = roles.outputs.len();
        ir::KernelRoles {
            scalars: roles
                .scalars
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as usize).collect())
                .collect(),
            inputs: roles
                .inputs
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as usize).collect())
                .collect(),
            outputs: roles
                .outputs
                .into_iter()
                .map(|path| path.into_iter().map(|step| step as usize).collect())
                .collect(),
            input_classes: roles
                .input_classes
                .into_iter()
                .map(|class| class.map(Into::into))
                .collect(),
            output_classes: roles
                .output_classes
                .into_iter()
                .map(|class| class.map(Into::into))
                .collect(),
            // A fused schema states no declared length: each output answers `None`, and the
            // launch's element count is its extent (`KernelRoles::output_extents`).
            output_extents: (0..output_count).map(|_| None).collect(),
        }
    }
}

/// The target facts a text declares, or the refusal naming a width it cannot hold.
fn target(at: &str, facts: schema::Target) -> Result<ir::TargetFacts, NotAnIrValue> {
    Ok(ir::TargetFacts {
        int_width: int_width(&format!("{at}.int_width"), facts.int_width)?,
        lane_width: ir::LaneWidth::new(facts.lane_width),
        lane_height: ir::LaneHeight::new(facts.lane_height),
    })
}

impl From<ir::TargetFacts> for schema::Target {
    fn from(target: ir::TargetFacts) -> Self {
        schema::Target {
            int_width: target.int_width.into(),
            lane_width: target.lane_width.lanes(),
            lane_height: target.lane_height.rows(),
        }
    }
}

/// One instruction's IR form.
fn instruction(at: &str, instr: &schema::KernelInstr) -> Result<ir::KernelInstr, NotAnIrValue> {
    Ok(match *instr {
        schema::KernelInstr::Const { class, bits } => ir::KernelInstr::Const(class.into(), bits),
        schema::KernelInstr::Bin { class, op } => ir::KernelInstr::Bin(class.into(), op.into()),
        schema::KernelInstr::Fma => ir::KernelInstr::Fma,
        schema::KernelInstr::Negate => ir::KernelInstr::Negate,
        schema::KernelInstr::I32WrapI64 => ir::KernelInstr::I32WrapI64,
        schema::KernelInstr::Select => ir::KernelInstr::Select,
        schema::KernelInstr::Exp => ir::KernelInstr::Exp,
        schema::KernelInstr::Sqrt => ir::KernelInstr::Sqrt,
        schema::KernelInstr::Floor => ir::KernelInstr::Floor,
        schema::KernelInstr::Ln => ir::KernelInstr::Ln,
        schema::KernelInstr::Sin => ir::KernelInstr::Sin,
        schema::KernelInstr::Cos => ir::KernelInstr::Cos,
        schema::KernelInstr::CallKernel { kernel } => {
            ir::KernelInstr::CallKernel(wide(&format!("{at}.kernel"), kernel)?)
        }
        schema::KernelInstr::Conv { from, to } => ir::KernelInstr::Conv {
            from: from.into(),
            to: to.into(),
        },
        schema::KernelInstr::BufferReadCall { class } => {
            ir::KernelInstr::BufferReadCall(class.into())
        }
        schema::KernelInstr::BufferWriteCall { class } => {
            ir::KernelInstr::BufferWriteCall(class.into())
        }
        schema::KernelInstr::BufferAtomicAddCall { class } => {
            ir::KernelInstr::BufferAtomicAddCall(class.into())
        }
        schema::KernelInstr::SliceWriteCall { class } => {
            ir::KernelInstr::SliceWriteCall(class.into())
        }
        schema::KernelInstr::SliceReadCall { class } => {
            ir::KernelInstr::SliceReadCall(class.into())
        }
    })
}

impl From<ir::KernelInstr> for schema::KernelInstr {
    fn from(instr: ir::KernelInstr) -> Self {
        match instr {
            ir::KernelInstr::Const(class, bits) => schema::KernelInstr::Const {
                class: class.into(),
                bits,
            },
            ir::KernelInstr::Bin(class, op) => schema::KernelInstr::Bin {
                class: class.into(),
                op: op.into(),
            },
            ir::KernelInstr::Fma => schema::KernelInstr::Fma,
            ir::KernelInstr::Negate => schema::KernelInstr::Negate,
            ir::KernelInstr::I32WrapI64 => schema::KernelInstr::I32WrapI64,
            ir::KernelInstr::Select => schema::KernelInstr::Select,
            ir::KernelInstr::Exp => schema::KernelInstr::Exp,
            ir::KernelInstr::Sqrt => schema::KernelInstr::Sqrt,
            ir::KernelInstr::Floor => schema::KernelInstr::Floor,
            ir::KernelInstr::Ln => schema::KernelInstr::Ln,
            ir::KernelInstr::Sin => schema::KernelInstr::Sin,
            ir::KernelInstr::Cos => schema::KernelInstr::Cos,
            ir::KernelInstr::CallKernel(kernel) => schema::KernelInstr::CallKernel {
                kernel: kernel as u64,
            },
            ir::KernelInstr::Conv { from, to } => schema::KernelInstr::Conv {
                from: from.into(),
                to: to.into(),
            },
            ir::KernelInstr::BufferReadCall(class) => schema::KernelInstr::BufferReadCall {
                class: class.into(),
            },
            ir::KernelInstr::BufferWriteCall(class) => schema::KernelInstr::BufferWriteCall {
                class: class.into(),
            },
            ir::KernelInstr::BufferAtomicAddCall(class) => {
                schema::KernelInstr::BufferAtomicAddCall {
                    class: class.into(),
                }
            }
            ir::KernelInstr::SliceWriteCall(class) => schema::KernelInstr::SliceWriteCall {
                class: class.into(),
            },
            ir::KernelInstr::SliceReadCall(class) => schema::KernelInstr::SliceReadCall {
                class: class.into(),
            },
        }
    }
}

impl From<ir::Br> for schema::Br {
    fn from(branch: ir::Br) -> Self {
        schema::Br {
            target: branch.target as u64,
            args: branch
                .args
                .into_iter()
                .map(|value| schema::ValueId(value.0))
                .collect(),
        }
    }
}

impl From<ir::Terminator> for schema::Terminator {
    fn from(terminator: ir::Terminator) -> Self {
        match terminator {
            ir::Terminator::Return { values } => schema::Terminator::Return {
                values: values
                    .into_iter()
                    .map(|value| schema::ValueId(value.0))
                    .collect(),
            },
            ir::Terminator::Br(branch) => schema::Terminator::Br(branch.into()),
            ir::Terminator::CondBr {
                cond,
                if_true,
                if_false,
            } => schema::Terminator::CondBr {
                cond: schema::ValueId(cond.0),
                if_true: if_true.into(),
                if_false: if_false.into(),
            },
        }
    }
}

impl From<ir::BasicBlock> for schema::BasicBlock {
    fn from(block: ir::BasicBlock) -> Self {
        schema::BasicBlock {
            params: block
                .params
                .into_iter()
                .map(|value| schema::ValueId(value.0))
                .collect(),
            instrs: block
                .instrs
                .into_iter()
                .map(|value| schema::ValueId(value.0))
                .collect(),
            terminator: block.terminator.into(),
        }
    }
}

impl From<ir::Barrier> for schema::Barrier {
    fn from(barrier: ir::Barrier) -> Self {
        schema::Barrier {
            write: schema::ValueId(barrier.write.0),
            read: schema::ValueId(barrier.read.0),
        }
    }
}

impl From<ir::ValueDef> for schema::ValueDef {
    fn from(definition: ir::ValueDef) -> Self {
        match definition {
            ir::ValueDef::BlockParam { block, .. } => schema::ValueDef::BlockParam {
                block: block as u64,
            },
            ir::ValueDef::Instr { op, args, classes } => schema::ValueDef::Instr {
                op: op.into(),
                args: args
                    .into_iter()
                    .map(|value| schema::ValueId(value.0))
                    .collect(),
                classes: classes.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl From<ir::KernelBody> for schema::Body {
    fn from(body: ir::KernelBody) -> Self {
        schema::Body {
            values: body.values.into_iter().map(Into::into).collect(),
            blocks: body.blocks.into_iter().map(Into::into).collect(),
            entry: body.entry as u64,
            exact_float_arithmetic: body.exact_float_arithmetic,
            barriers: body.barriers.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ir::KernelFragment> for schema::Fragment {
    fn from(fragment: ir::KernelFragment) -> Self {
        schema::Fragment {
            param_shape: fragment.param_shape.into(),
            roles: fragment.roles.into(),
            body: fragment.body.into(),
            inputs: fragment.inputs as u64,
            outputs: fragment.outputs as u64,
            input_classes: fragment.input_classes.into_iter().map(Into::into).collect(),
            output_classes: fragment
                .output_classes
                .into_iter()
                .map(Into::into)
                .collect(),
            result_classes: fragment
                .result_classes
                .into_iter()
                .map(Into::into)
                .collect(),
            target: fragment.target.into(),
            slices: fragment.slices.into_iter().map(Into::into).collect(),
            marks: fragment
                .marks
                .into_iter()
                .map(|mark| schema::Mark {
                    value: schema::ValueId(mark.value.0),
                    payload: mark.payload.into_iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}

impl From<&FusedKernel> for schema::Artifact {
    fn from(kernel: &FusedKernel) -> Self {
        schema::Artifact {
            version: schema::FORMAT_VERSION,
            fragment: kernel.fragment.clone().into(),
            claim: kernel.claim.clone().map(Into::into),
            plan: kernel.plan.clone().into(),
        }
    }
}

/// The IR a hand-edited text names, or the refusal naming what it met.
///
/// # Invariant
/// The parameter indices are re-derived from the blocks' own `params` rather than
/// read from the text: an index the text stated separately could disagree with
/// the list that binds it, and the two are one fact.
pub(crate) fn fragment(text: schema::Fragment) -> Result<ir::KernelFragment, FusedRefusal> {
    let table = Table {
        values: text.body.values.len(),
    };
    let blocks = text
        .body
        .blocks
        .iter()
        .enumerate()
        .map(|(ordinal, block)| {
            let at = format!("fragment.body.blocks[{ordinal}]");
            basic_block(&at, block, &table)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let values = text
        .body
        .values
        .iter()
        .enumerate()
        .map(|(ordinal, entry)| {
            let at = format!("fragment.body.values[{ordinal}]");
            let definition = match entry {
                schema::ValueDef::BlockParam { block } => {
                    let block = wide(&format!("{at}.block_param.block"), *block)?;
                    let index = blocks
                        .get(block)
                        .and_then(|basic| {
                            basic
                                .params
                                .iter()
                                .position(|value| value.0 as usize == ordinal)
                        })
                        .ok_or_else(|| {
                            NotAnIrValue::at(
                                &format!("{at}.block_param"),
                                format!(
                                    "block {block}, which does not list value {ordinal} among its \
                                     params"
                                ),
                            )
                        })?;
                    ir::ValueDef::BlockParam {
                        block,
                        index: u32::try_from(index).map_err(|_| {
                            NotAnIrValue::at(
                                &format!("{at}.block_param"),
                                format!(
                                    "block {block}, which receives {index} parameter(s), more than \
                                     a block may declare"
                                ),
                            )
                        })?,
                    }
                }
                schema::ValueDef::Instr { op, args, classes } => ir::ValueDef::Instr {
                    op: instruction(&format!("{at}.instr.op"), op)?,
                    args: table.ids(&format!("{at}.instr.args"), args)?,
                    classes: classes.iter().copied().map(Into::into).collect(),
                },
            };
            Ok(definition)
        })
        .collect::<Result<Vec<_>, NotAnIrValue>>()?;
    let barriers = text
        .body
        .barriers
        .iter()
        .enumerate()
        .map(|(ordinal, barrier)| {
            let at = format!("fragment.body.barriers[{ordinal}]");
            Ok(ir::Barrier {
                write: table.id(&format!("{at}.write"), barrier.write)?,
                read: table.id(&format!("{at}.read"), barrier.read)?,
            })
        })
        .collect::<Result<Vec<_>, NotAnIrValue>>()?;
    let marks = text
        .marks
        .iter()
        .enumerate()
        .map(|(ordinal, mark)| {
            let at = format!("fragment.marks[{ordinal}]");
            Ok(ir::KernelMark {
                value: table.id(&format!("{at}.value"), mark.value)?,
                payload: mark.payload.iter().cloned().map(Into::into).collect(),
            })
        })
        .collect::<Result<Vec<_>, NotAnIrValue>>()?;
    Ok(ir::KernelFragment {
        param_shape: text.param_shape.into(),
        roles: text.roles.into(),
        body: ir::KernelBody {
            values,
            blocks,
            entry: wide("fragment.body.entry", text.body.entry)?,
            exact_float_arithmetic: text.body.exact_float_arithmetic,
            barriers,
        },
        inputs: wide("fragment.inputs", text.inputs)?,
        outputs: wide("fragment.outputs", text.outputs)?,
        input_classes: text.input_classes.into_iter().map(Into::into).collect(),
        output_classes: text.output_classes.into_iter().map(Into::into).collect(),
        result_classes: text.result_classes.into_iter().map(Into::into).collect(),
        target: target("fragment.target", text.target)?,
        slices: text.slices.into_iter().map(Into::into).collect(),
        marks,
    })
}

/// One block's IR form.
fn basic_block(
    at: &str,
    block: &schema::BasicBlock,
    table: &Table,
) -> Result<ir::BasicBlock, NotAnIrValue> {
    Ok(ir::BasicBlock {
        params: table.ids(&format!("{at}.params"), &block.params)?,
        instrs: table.ids(&format!("{at}.instrs"), &block.instrs)?,
        terminator: terminator(&format!("{at}.terminator"), &block.terminator, table)?,
    })
}

/// One terminator's IR form.
fn terminator(
    at: &str,
    terminator: &schema::Terminator,
    table: &Table,
) -> Result<ir::Terminator, NotAnIrValue> {
    Ok(match terminator {
        schema::Terminator::Return { values } => ir::Terminator::Return {
            values: table.ids(&format!("{at}.return.values"), values)?,
        },
        schema::Terminator::Br(branch) => {
            ir::Terminator::Br(branch_at(&format!("{at}.br"), branch, table)?)
        }
        schema::Terminator::CondBr {
            cond,
            if_true,
            if_false,
        } => ir::Terminator::CondBr {
            cond: table.id(&format!("{at}.cond_br.cond"), *cond)?,
            if_true: branch_at(&format!("{at}.cond_br.if_true"), if_true, table)?,
            if_false: branch_at(&format!("{at}.cond_br.if_false"), if_false, table)?,
        },
    })
}

/// One branch's IR form.
fn branch_at(at: &str, branch: &schema::Br, table: &Table) -> Result<ir::Br, NotAnIrValue> {
    Ok(ir::Br {
        target: wide(&format!("{at}.target"), branch.target)?,
        args: table.ids(&format!("{at}.args"), &branch.args)?,
    })
}

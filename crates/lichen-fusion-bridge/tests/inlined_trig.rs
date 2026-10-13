//! The `sin`/`cos` readers answer the operator, and only where the definition is.

use lichen_fusion::{Expr, UnaryOp};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelShape, LaneWidth, ScalarClass,
    TargetFacts, ValueDef,
};

/// A scalar fragment over `body`, whose entry leaf `0` is its datum.
fn fragment(body: KernelBody) -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Float),
        ]),
        body,
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Float],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
        roles: Default::default(),
    }
}

/// The expression the fragment's last computed value denotes.
fn read(fragment: &KernelFragment) -> Result<Expr, BridgeRefusal> {
    let marked = fragment.body.blocks[fragment.body.entry]
        .instrs
        .last()
        .copied()
        .expect("the body computes something");
    projection(fragment, marked, ReadRoles::parameters(0, 1))
}

/// `datum · 2`, the value the definition is spliced over.
///
/// # Invariant
/// Two leaves and one value, so the reader's answer is the definition's operand and
/// nothing else: a reader that answered some other value of the definition would answer a
/// different expression rather than the same one refused.
fn operand_of() -> KernelBody {
    KernelBody::from_flat(
        2,
        &[
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::Const(
                ScalarClass::Float,
                f32::to_bits(2.0) as i64,
            )),
            FlatOp::Instr(KernelInstr::Bin(
                ScalarClass::Float,
                lichen_kernel_ir::KernelBin::Mul,
            )),
        ],
    )
}

/// `definition` spliced over that operand, the way the language's lowering splices it.
///
/// # Invariant
/// The target is a **fresh** placeholder, as it is in `compute`: the inliner rewrites that
/// one value in place, so a target the operand already reads would leave the block's own
/// definition later than its reader.
fn inlined(definition: KernelBody) -> KernelBody {
    let mut body = operand_of();
    let operand = body.blocks[body.entry].instrs.last().copied().unwrap();
    let target = body.add_const(body.entry, ScalarClass::Float, f32::to_bits(0.0) as i64);
    let position = body.blocks[body.entry].instrs.len() - 1;
    body.inline_definition(&definition, body.entry, position, target, operand)
        .expect("the definition inlines");
    body
}

/// The sine's definition with its head constant moved: a different computation.
fn moved() -> KernelBody {
    let mut definition = lichen_kernel_ir::sin_body();
    let entry = definition.entry;
    let head = f32::to_bits(1.5708008) as i64;
    let operand = definition.blocks[entry]
        .instrs
        .iter()
        .filter_map(|step| match &definition.values[step.0 as usize] {
            lichen_kernel_ir::ValueDef::Instr {
                op: KernelInstr::Bin(_, _),
                args,
                ..
            } => args.get(1).copied(),
            _ => None,
        })
        .find(|arg| {
            matches!(
                definition.values[arg.0 as usize],
                lichen_kernel_ir::ValueDef::Instr {
                    op: KernelInstr::Const(ScalarClass::Float, bits),
                    ..
                } if bits == head
            )
        })
        .expect("the definition multiplies by its head");
    definition.values[operand.0 as usize] = ValueDef::Instr {
        op: KernelInstr::Const(ScalarClass::Float, f32::to_bits(1.25) as i64),
        args: Vec::new(),
        classes: vec![ScalarClass::Float],
    };
    definition
}

/// Both trigonometric definitions bridge, and a moved constant does not.
///
/// # Invariant
/// **The check a wrong recognition cannot pass.** The reader walks the definition
/// backwards from its own result, so the inlined `sin`/`cos` answer the operator and the
/// moved constant — a different computation in the same shape — is refused by the
/// `Select` it really holds.
#[test]
fn sin_and_cos_are_read_as_themselves_only_where_the_definition_is() {
    let datum = Expr::Binary(
        lichen_fusion::BinaryOp::Mul,
        Box::new(Expr::Variable(lichen_fusion::Variable::DATUM)),
        Box::new(Expr::Constant(2.0)),
    );
    assert_eq!(
        read(&fragment(inlined(lichen_kernel_ir::sin_body()))),
        Ok(UnaryOp::Sin.apply(datum.clone())),
        "the whole inlined definition is read as one `sin`, not walked into"
    );
    assert_eq!(
        read(&fragment(inlined(lichen_kernel_ir::cos_body()))),
        Ok(UnaryOp::Cos.apply(datum)),
        "the whole inlined definition is read as one `cos`, not walked into"
    );
    assert_eq!(
        read(&fragment(inlined(moved()))).expect_err("a moved constant is not the definition"),
        BridgeRefusal::Select,
        "a body that computes something else is refused by the instruction it holds"
    );
}

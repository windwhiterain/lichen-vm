//! A projection may read `the invocation`: the launch's extent and the invocation's index.

use lichen_fusion::{Expr, Variable};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBody, KernelFragment, KernelInstr, KernelShape, LaneWidth, ScalarClass,
    TargetFacts, ValueId,
};

/// A parallel fragment with `runtime` runtime scalars beside the extent, marked at `leaf`.
///
/// # Invariant
/// `param_shape` is `[Float, …runtime…, Int]`, which is what `runtime_scalars` counts and
/// `scalar_leaves` stops short of: leaf `0` is the extent, leaf `1 + runtime` is the index, and
/// the leaves between are the runtime scalars a launch fixes beside the extent.
fn parallel_over(runtime: usize, leaf: usize) -> KernelFragment {
    let mut leaves = vec![KernelShape::Scalar(ScalarClass::Float)];
    leaves.extend((0..runtime).map(|_| KernelShape::Scalar(ScalarClass::Float)));
    leaves.push(KernelShape::Scalar(ScalarClass::Int));
    let params = leaves.len();
    let mut ops = vec![FlatOp::Read(leaf)];
    ops.push(FlatOp::Instr(KernelInstr::Conv {
        from: ScalarClass::Int,
        to: ScalarClass::Float,
    }));
    fragment(params, &ops, KernelShape::Tuple(leaves))
}

/// The fragment `params` entry leaves and `ops`, over `param_shape`.
fn fragment(params: usize, ops: &[FlatOp], param_shape: KernelShape) -> KernelFragment {
    KernelFragment {
        param_shape,
        body: KernelBody::from_flat(params, ops),
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

/// The expression the fragment's last computed value denotes, over roles of no producer.
fn read(fragment: &KernelFragment) -> Result<Expr, BridgeRefusal> {
    let marked = fragment.body.blocks[fragment.body.entry]
        .instrs
        .last()
        .copied()
        .expect("the body computes something");
    projection(fragment, marked, ReadRoles::DATUM_ONLY)
}

/// The launch's **extent** is a slot, and a projection may read it.
///
/// # Invariant
/// **It is the ABI's first leaf**, which [`KernelFragment::runtime_scalars`] states, so the
/// element of a normalisation reading `Σx² / n` bridges instead of refusing — and it is one
/// value for the whole launch, so the projection stays one function of the fused step.
#[test]
fn the_launch_extent_is_a_slot() {
    let fragment = parallel_over(0, 0);
    let expression = read(&fragment).expect("the extent is `the invocation`");
    assert_eq!(expression, Expr::Variable(Variable::EXTENT));
}

/// The invocation's **index** is a slot too, and it is the domain's **last** leaf.
///
/// # Invariant
/// **Not the extent**: the two are read at leaves `0` and `scalar_leaves`, so a position-dependent
/// element — a relative-position term, a mask — is writable without a new role.
#[test]
fn the_invocation_index_is_a_slot_at_the_last_leaf() {
    let fragment = parallel_over(0, 1);
    let expression = read(&fragment).expect("the invocation index is `the invocation`");
    assert_eq!(expression, Expr::Variable(Variable::INVOCATION));
}

/// A **runtime scalar** is neither half, and is refused by name.
///
/// # Invariant
/// Leaf `0` is the extent and leaf `2` the index of a two-scalar launch, so leaf `1` — the value
/// the launch fixes *beside* the extent, `eps` — is the one leaf no slot names.
#[test]
fn a_runtime_scalar_beside_the_extent_is_refused_by_name() {
    let fragment = parallel_over(1, 1);
    let refusal = read(&fragment).expect_err("no slot names a runtime scalar");
    assert_eq!(refusal, BridgeRefusal::ReadsARuntimeScalar { index: 1 });
    let message = refusal.to_string();
    assert!(
        message.contains("runtime scalar the launch fixes beside its extent"),
        "names what the leaf is: {message}"
    );
    assert!(
        message.contains("one function of the fused step's slots and the invocation"),
        "and restates the licence the refusal enforces: {message}"
    );
}

/// A projection that reads a **buffer** is still refused, and by its own name.
///
/// # Invariant
/// **The widening is about the domain only**: a scalar fragment's parameters *are* its slots, so
/// a marked value that reads storage is still refused rather than read as a term.
#[test]
fn a_buffer_read_is_still_refused_by_name() {
    let fragment = fragment(
        2,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(1),
            FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Float)),
        ],
        KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
    );
    let marked: ValueId = fragment.body.blocks[fragment.body.entry]
        .instrs
        .last()
        .copied()
        .expect("the body reads something");
    let refusal = projection(&fragment, marked, ReadRoles::parameters(0, 1))
        .expect_err("a scalar fragment's projection carries no buffer");
    assert_eq!(refusal, BridgeRefusal::ReadsABuffer);
    let message = refusal.to_string();
    assert!(
        message.contains("a scalar projection has no buffers"),
        "names the cause rather than the domain: {message}"
    );
}

//! A projection may read `the invocation`: the launch's extent and the invocation's index.

use lichen_fusion::{Expr, RUNTIME_SCALAR_INPUTS, Variable};
use lichen_fusion_bridge::{BridgeRefusal, ReadRoles, projection};
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelShape, LaneHeight,
    LaneWidth, ScalarClass, TargetFacts, ValueId,
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
            lane_height: LaneHeight::new(1),
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

/// A **runtime scalar** is a third kind of free value, one slot per ordinal beside the extent.
///
/// # Invariant
/// **It is one value for the whole launch**, so a projection that reads it is still one function
/// per lane and needs no role of its own — the licence the extent already had, widened to the
/// leaves between the extent and the index. Leaf `1` is [`Variable::RUNTIME_SCALAR`] and leaf `2`
/// the slot after it, so the ordinal is the leaf's own place in the domain.
#[test]
fn a_runtime_scalar_beside_the_extent_is_a_slot_of_its_own() {
    let first = parallel_over(1, 1);
    let expression = read(&first).expect("a runtime scalar is a slot");
    assert_eq!(expression, Expr::Variable(Variable::RUNTIME_SCALAR));
    let second = parallel_over(2, 2);
    let expression = read(&second).expect("each runtime scalar is a slot of its own");
    assert_eq!(
        expression,
        Expr::Variable(Variable::runtime_scalar(1).expect("the table holds the second")),
        "the second runtime scalar is a different slot, not the first re-read"
    );
}

/// A runtime scalar past the number the symbolic language names is refused by name.
///
/// # Invariant
/// **The bound is a capability rather than a licence**: a fragment reading more runtime scalars
/// than the table holds is refused rather than admitted with two of them read as one.
#[test]
fn a_runtime_scalar_past_the_named_table_is_refused_by_name() {
    let past = RUNTIME_SCALAR_INPUTS as u32 + 1;
    let fragment = parallel_over(past as usize + 1, past as usize);
    let refusal = read(&fragment).expect_err("the table does not reach this leaf");
    assert_eq!(refusal, BridgeRefusal::ReadsARuntimeScalar { index: past });
    let message = refusal.to_string();
    assert!(
        message.contains("runtime scalar the launch fixes beside its extent"),
        "names what the leaf is: {message}"
    );
    assert!(
        message.contains(&format!(
            "past the {RUNTIME_SCALAR_INPUTS} the symbolic language names"
        )),
        "and why it has no slot: {message}"
    );
}

/// A **scalar** fragment's own leaf that the roles name no producer's value at is still refused.
///
/// # Invariant
/// **The widening is about a parallel domain only.** A scalar fragment's parameters *are* its
/// slots, so leaf `2` of a three-leaf scalar domain names no value at all and reading it is not a
/// third kind of free value — it is none.
#[test]
fn an_unnamed_leaf_of_a_scalar_fragment_is_still_refused_by_name() {
    let fragment = fragment(
        3,
        &[
            FlatOp::Instr(KernelInstr::Const(
                ScalarClass::Float,
                1.0f32.to_bits() as i64,
            )),
            FlatOp::Read(2),
            FlatOp::Instr(KernelInstr::Bin(ScalarClass::Float, KernelBin::Mul)),
        ],
        KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Float),
            KernelShape::Scalar(ScalarClass::Float),
        ]),
    );
    let marked = fragment.body.blocks[fragment.body.entry]
        .instrs
        .last()
        .copied()
        .expect("the body computes something");
    let refusal = projection(&fragment, marked, ReadRoles::parameters(0, 1))
        .expect_err("no slot names a scalar fragment's third leaf");
    assert_eq!(refusal, BridgeRefusal::ReadsAnUnnamedLeaf { index: 2 });
    let message = refusal.to_string();
    assert!(
        message.contains("a scalar kernel's own domain"),
        "names which domain it was: {message}"
    );
    assert!(
        message
            .contains("a projection is one function of the fused step's slots and the invocation"),
        "and restates the licence a scalar projection still answers to: {message}"
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

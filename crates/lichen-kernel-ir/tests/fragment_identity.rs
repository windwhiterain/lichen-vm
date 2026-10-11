//! The crate's stated invariants — the digest, both gates, and the barrier scan — through its public
//! surface.

use lichen_kernel_ir::{
    Barrier, Br, FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles,
    KernelShape, LaneWidth, ScalarClass, Slice, Space, TargetFacts, Terminator, ValueDef, ValueId,
    from_flat,
};

/// A one-parameter body: `p0 + first`, in SSA.
///
/// # Invariant
/// Built the way a lowering builds one — `add_param`, `add_op` in dependency order,
/// `set_terminator` last — because the digest is over the shape a consumer reads.
fn body(first: i64, combine: bool) -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let parameter = body.add_param(entry);
    let literal = body.add_op(
        entry,
        KernelInstr::Const(ScalarClass::Int, first),
        Vec::new(),
        vec![ScalarClass::Int],
    );
    let result = if combine {
        body.add_op(
            entry,
            KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
            vec![literal, parameter],
            vec![ScalarClass::Int],
        )
    } else {
        literal
    };
    body.set_terminator(
        entry,
        lichen_kernel_ir::Terminator::Return {
            values: vec![result],
        },
    );
    body
}

fn fragment() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Scalar(ScalarClass::Int),
        body: body(1, true),
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: Vec::new(),
        marks: Vec::new(),
    }
}

#[test]
fn the_digest_separates_fragments_that_differ_in_any_field_it_hashes() {
    let base = fragment();
    let base_digest = lichen_kernel_ir::fragment_digest(&base);

    // One mutation per field the digest hashes.

    // `int_width` is absent and that is a real gap: `IntWidth` has one variant, so the
    // case cannot be constructed.

    // A leaf's class rides on `param_shape`, which is hashed whole; each buffer-class
    // list is its own hashed field.
    let mut variants: Vec<(&str, KernelFragment)> = vec![
        (
            "param_shape",
            KernelFragment {
                roles: KernelRoles::default(),
                param_shape: KernelShape::Tuple(vec![
                    KernelShape::Scalar(ScalarClass::Int),
                    KernelShape::Scalar(ScalarClass::Int),
                ]),
                ..base.clone()
            },
        ),
        (
            "body",
            KernelFragment {
                roles: KernelRoles::default(),
                body: body(2, true),
                ..base.clone()
            },
        ),
        (
            "inputs",
            KernelFragment {
                roles: KernelRoles::default(),
                inputs: 1,
                ..base.clone()
            },
        ),
        (
            "outputs",
            KernelFragment {
                roles: KernelRoles::default(),
                outputs: 1,
                ..base.clone()
            },
        ),
        (
            "input_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                input_classes: vec![ScalarClass::Int],
                ..base.clone()
            },
        ),
        (
            "output_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                output_classes: vec![ScalarClass::Float],
                ..base.clone()
            },
        ),
        (
            "result_classes",
            KernelFragment {
                roles: KernelRoles::default(),
                result_classes: vec![ScalarClass::Int, ScalarClass::Int],
                ..base.clone()
            },
        ),
        (
            "slices",
            KernelFragment {
                roles: KernelRoles::default(),
                slices: vec![Slice {
                    class: ScalarClass::Int,
                    count: 64,
                    space: Space::Shared,
                }],
                ..base.clone()
            },
        ),
        (
            "lane width",
            KernelFragment {
                target: TargetFacts {
                    lane_width: LaneWidth::new(32),
                    ..base.target
                },
                ..base.clone()
            },
        ),
    ];
    variants.push((
        "body instruction kind",
        KernelFragment {
            roles: KernelRoles::default(),
            body: body(1, false),
            ..base.clone()
        },
    ));

    for (field, variant) in variants {
        assert_ne!(
            lichen_kernel_ir::fragment_digest(&variant),
            base_digest,
            "a fragment differing in {field} must not intern to the same id, or the module \
             cache serves one kernel's compiled form for another's"
        );
    }
}

#[test]
fn the_digest_is_a_function_of_the_fragment_alone() {
    // Content addressing: one fragment must digest the same every time and every build,
    // or the module cache never hits.
    assert_eq!(
        lichen_kernel_ir::fragment_digest(&fragment()),
        lichen_kernel_ir::fragment_digest(&fragment())
    );
}

#[test]
fn a_domain_flattens_to_exactly_its_leaf_count() {
    // The parameter list is built from this count: a different flattening would pass
    // another kernel's arguments.
    assert_eq!(KernelShape::Scalar(ScalarClass::Int).flat_arity(), 1);
    assert_eq!(KernelShape::Scalar(ScalarClass::Float).flat_arity(), 1);
    assert_eq!(KernelShape::Tuple(vec![]).flat_arity(), 0);
    assert_eq!(
        KernelShape::Tuple(vec![KernelShape::Scalar(ScalarClass::Int); 3]).flat_arity(),
        3
    );
    // A nested domain, which a `jit` of a `((Int, Int), Int)` produces.
    assert_eq!(
        KernelShape::Tuple(vec![
            KernelShape::Tuple(vec![
                KernelShape::Scalar(ScalarClass::Int),
                KernelShape::Scalar(ScalarClass::Int),
            ]),
            KernelShape::Scalar(ScalarClass::Int),
        ])
        .flat_arity(),
        3
    );
}

/// A body whose slice read sits either in a branch's arm or in the block it joins.
///
/// # Invariant
/// The arm is reached only when the condition holds, so a barrier before a read there is
/// divergent by construction; the join is on every path to the return, so a barrier there is
/// uniform.
fn branched_slice_read(in_the_arm: bool) -> (KernelBody, Barrier) {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _config = body.add_param(entry);
    let index = body.add_param(entry);
    let slot = body.add_const(entry, ScalarClass::Int, 0);
    let value = body.add_const(entry, ScalarClass::Int, 7);
    let version = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![slot, index, value],
        vec![ScalarClass::Int],
    );
    let arm = body.add_block();
    let join = body.add_block();
    let taken = body.add_const(entry, ScalarClass::Int, 1);
    body.set_terminator(
        entry,
        Terminator::CondBr {
            cond: taken,
            if_true: Br {
                target: arm,
                args: Vec::new(),
            },
            if_false: Br {
                target: join,
                args: Vec::new(),
            },
        },
    );
    let read = body.add_op(
        if in_the_arm { arm } else { join },
        KernelInstr::SliceReadCall(ScalarClass::Int),
        vec![slot, index, version],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        arm,
        Terminator::Br(Br {
            target: join,
            args: Vec::new(),
        }),
    );
    let returned = body.add_const(join, ScalarClass::Int, 0);
    body.set_terminator(
        join,
        Terminator::Return {
            values: vec![returned],
        },
    );
    (
        body,
        Barrier {
            write: version,
            read,
        },
    )
}

/// A barrier whose read a lane can skip is invalid rather than imprecise, so the gate refuses it.
#[test]
fn a_barrier_whose_read_sits_under_a_branch_is_refused() {
    let (mut body, pair) = branched_slice_read(true);
    body.barriers.push(pair);
    let refusal = body
        .validate()
        .expect_err("a barrier before a read a lane can skip is refused");
    assert!(refusal.contains("divergent"), "names the cause: {refusal}");
    assert!(refusal.contains("block 1"), "names the block: {refusal}");
}

/// What the check does catch, from the other side: a read every path reaches needs no refusal.
#[test]
fn a_barrier_whose_read_every_lane_reaches_is_accepted() {
    let (mut body, pair) = branched_slice_read(false);
    body.barriers.push(pair);
    body.validate()
        .expect("a read on every path to the return carries a uniform barrier");
}

/// The `ValueId`s of a body's slice accesses of one kind, in the order they are defined.
///
/// # Invariant
/// A `from_flat` body numbers its values in the order its steps define them, so this names the
/// access a test wrote there without counting the ids it happens to take.
fn slice_accesses(body: &KernelBody, write: bool) -> Vec<ValueId> {
    body.values
        .iter()
        .enumerate()
        .filter_map(|(at, def)| {
            let ValueDef::Instr { op, .. } = def else {
                return None;
            };
            let addressed = match op {
                KernelInstr::SliceWriteCall(_) => write,
                KernelInstr::SliceReadCall(_) => !write,
                _ => return None,
            };
            addressed.then_some(ValueId(at as u32))
        })
        .collect()
}

/// A write and a read at two different indices may meet across lanes, so they are separated.
#[test]
fn a_slice_write_and_a_read_by_another_index_are_a_pair() {
    let body = from_flat(
        1,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
            FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)),
            FlatOp::Instr(KernelInstr::SliceWriteCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::SliceReadCall(ScalarClass::Int)),
        ],
    );
    let lane = body.parameters()[0];
    let pairs = body
        .barrier_scan(lane, &[])
        .expect("a lane-dependent write and a read at another index are well formed");
    assert_eq!(
        pairs,
        vec![Barrier {
            write: slice_accesses(&body, true)[0],
            read: slice_accesses(&body, false)[0],
        }]
    );
}

/// A lane reads the address it wrote itself, in program order, so nothing is owed.
#[test]
fn a_slice_write_and_a_read_by_the_same_index_are_no_pair() {
    let body = from_flat(
        1,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Read(0),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)),
            FlatOp::Instr(KernelInstr::SliceWriteCall(ScalarClass::Int)),
            FlatOp::Instr(KernelInstr::SliceReadCall(ScalarClass::Int)),
        ],
    );
    let lane = body.parameters()[0];
    assert!(
        body.barrier_scan(lane, &[])
            .expect("a lane-dependent write and the same read index are well formed")
            .is_empty()
    );
}

/// A slice write every lane makes to one element is contended, and no barrier can order it.
#[test]
fn a_slice_write_whose_index_excludes_the_lane_is_refused() {
    let body = from_flat(
        1,
        &[
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 7)),
            FlatOp::Instr(KernelInstr::SliceWriteCall(ScalarClass::Int)),
        ],
    );
    let lane = body.parameters()[0];
    let refusal = body
        .barrier_scan(lane, &[])
        .expect_err("every lane writes one slot, so the write is refused");
    assert!(refusal.contains("contended"), "names the cause: {refusal}");
    assert!(
        refusal.contains("lane parameter"),
        "names what it excludes: {refusal}"
    );
}

/// A fragment staging one `Int` slice of `count` elements, over the body a test supplies.
fn staging(count: u32, body: KernelBody) -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 0,
        outputs: 0,
        input_classes: Vec::new(),
        output_classes: Vec::new(),
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(64),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count,
            space: Space::Shared,
        }],
        marks: Vec::new(),
    }
}

/// A slice of zero elements has no type to declare, refused at the fragment's own gate.
#[test]
fn a_fragment_staging_a_zero_length_slice_is_refused() {
    let refusal = staging(0, body(1, true))
        .validate()
        .expect_err("a slice with no elements cannot stage anything");
    assert!(
        refusal.contains("no element(s)"),
        "names the cause: {refusal}"
    );
    assert!(refusal.contains('0'), "names the slice: {refusal}");
}

/// An index the interval machinery cannot bound inside the slice is a GPU fault, refused by name.
#[test]
fn a_slice_index_that_may_address_past_its_slice_is_refused() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let slot = body.add_const(entry, ScalarClass::Int, 0);
    let value = body.add_const(entry, ScalarClass::Int, 7);
    let written = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![slot, index, value],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![written],
        },
    );
    let lane = body.parameters()[1];
    let refusal = staging(64, body)
        .barrier_scan(lane)
        .expect_err("the lane's own value reaches past a slice of 64");
    assert!(
        refusal.contains("may address past its slice"),
        "names the cause: {refusal}"
    );
    assert!(
        refusal.contains("64 element(s)"),
        "names the length it leaves: {refusal}"
    );
}

/// `i % 64` against a slice of 64 is in bounds, so the pair it stages is still recorded.
#[test]
fn a_slice_index_bounded_by_the_slice_is_accepted() {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let sixty_four = body.add_const(entry, ScalarClass::Int, 64);
    let one = body.add_const(entry, ScalarClass::Int, 1);
    let write_slot = body.add_const(entry, ScalarClass::Int, 0);
    let offset = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![index, sixty_four],
        vec![ScalarClass::Int],
    );
    let written = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![write_slot, offset, one],
        vec![ScalarClass::Int],
    );
    let read_slot = body.add_const(entry, ScalarClass::Int, 0);
    let next = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Add),
        vec![index, one],
        vec![ScalarClass::Int],
    );
    let next_offset = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Int, KernelBin::Rem),
        vec![next, sixty_four],
        vec![ScalarClass::Int],
    );
    let read = body.add_op(
        entry,
        KernelInstr::SliceReadCall(ScalarClass::Int),
        vec![read_slot, next_offset, written],
        vec![ScalarClass::Int],
    );
    body.set_terminator(entry, Terminator::Return { values: vec![read] });
    let lane = body.parameters()[1];
    let pairs = staging(64, body)
        .barrier_scan(lane)
        .expect("a slice of 64 holds `i % 64`");
    assert_eq!(
        pairs,
        vec![Barrier {
            write: written,
            read
        }]
    );
}

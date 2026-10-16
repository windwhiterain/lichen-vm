//! A plan is checked against the fragment, never believed
//! (`docs/notes/graph-fusion.md` §5).

use lichen_kernel_ir::{
    IntWidth, KernelBody, KernelFragment, KernelInstr, KernelMark, KernelRoles, KernelShape,
    LaneHeight, LaneWidth, MarkField, Plan, PlanRefusal, ScalarClass, Slice, Space, Staging,
    TargetFacts, Terminator, ValueId,
};

/// The lane count one workgroup runs.
const LANES: u32 = 64;

/// The mark the staged value carries.
const TILE: &str = "tile";

/// A body that stages `7` into the slice at its own index.
///
/// # Invariant
/// The mark rides the **slice write's result** — the version — because that value's producer is
/// the staging itself, which is the value a plan names.
fn staged_body() -> (KernelBody, ValueId, ValueId) {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let _extent = body.add_param(entry);
    let index = body.add_param(entry);
    let ordinal = body.add_const(entry, ScalarClass::Int, 0);
    let written = body.add_const(entry, ScalarClass::Int, 7);
    let staged = body.add_op(
        entry,
        KernelInstr::SliceWriteCall(ScalarClass::Int),
        vec![ordinal, index, written],
        vec![ScalarClass::Int],
    );
    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![staged],
        },
    );
    (body, staged, written)
}

/// The fragment that body is dispatched as, with one slice in `space` and those marks.
fn fragment(space: Space, marks: &[(&str, ValueId)]) -> KernelFragment {
    let (body, _, _) = staged_body();
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body,
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
        target: TargetFacts {
            int_width: IntWidth::I64,
            lane_width: LaneWidth::new(LANES),
            lane_height: LaneHeight::new(1),
        },
        slices: vec![Slice {
            class: ScalarClass::Int,
            count: LANES,
            space,
        }],
        marks: marks
            .iter()
            .map(|(name, value)| KernelMark {
                value: *value,
                payload: vec![MarkField {
                    name: None,
                    text: name.to_string(),
                }],
            })
            .collect(),
    }
}

/// The plan that stages the marked value in `space` at `group_width`.
fn plan(space: Space, group_width: u32) -> Plan {
    Plan {
        staging: vec![Staging {
            name: TILE.to_string(),
            space,
            group_width,
        }],
    }
}

#[test]
fn a_plan_the_fragment_honours_is_accepted() {
    let (_, staged, _) = staged_body();
    let fragment = fragment(Space::Shared, &[(TILE, staged)]);
    assert_eq!(
        fragment.marked_value(TILE),
        Ok(staged),
        "the name resolves to the value the body marked"
    );
    fragment
        .check_plan(&plan(Space::Shared, LANES))
        .expect("the fragment stages the marked value where the plan says");
    fragment
        .check_plan(&Plan::default())
        .expect("a plan that proposes nothing is refused by nothing");
}

#[test]
fn a_plan_naming_a_value_no_mark_published_is_refused() {
    let refusal = fragment(Space::Shared, &[])
        .check_plan(&plan(Space::Shared, LANES))
        .expect_err("a name no mark published names no value");
    assert!(
        matches!(refusal, PlanRefusal::UnknownStagedValue { ref name } if name == TILE),
        "the refusal carries the name: {refusal}"
    );
    assert!(
        refusal.message().contains(TILE),
        "the refusal names the entry: {refusal}"
    );
}

#[test]
fn a_name_two_values_carry_is_refused() {
    let (_, staged, written) = staged_body();
    let refusal = fragment(Space::Shared, &[(TILE, staged), (TILE, written)])
        .check_plan(&plan(Space::Shared, LANES))
        .expect_err("two values carrying one name is an ambiguity, not a value");
    assert!(
        matches!(refusal, PlanRefusal::AmbiguousStagedValue { ref name } if name == TILE),
        "the refusal carries the name: {refusal}"
    );
}

#[test]
fn a_plan_staging_a_value_the_body_did_not_stage_is_refused() {
    let (_, _, written) = staged_body();
    let refusal = fragment(Space::Shared, &[(TILE, written)])
        .check_plan(&plan(Space::Shared, LANES))
        .expect_err("a value no slice write produced stages nothing");
    assert!(
        matches!(refusal, PlanRefusal::NotAStagedValue { ref name } if name == TILE),
        "the refusal carries the name: {refusal}"
    );
}

#[test]
fn a_group_width_the_target_does_not_have_is_refused() {
    let (_, staged, _) = staged_body();
    let refusal = fragment(Space::Shared, &[(TILE, staged)])
        .check_plan(&plan(Space::Shared, LANES / 2))
        .expect_err("a group narrower than the workgroup is not a width this target has");
    assert!(
        matches!(
            refusal,
            PlanRefusal::StagingWidthNotAWidth { ref name, asked, lane_width }
                if name == TILE && asked == LANES / 2 && lane_width == LANES
        ),
        "the refusal carries the width it asked for and the width the target has: {refusal}"
    );
    assert!(
        refusal.message().contains(&(LANES / 2).to_string())
            && refusal.message().contains(&LANES.to_string()),
        "the refusal names both widths: {refusal}"
    );
}

#[test]
fn a_space_the_fragment_did_not_apply_is_refused() {
    let (_, staged, _) = staged_body();
    let refusal = fragment(Space::Shared, &[(TILE, staged)])
        .check_plan(&plan(Space::Register, LANES))
        .expect_err("a plan the fragment contradicts is not reported as applied");
    assert!(
        matches!(
            refusal,
            PlanRefusal::PlanContradicted { ref name, asked: Space::Register, found: Space::Shared }
                if name == TILE
        ),
        "the refusal carries both spaces: {refusal}"
    );
}

//! A declared staging plan, checked and driven on a device.
//! See `docs/notes/graph-fusion.md` §5.

use super::common;

use lichen_compute::fusion_claim::{claim_of, fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use super::common::LANES;
use lichen_fusion::{BinaryOp, Reduction};
use lichen_fusion_bridge::{Element, Folding};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBody, KernelFragment, KernelMark, KernelRoles, KernelShape,
    LaneHeight, LaneWidth, LaunchSet, MarkField, Plan, PlanRefusal, ScalarClass, ScalarData, Slice,
    Space, Staging, TargetFacts,
};
use lichen_lowlevel::Module;

/// The mark the logic publishes and the plan names.
const COMBINE: &str = "combine";


/// The length of every buffer the case drives.
///
/// # Invariant
/// The longest length the case drives, so a length past it is a bug here rather than padding the
/// device reads. The declared loop's bound is a literal, so the launch's own count selects the
/// answer through the **data**: lane `i` folds `count - i` values and the last lane's fold is
/// `count(count + 1) / 2`.
const BOUND: u64 = 256;

/// The data one launch over `count` elements folds: `1 ..= count`, then zeroes.
///
/// # Invariant
/// The closed form of `1 ..= count` is `count(count + 1) / 2`.
fn triangle_data(count: u64) -> Vec<i64> {
    let mut data: Vec<i64> = vec![0_i64; BOUND as usize];
    for (position, value) in data[..count as usize].iter_mut().enumerate() {
        *value = (position + 1) as i64;
    }
    data
}

/// The kernel a program dispatched, with the claim and fragment the registry holds.
///
/// # Invariant
/// The id is read from the node whose own value names a kernel — the node `compute.parallel`
/// compiled — and never from a value the host re-derives: a kernel *value* is a `Copy` id, so the
/// fragment the registry holds beside it is the only compiled body there is.
fn dispatched_kernel(module: &Module<LangProgram>) -> Option<usize> {
    module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
}

/// The claim and fragment of the kernel a program dispatched.
fn dispatched(module: &Module<LangProgram>) -> Result<(String, KernelFragment), String> {
    let id = dispatched_kernel(module).ok_or("the program dispatched no parallel kernel")?;
    let claim = claim_of(id).ok_or("the kernel claims no interface")?;
    let fragment = fragment_of(id).map_err(|refusal| refusal.message())?;
    Ok((claim.interface, fragment))
}

/// A compute context on a device carrying `shaderInt64`, or `None` when there is none here.
///

/// The packed bytes of `elements`, one `i64` each.
fn pack(elements: &[i64]) -> Vec<u8> {
    elements
        .iter()
        .flat_map(|element| element.to_le_bytes())
        .collect()
}

/// The `i64` elements of a fetched payload.
fn words(data: ScalarData) -> Vec<i64> {
    match data {
        ScalarData::Int(elements) => elements,
        ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
    }
}

/// The source whose reduction the case stages: one kernel, one claim, one loop.
///
/// # Invariant
/// The `@loop` is the reduction's own, and what it declares is the **algebra** — the combine and
/// the identity — because its bound is a literal.
fn source() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct{.b (compute.Buf Float)}
Out = struct{.z (compute.Buf Float)}
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) >= 7 then s(1) else fold (s(0) + 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.b, .at 0)) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (0, 0.0 ~ "identity")))
}
k = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
k
"#
}

/// The reduction the program's own kernel declares, and the name the logic published for it.
///
/// # Invariant
/// The program **is** a pack: it claims `claim:Reduction(combine,identity)`. Its loop's bound is a
/// literal, so the loop is compile-time expanded and the marked accumulation folds into its base
/// case's constant: `projection(…, ReadRoles::DATUM_ONLY)` answers `0` where the marks name `+`.
/// The name is the **logic's own published text**, never one this test invents.
fn declared_reduction_and_name() -> (Reduction, String) {
    let (module, _value, _root) = common::run(source());
    let (interface, fragment) = dispatched(&module)
        .unwrap_or_else(|refusal| panic!("the program dispatched one claimed kernel: {refusal}"));
    assert_eq!(
        interface, "Reduction",
        "the claim the pack wrote names the reduction interface"
    );
    let published: Vec<&str> = fragment.marks.iter().filter_map(KernelMark::text).collect();
    assert!(
        published.contains(&COMBINE),
        "the logic publishes the `combine` mark the plan names: {published:?}"
    );
    let name = published
        .into_iter()
        .find(|text| *text == COMBINE)
        .expect("the assertion above names the published text")
        .to_string();
    (
        Reduction {
            combine: BinaryOp::Add,
            identity: 0.0,
        },
        name,
    )
}

/// The plan that stages `name` in `space` at `group_width`.
fn plan(name: &str, space: Space, group_width: u32) -> Plan {
    Plan {
        staging: vec![Staging {
            name: name.to_string(),
            space,
            group_width,
        }],
    }
}

/// The fragment a tiled body is dispatched as, staging `plan`'s entry.
///
/// # Invariant
/// The space is the plan's and the length is the tiling's own count — two different quantities,
/// which is why they are read from two places and not one: reading a slice's count off a plan's
/// group width is right only where the case makes them coincide.
fn staged_fragment(
    body: KernelBody,
    plan: &Plan,
    count: u32,
    marks: Vec<KernelMark>,
) -> KernelFragment {
    let staging = plan
        .staging
        .first()
        .expect("the plan states one staging entry");
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
            count,
            space: staging.space,
        }],
        marks,
    }
}

/// One dispatch of a folded body, answering the elements it wrote.
///

/// A declared plan stages the reduction the logic marked, and the device answers the closed form.
///
/// # Invariant
/// The name is the logic's own published mark and the slice is the plan's own placement, so what
/// the check accepts is the plan this case declared rather than a fragment built past it. The
/// launch covers the elements the fold reads.
#[test]
fn a_declared_plan_stages_the_reduction() {
    let (_stub, _guard) = common::stub();
    let (reduction, staged_name) = declared_reduction_and_name();
    let lengths = [LANES as u64, (LANES * 3) as u64, (LANES * 4) as u64];
    for &count in &lengths {
        let data = triangle_data(count);
        let folding = Folding::new(
            &reduction,
            Element::Buffer { position: 0 },
            ScalarClass::Int,
        )
        .unwrap_or_else(|refusal| panic!("the declaration tiles: {refusal}"));
        let tiled = folding
            .tiled(LANES, 0)
            .unwrap_or_else(|refusal| panic!("{count} element(s) tile: {refusal}"));
        let declared = plan(&staged_name, Space::Shared, LANES);
        let fragment = staged_fragment(
            tiled.body().clone(),
            &declared,
            tiled.staged_count(),
            tiled.marks(&staged_name),
        );
        fragment
            .check_plan(&declared)
            .unwrap_or_else(|refusal| panic!("the fragment stages what the plan says: {refusal}"));
        assert_eq!(
            fragment.marked_value(&staged_name),
            Ok(tiled.staged()),
            "the plan's name resolves to the value the body staged"
        );
        fragment
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("the tiled body is well formed: {broken}"));
    }
}

/// A plan the fragment refuses is refused by name, and no case here needs a device.
///
/// # Invariant
/// Every refusal is decided on the fragment's own data — the published marks, the staged slice and
/// the target's width — so the check runs before any emit and a wrong plan never reaches a backend.
#[test]
fn a_plan_the_fragment_refutes_is_refused_by_name() {
    let (reduction, staged_name) = declared_reduction_and_name();
    let folding = Folding::new(
        &reduction,
        Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("the declared sum is associative and integer");
    let tiled = folding
        .tiled(LANES, 0)
        .expect("the declared sum tiles");
    let declared = plan(&staged_name, Space::Shared, LANES);

    // A group narrower than the workgroup is not a width this target has.
    let narrowed = plan(&staged_name, Space::Shared, LANES / 2);
    let fragment = staged_fragment(
        tiled.body().clone(),
        &declared,
        tiled.staged_count(),
        tiled.marks(&staged_name),
    );
    let refusal = fragment
        .check_plan(&narrowed)
        .expect_err("a group of half a workgroup is not a width this target has");
    assert!(
        matches!(
            refusal,
            PlanRefusal::StagingWidthNotAWidth { ref name, asked, lane_width }
                if name == &staged_name && asked == LANES / 2 && lane_width == LANES
        ),
        "the refusal carries the width it asked for and the width the target has: {refusal}"
    );
    assert!(
        refusal.message().contains(&staged_name)
            && refusal.message().contains(&(LANES / 2).to_string()),
        "the refusal names the entry and the width: {refusal}"
    );

    // A name no mark published names no value.
    let unknown = plan("unpublished", Space::Shared, LANES);
    let fragment = staged_fragment(
        tiled.body().clone(),
        &declared,
        tiled.staged_count(),
        tiled.marks(&staged_name),
    );
    let refusal = fragment
        .check_plan(&unknown)
        .expect_err("a name no mark published names no value");
    assert!(
        matches!(refusal, PlanRefusal::UnknownStagedValue { ref name } if name == "unpublished"),
        "the refusal carries the name: {refusal}"
    );
    assert!(
        refusal.message().contains("unpublished"),
        "the refusal names the entry: {refusal}"
    );

    // A slice the fragment placed in registers while the plan asks for shared memory.
    let registers = plan(&staged_name, Space::Register, LANES);
    let fragment = staged_fragment(
        tiled.body().clone(),
        &registers,
        tiled.staged_count(),
        tiled.marks(&staged_name),
    );
    let refusal = fragment
        .check_plan(&declared)
        .expect_err("a plan the fragment contradicts is not reported as applied");
    assert!(
        matches!(
            refusal,
            PlanRefusal::PlanContradicted { ref name, asked: Space::Shared, found: Space::Register }
                if name == &staged_name
        ),
        "the refusal carries both spaces: {refusal}"
    );

    // Two values carrying the plan's name is an ambiguity rather than a value.
    let mut duplicated = tiled.marks(&staged_name);
    duplicated.push(KernelMark {
        value: tiled.body().parameters()[1],
        payload: vec![MarkField {
            name: None,
            text: staged_name.clone(),
        }],
    });
    let fragment = staged_fragment(
        tiled.body().clone(),
        &declared,
        tiled.staged_count(),
        duplicated,
    );
    let refusal = fragment
        .check_plan(&declared)
        .expect_err("two values carrying one name is an ambiguity, not a value");
    assert!(
        matches!(refusal, PlanRefusal::AmbiguousStagedValue { ref name } if name == &staged_name),
        "the refusal carries the name: {refusal}"
    );
}

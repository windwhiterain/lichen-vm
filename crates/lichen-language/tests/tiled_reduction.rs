//! The acceptance case: a reduction folded across lanes. See
//! `docs/notes/shared-memory-and-barriers.md` §9 step 8.

mod common;

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_fusion::{BinaryOp, Reduction};
use lichen_fusion_bridge::{Element, Folding, TileRefusal};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBody, KernelFragment, KernelRoles, KernelShape, LaneWidth,
    LaunchSet, ScalarClass, ScalarData, Slice, Space, TargetFacts,
};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id};
use lichen_language::program::LangProgram;
use lichen_lowlevel::Module;

/// The combine a reduction's `combine` role declares.
const COMBINE: &str = "combine";

/// The lane width one workgroup runs.
const LANES: usize = LOCAL_SIZE_X as usize;

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
/// # Invariant
/// The skip is reported and never turned into a failure, the way every device-backed test in this
/// tree reports one.
fn device(test: &str) -> Option<GpuContext> {
    match GpuContext::new() {
        Ok(context) => Some(context),
        Err(reason) => {
            eprintln!("SKIPPED {test}: no device to run on: {reason}");
            None
        }
    }
}

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

/// The fragment a folded body is dispatched as.
///
/// # Invariant
/// A folded body's domain is `(extent, index)` so a launch fixes no runtime scalar, every leaf is
/// `Int` because that is what a body's addresses are, and the slice is the staging a tiled fold
/// writes, one element per lane in `Workgroup` storage
/// (`docs/notes/shared-memory-and-barriers.md` §5).
fn body_fragment(body: KernelBody, tiled: bool) -> KernelFragment {
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
            lane_width: LaneWidth::new(LOCAL_SIZE_X),
        },
        slices: if tiled {
            vec![Slice {
                class: ScalarClass::Int,
                count: LOCAL_SIZE_X,
                space: Space::Shared,
            }]
        } else {
            Vec::new()
        },
        marks: Vec::new(),
    }
}

/// One dispatch of a folded body, answering the elements it wrote.
///
/// # Invariant
/// The launch fixes the extent and the body folds it, so `extent` decides how many indices are
/// covered and — because a staged dispatch uploads `extent × width` bytes — how much of `data` the
/// device can read.
fn run_at(
    context: &GpuContext,
    fragment: &KernelFragment,
    data: &[i64],
    extent: usize,
    fetch: usize,
) -> Vec<i64> {
    let packed = pack(data);
    let launch = LaunchSet::single(fragment);
    let resident = context
        .run(&launch, &[BufferSlot::Host(&packed)], extent)
        .unwrap_or_else(|refusal| panic!("a folded body is dispatched: {refusal}"));
    let answer = words(
        context
            .fetch(resident[0], fetch)
            .expect("the folded answer comes off the device"),
    );
    context.release(resident[0]);
    answer
}

/// The source whose reduction the acceptance case folds: one kernel, one claim, one loop.
///
/// # Invariant
/// The `@loop` is the reduction's own, and what it declares is the **algebra** — the combine and
/// the identity — because its bound is a literal.
fn source() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct<.b (compute.Buf Float)>
Out = struct<.z (compute.Buf Float)>
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) >= 7 then s(1) else fold (s(0) + 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.b, .at 0))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (0, 0.0 ~ "identity")))
}
k = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu"
k
"#
}

/// The reduction the program's own kernel declares.
///
/// # Invariant
/// The program **is** a pack: it claims `claim:Reduction(combine,identity)`. Its loop's bound is a
/// literal, so the loop is compile-time expanded and the marked accumulation folds into its base
/// case's constant: `projection(…, ReadRoles::DATUM_ONLY)` answers `0` where the marks name `+`.
/// Reading a lowered body's algebra needs a loop that survives lowering (§9 step 8).
fn declared_reduction() -> Reduction {
    let (module, _value, _root) = common::run(source());
    let (interface, fragment) = dispatched(&module)
        .unwrap_or_else(|refusal| panic!("the program dispatched one claimed kernel: {refusal}"));
    assert_eq!(
        interface, "Reduction",
        "the claim the pack wrote names the reduction interface"
    );
    let roles: Vec<&str> = fragment
        .marks
        .iter()
        .filter_map(|mark| mark.role())
        .collect();
    assert!(
        roles.contains(&COMBINE),
        "the body marks the `combine` role the claim requires: {roles:?}"
    );
    Reduction {
        combine: BinaryOp::Add,
        identity: 0.0,
    }
}

/// A tiled reduction answers the closed form, pinned against the per-lane implementation, on a
/// real device.
///
/// # Invariant
/// One declaration produces both bodies, so the difference is the schedule and nothing else. Three
/// lengths are driven at `out[0]`, only one of them a single tile: one element per lane is the case
/// a schedule that never advances its address still passes. The host path refuses a staged body,
/// so the closed form and the per-lane run are this class's evidence.
#[test]
fn a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop() {
    let Some(context) = device("a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop")
    else {
        return;
    };
    let reduction = declared_reduction();
    assert_eq!(
        reduction.combine,
        BinaryOp::Add,
        "the program's own marks declare the sum its body folds"
    );
    // **One body per declaration**: the count is the launch's, so the two bodies are built once
    // and driven at each length.
    let folding = Folding::new(
        &reduction,
        Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .unwrap_or_else(|refusal| panic!("the declared sum tiles: {refusal}"));
    let tiled = folding
        .tiled(LOCAL_SIZE_X, 0)
        .unwrap_or_else(|refusal| panic!("the declared sum tiles at {LOCAL_SIZE_X}: {refusal}"));
    let tiled_fragment = body_fragment(tiled.into_body(), true);
    tiled_fragment
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("the tiled body is well formed: {broken}"));
    assert_eq!(
        tiled_fragment.body.barriers.len(),
        1,
        "the tiled body records the write-to-read pair its barrier separates"
    );
    let owed = tiled_fragment
        .barrier_scan(tiled_fragment.body.parameters()[1])
        .unwrap_or_else(|refusal| panic!("the tiled body's scan decides: {refusal}"));
    assert!(
        owed.len() <= 1,
        "the tiled body's scan owes at most the one barrier it records"
    );

    // The floor the tiling replaces, and the body it is differenced against.
    let per_lane_fragment = body_fragment(folding.per_lane(), false);
    per_lane_fragment
        .body
        .validate()
        .unwrap_or_else(|broken| panic!("the per-lane body is well formed: {broken}"));

    // **Three lengths, only one of them a single tile**, each a whole workgroup multiple.
    let lengths = [LANES as u64, (LANES * 3) as u64, (LANES * 4) as u64];
    for &count in &lengths {
        let data = triangle_data(count);
        let per_lane = run_at(
            &context,
            &per_lane_fragment,
            &data,
            count as usize,
            count as usize,
        );
        // **The launch covers the elements the fold reads**: `run` uploads `extent × width` bytes.
        let tiled_answer = run_at(
            &context,
            &tiled_fragment,
            &data,
            count as usize,
            count as usize,
        );
        let expected = (count * (count + 1) / 2) as i64;
        assert_eq!(
            per_lane[count as usize - 1],
            expected,
            "the per-lane loop reduces {count} element(s) to the closed form"
        );
        let total = tiled_answer[0];
        assert_eq!(
            total, expected,
            "the workgroup's {LANES} lane(s) fold {count} element(s) across a slice and a barrier \
             to the closed form"
        );
        assert_eq!(
            total,
            per_lane[count as usize - 1],
            "the tiled body and the per-lane loop answer one declaration alike"
        );
    }
}

/// The same tiling path folds a maximum and a product, because the licence is the declaration's.
///
/// # Invariant
/// Nothing in the tiling reads the operator beyond applying it: one `Folding::tiled` call produced
/// these bodies and the sum body above, and each operator is checked against a hand-derived closed
/// form. A **one-step tile and a three-step one are both driven**, because one element per lane is
/// the case a schedule that never advances its address still passes.
#[test]
fn a_second_combine_tiles_through_the_same_path() {
    let Some(context) = device("a_second_combine_tiles_through_the_same_path") else {
        return;
    };
    // **The data is bounded**: a power of two at an even index, one at an odd one.
    let data: Vec<i64> = (0..BOUND)
        .map(|position| if position % 2 == 0 { 2 } else { 1 })
        .collect();
    for steps in [1u64, 3] {
        let count = u64::from(LOCAL_SIZE_X) * steps;
        for declared_reduction in [
            Reduction {
                combine: BinaryOp::Max,
                identity: 0.0,
            },
            Reduction {
                combine: BinaryOp::Mul,
                identity: 1.0,
            },
        ] {
            let folding = Folding::new(
                &declared_reduction,
                Element::Buffer { position: 0 },
                ScalarClass::Int,
            )
            .unwrap_or_else(|refusal| {
                panic!(
                    "a `{}` over {count} element(s): {refusal}",
                    declared_reduction.combine.name()
                )
            });
            let tiled = folding.tiled(LOCAL_SIZE_X, 0).unwrap_or_else(|refusal| {
                panic!(
                    "a `{}` over {count} element(s) tiles: {refusal}",
                    declared_reduction.combine.name()
                )
            });
            let per_lane_fragment = body_fragment(folding.per_lane(), false);
            per_lane_fragment
                .body
                .validate()
                .unwrap_or_else(|broken| panic!("the per-lane body is well formed: {broken}"));
            let per_lane = run_at(
                &context,
                &per_lane_fragment,
                &data,
                count as usize,
                count as usize,
            );
            let closed = closed_form(declared_reduction.combine, &data, count);
            assert_eq!(
                per_lane[count as usize - 1],
                closed,
                "the per-lane body folds a `{}` to the same closed form",
                declared_reduction.combine.name()
            );
            let tiled_fragment = body_fragment(tiled.into_body(), true);
            // **The launch covers the elements the fold reads**, not the lanes it runs.
            let tiled_answer = run_at(
                &context,
                &tiled_fragment,
                &data,
                count as usize,
                count as usize,
            );
            assert_eq!(
                tiled_answer[0],
                closed,
                "the tiled `{}` over the {count} element(s)' tile is the hand-derived closed form",
                declared_reduction.combine.name()
            );
        }
    }
}

/// The fold of a tiled fold's iteration space under `combine`, derived by hand.
///
/// # Invariant
/// The oracle for the second combine: a tiled fold covers a **whole workgroup tile** —
/// `lane_width × ceil(count / lane_width)` elements — and a maximum over them is the largest while
/// a product is their running product, both computed here in a width that holds the running value
/// and read back at the fold's own class, which is what the device's arithmetic does.
fn closed_form(combine: BinaryOp, elements: &[i64], count: u64) -> i64 {
    let tiled = count.div_ceil(u64::from(LOCAL_SIZE_X)) * u64::from(LOCAL_SIZE_X);
    let covered = &elements[..tiled as usize];
    match combine {
        BinaryOp::Max => covered.iter().copied().fold(i64::MIN, i64::max),
        BinaryOp::Mul => covered
            .iter()
            .fold(1_i128, |left, right| left * i128::from(*right)) as i64,
        _ => panic!("this oracle states the operators the second leg folds"),
    }
}

/// A declared combine that is not associative is refused by name, beside the positive cases.
///
/// # Invariant
/// The refusal is the declaration's own boundary: re-associating a fold is licensed by the declared
/// associativity, so `-` and `/` — the language's two non-associative arithmetic operators — must
/// not tile.
#[test]
fn a_combine_that_is_not_associative_is_refused_by_name() {
    for combine in [BinaryOp::Sub, BinaryOp::Div] {
        let declared_reduction = Reduction {
            combine,
            identity: 0.0,
        };
        let refusal = Folding::new(
            &declared_reduction,
            Element::Buffer { position: 0 },
            ScalarClass::Int,
        )
        .expect_err("a non-associative combine does not tile");
        let message = refusal.to_string();
        assert!(
            message.contains(combine.name()) && message.contains("not associative"),
            "the refusal names the operator and the reason: {message}"
        );
        assert!(
            matches!(refusal, TileRefusal::CombineIsNotAssociative(named) if named == combine),
            "the refusal carries the declared operator"
        );
    }
    // The associated ones tile, so the refusal is the boundary and not a blanket.
    for combine in [BinaryOp::Add, BinaryOp::Mul, BinaryOp::Max, BinaryOp::Min] {
        Folding::new(
            &Reduction {
                combine,
                identity: 0.0,
            },
            Element::Buffer { position: 0 },
            ScalarClass::Int,
        )
        .unwrap_or_else(|refusal| {
            panic!(
                "the associative combine `{}` tiles: {refusal}",
                combine.name()
            )
        });
    }
}

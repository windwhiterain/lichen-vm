//! The acceptance case: a reduction folded across the workgroup's lanes, judged against its
//! hand-derived closed form and against the per-lane implementation it replaces.
//! See `docs/notes/shared-memory-and-barriers.md` §9 step 8.

mod common;

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_compute::{ComputeOperator, ComputeValue};
use lichen_fusion::{BinaryOp, Expr, Reduction};
use lichen_fusion_bridge::{Element, Folding, ReadRoles, TileRefusal, projection};
use lichen_kernel_ir::{
    BufferSlot, IntWidth, KernelBody, KernelFragment, KernelRoles, KernelShape, LaneWidth,
    LaunchSet, ScalarClass, ScalarData, ScalarLeaf, Slice, Space, TargetFacts,
};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, Module};
use lichen_utils::extend::AsEnum;

/// The combine a reduction's `combine` role declares.
const COMBINE: &str = "combine";
/// The literal a reduction's `identity` role declares.
const IDENTITY: &str = "identity";

/// The lane width one workgroup runs.
const LANES: usize = LOCAL_SIZE_X as usize;

/// The count every declared loop is bounded by.
///
/// # Invariant
/// A loop conversion cannot be bounded by an argument of the function that writes it, so the bound
/// is a literal and the launch's own count selects its answer through the **data** instead: lane
/// `i` folds `count - i` values, so the last lane's fold is `count(count + 1) / 2` and every leg
/// runs at every test length without recompiling.
const BOUND: u64 = 192;

/// The data one launch over `count` elements folds: `1.0 ..= count`, then identity padding.
///
/// # Invariant
/// The declared loop's bound is the launch's own count, so every lane folds exactly the elements
/// the launch declared and the identity padding past them contributes nothing: the answer at
/// `out[count - 1]` is the fold over `[0, count)`, and the closed form of `1.0 ..= count` is
/// `count(count + 1) / 2`.
fn padded(count: u64) -> Vec<i64> {
    let mut data: Vec<i64> = vec![0_i64; BOUND as usize];
    for (position, value) in data[..count as usize].iter_mut().enumerate() {
        *value = (position + 1) as i64;
    }
    data
}

/// The reduction the pack declares: `claim:Reduction(combine,identity)`, with the node the
/// combine's own expression marks and the literal the identity's marks.
///
/// # Invariant
/// Both facts are read out of the fragment's marks and bridged out of the body, so what the
/// tiling is licensed by is the declaration a pack wrote and not a decision this file made. A
/// `combine` that is not one binary operator, or an `identity` that is not a literal, is refused
/// by name.
fn declared(fragment: &KernelFragment, interface: &str) -> Result<Reduction, String> {
    let combine = marked_value(fragment, interface, COMBINE).map_err(|refusal| refusal.message())?;
    let combine = projection(fragment, combine, ReadRoles::DATUM_ONLY)
        .map_err(|refusal| refusal.message())?;
    let Expr::Binary(operator, _, _) = &combine else {
        return Err(format!(
            "the `{COMBINE}` mark's expression `{combine}` is not one binary operator"
        ));
    };
    let identity =
        marked_value(fragment, interface, IDENTITY).map_err(|refusal| refusal.message())?;
    let identity = projection(fragment, identity, ReadRoles::DATUM_ONLY)
        .map_err(|refusal| refusal.message())?;
    let Some(identity) = identity.as_constant() else {
        return Err(format!(
            "the `{IDENTITY}` mark's expression `{identity}` is not a literal"
        ));
    };
    Ok(Reduction {
        combine: *operator,
        identity,
    })
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
/// A tiled body's domain is `(extent, count, index)` — the count the launch covers, the runtime
/// scalar the fold's iteration space is, and the lane — and a per-lane body's is `(extent, index)`.
/// Every leaf is `Int`, the lane included: the fold's **elements** are `Float` while its addresses
/// and its bounds are not, and a `Float` lane is what makes an address a float remainder the
/// language has no form for. The slice is the staging a tiled fold writes, one element per lane, in
/// `Workgroup` storage (`docs/notes/shared-memory-and-barriers.md` §5).
fn body_fragment(body: KernelBody, tiled: bool) -> KernelFragment {
    let leaves = if tiled { 3 } else { 2 };
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(
            (0..leaves)
                .map(|_| KernelShape::Scalar(ScalarClass::Int))
                .collect(),
        ),
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
/// The launch fixes one runtime scalar — the count the fold is over — and covers `extent` indices,
/// which are two different facts: a workgroup is `LANES` wide whatever count it folds.
fn run_at(
    context: &GpuContext,
    fragment: &KernelFragment,
    data: &[i64],
    extent: usize,
    count: u64,
    fetch: usize,
) -> Vec<i64> {
    let packed = pack(data);
    let leaves = [ScalarLeaf {
        class: ScalarClass::Int,
        bits: count as i64,
    }];
    let resident = context
        .run(
            &LaunchSet::single(fragment).with_leaves(&leaves),
            &[BufferSlot::Host(&packed)],
            extent,
        )
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
/// The `@loop` is the reduction's own: a counter, an accumulator, and a step that combines the
/// accumulator with a term under the operator the `combine` mark names. Its bound is a literal
/// because a loop conversion cannot be bounded by an argument of the function that writes it, so
/// what this program declares is the **algebra** — the combine and the identity — and the
/// iteration space is the launch's, which is what the transformation is applied to.
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
/// The declaration is the pack's and it is what the tiling reads: the `combine` and `identity`
/// marks bridged out of the body a `compute.parallel` compiled, so the operator the transform
/// folds with is the operator the program wrote and not one this file chose.
fn declared_reduction() -> Reduction {
    let (module, _value, _root) = common::run(source());
    let (interface, fragment) = dispatched(&module)
        .unwrap_or_else(|refusal| panic!("the program dispatched one claimed kernel: {refusal}"));
    declared(&fragment, &interface)
        .unwrap_or_else(|refusal| panic!("the claim's roles bridge: {refusal}"))
}

/// A tiled reduction answers the closed form, pinned against the per-lane implementation, on a
/// real device.
///
/// # Invariant
/// One declaration produces both the per-lane body and the tiled one, so the difference between
/// them is the schedule and nothing else: the per-lane loop folds the space once per lane, the
/// tiled body folds a strided subset per lane into a slice, separates the writes from the reads
/// with a barrier, and folds the slice with the same declared combine. Three answers are compared —
/// the closed form, the declared kernel on the device, and the tiled body — and the host path
/// refuses a staged body, so the closed form and the per-lane run are this class's evidence
/// (`docs/notes/shared-memory-and-barriers.md` §7).
#[test]
fn a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop() {
    let Some(context) = device("a_tiled_reduction_answers_the_closed_form_and_the_per_lane_loop")
    else {
        return;
    };
    // A whole workgroup at the declared count, so no surplus lane writes what a real lane reads.
    let reduction = declared_reduction();
    assert_eq!(
        reduction.combine,
        BinaryOp::Add,
        "the program's own marks declare the sum its body folds"
    );
    for &count in &[LANES as u64, 128, 600] {
        let data = padded(count);
        let folding = Folding::new(
            &reduction,
            count,
            Element::Buffer { position: 0 },
            ScalarClass::Int,
        )
        .unwrap_or_else(|refusal| panic!("the declaration tiles {count} element(s): {refusal}"));
        assert_eq!(
            folding.count(),
            count,
            "the fold is over the count the launch declared, not the tile it falls in"
        );

        let tiled = folding
            .tiled(LOCAL_SIZE_X, 0)
            .unwrap_or_else(|refusal| panic!("{count} element(s) tile: {refusal}"));
        let tiled_fragment = body_fragment(tiled.into_body(), true);
        tiled_fragment
            .body
            .validate()
            .unwrap_or_else(|broken| panic!("the tiled body is well formed: {broken}"));
        let owed = tiled_fragment
            .barrier_scan(tiled_fragment.body.parameters()[2])
            .unwrap_or_else(|refusal| panic!("the tiled body's scan decides: {refusal}"));
        assert_eq!(
            owed.len(),
            1,
            "the tiled body's slice writes are separated from its reads by one barrier"
        );

        // The per-lane implementation is the same declaration folded in one lane, which is
        // the floor the tiling replaces and the body this one is differenced against
        // (`docs/notes/shared-memory-and-barriers.md` §9 step 8).
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
            count,
            count as usize,
        );
        let tiled_answer = run_at(&context, &tiled_fragment, &data, LANES, count, LANES);

        let expected = (count * (count + 1) / 2) as i64;
        assert_eq!(
            per_lane[count as usize - 1],
            expected,
            "the per-lane loop reduces {count} element(s) to the closed form"
        );
        let total = tiled_answer[0];
        assert_eq!(
            total,
            expected,
            "the workgroup's {LANES} lane(s) fold {count} element(s) across a slice and a barrier \
             to the closed form"
        );
        assert_eq!(
            total, per_lane[count as usize - 1],
            "the tiled body and the per-lane loop answer one declaration alike"
        );
    }
}

/// The same tiling path folds a maximum and a product, because the licence is the declaration's.
///
/// # Invariant
/// Nothing in the tiling reads the operator beyond applying it: one `Folding::tiled` call produced
/// these bodies and the sum body above, and the declared `combine` names `max` and `*` here and `+`
/// there. A transform that pattern-matched the arithmetic would have one answer, so each operator
/// is driven by a declaration and checked against a hand-derived closed form.
#[test]
fn a_second_combine_tiles_through_the_same_path() {
    let Some(context) = device("a_second_combine_tiles_through_the_same_path") else {
        return;
    };
    for (index, value) in Folding::new(
        &Reduction {
            combine: BinaryOp::Max,
            identity: 0.0,
        },
        64,
        Element::Buffer { position: 0 },
        ScalarClass::Int,
    )
    .expect("a maximum tiles")
    .tiled(LOCAL_SIZE_X, 0)
    .expect("it tiles")
    .body()
    .values
    .iter()
    .enumerate()
    {
        println!("tiled value {index}: {value:?}");
    }
    for &count in &[64_u64, 100, 512] {
        // The tiled fold's iteration space is a whole workgroup tile, so every element up to that
        // bound is an element it folds: a maximum over them is the largest and a product is their
        // running product, and both are the closed form the oracle states below.
        let data: Vec<i64> = (0..BOUND).map(|value| ((value % 5) + 1) as i64).collect();
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
                count,
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
            let per_lane = run_at(
                &context,
                &per_lane_fragment,
                &data,
                count as usize,
                count,
                count as usize,
            );
            let tiled_fragment = body_fragment(tiled.into_body(), true);
            for (index, value) in tiled_fragment.body.values.iter().enumerate() {
                println!("max tiled {index}: {value:?}");
            }
            let tiled_answer = run_at(&context, &tiled_fragment, &data, LANES, count, LANES);
            let closed = closed_form(declared_reduction.combine, &data, count);
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
/// a product is their running product, both computed here from the data the launch was handed.
fn closed_form(combine: BinaryOp, elements: &[i64], count: u64) -> i64 {
    let tiled = count.div_ceil(u64::from(LOCAL_SIZE_X)) * u64::from(LOCAL_SIZE_X);
    let covered = &elements[..tiled as usize];
    match combine {
        BinaryOp::Max => covered.iter().copied().fold(i64::MIN, i64::max),
        BinaryOp::Mul => covered.iter().copied().fold(1_i64, |left, right| left * right),
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
            64,
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
            64,
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

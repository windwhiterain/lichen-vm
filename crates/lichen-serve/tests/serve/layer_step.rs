//! A layer of three kernels with three different input shapes, batched by sequence.

use super::common::{floats, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

/// Scale, summarise and add: the shape of a normalisation's two passes, over two sequences.
///
/// # Invariant
/// Three kernels, and **each takes a different input struct** — two buffers, one, then two again —
/// while the graph's own parameter holds two. That is the shape this test exists to try: a step
/// whose dispatches do not share one argument shape. A lane's sequence is `lane / dim`, so the scale
/// reads the shared per-column weights and the add reads its own sequence's summary.
const LAYER: &str = r#"
--- compute = import "compute.lichen" ---
Out    = struct{.e (compute.Buf Float)}
TwoIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
OneIn  = struct{.y (compute.Buf Float)}
MixIn  = struct{.y (compute.Buf Float), .m (compute.Buf Float)}
ScalePar = struct{.n Int, .dim Int, .in TwoIn, .out Out}
ScaleArg = struct{.n Int, .dim Int, .I TwoIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  column = i - (i / k.dim) * k.dim
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  weight = (compute.read ((compute.Read _)(.from k.in.w, .at column)) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value * weight)))
}
scale = compute.parallel scale_raw "gpu" Out
ReducePar = struct{.n Int, .dim Int, .in OneIn, .out Out}
ReduceArg = struct{.n Int, .dim Int, .I OneIn}
reduce_raw = (k : ReducePar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.y, .at (lane * k.dim + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
reduce = compute.parallel (reduce_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
MixPar = struct{.n Int, .dim Int, .in MixIn, .out Out}
MixArg = struct{.n Int, .dim Int, .I MixIn}
mix_raw = (k : MixPar) => {
  i = compute.range k.n
  sequence = i / k.dim
  value = (compute.read ((compute.Read _)(.from k.in.y, .at i)) : Float)
  summary = (compute.read ((compute.Read _)(.from k.in.m, .at sequence)) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value + summary)))
}
mix = compute.parallel mix_raw "gpu" Out
StepIn = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
GArg = struct{.n Int, .lanes Int, .in StepIn}
step = (s : GArg) => {
  scaled = (compute.plrun scale ((ScaleArg)(.n s.n, .dim 3, .I TwoIn(.x s.in.x, .w s.in.w))) : Out)
  summed = (compute.plrun reduce ((ReduceArg)(.n s.lanes, .dim 3, .I OneIn(.y scaled.e))) : Out)
  compute.plrun mix ((MixArg)(.n s.n, .dim 3, .I MixIn(.y scaled.e, .m summed.e)))
}
built = compute.graph step
built
"#;

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> ResidentBuffer {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => *resident,
        _ => panic!("this step answers with one buffer, and this one answered otherwise"),
    }
}

/// Three kernels with three argument shapes record and run as one submission.
///
/// # Invariant
/// `1..6` scaled by `2, 1, 1` is `2, 2, 3` and `8, 5, 6`; their sums are `7` and `19`; and adding
/// each sequence's sum back to its row is `9, 9, 10` and `27, 24, 25`. Every number is checkable by
/// hand, and no sequence's sum may reach the other one's row.
#[test]
fn a_layer_of_three_shapes_runs_as_one_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_layer_of_three_shapes_runs_as_one_submission: no device");
        return;
    };
    let prepared = session
        .prepare(LAYER)
        .unwrap_or_else(|refusal| panic!("the layer prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 3, "the held step is three kernels");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(6),
            RunArgument::Count(2),
            floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            floats(&[2.0, 1.0, 1.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the layer runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "three kernels of three different shapes are one submission"
    );
    let answered = only(&stepped);
    assert_eq!(
        read(&session, answered),
        vec![9.0, 9.0, 10.0, 27.0, 24.0, 25.0],
        "each sequence's own summary, added back to its own row"
    );
    session.release(answered);
    println!("a three-kernel layer with three argument shapes ran as one submission");
}

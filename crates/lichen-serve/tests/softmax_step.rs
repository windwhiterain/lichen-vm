//! A softmax attention over a resident cache, in one submission.

mod common;

use common::{floats, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

/// Softmax attention over two sequences of three slots of two columns.
///
/// # Invariant
/// Seven kernels because a fold's body reads one buffer: weigh, exponentiate and scale each bind
/// what they combine before they write, and every fold sums a single buffer. The scale writes
/// transposed, `(sequence, column, position)`, so the last fold reads a contiguous run. A lane
/// carries its own sequence. The cache arrives as the **resident** a previous step wrote.
const ATTEND: &str = r#"
--- compute = import "compute.lichen" ---
Out     = struct{.e (compute.Buf Float)}
WeighIn = struct{.k (compute.Buf Float), .q (compute.Buf Float)}
TallyIn = struct{.t (compute.Buf Float)}
PeakIn  = struct{.s (compute.Buf Float)}
ExpoIn  = struct{.s (compute.Buf Float), .m (compute.Buf Float)}
ScaleIn = struct{.s (compute.Buf Float), .m (compute.Buf Float), .total (compute.Buf Float), .v (compute.Buf Float)}
PosIn   = struct{.w (compute.Buf Float)}
WeighPar = struct{.n Int, .cap Int, .dim Int, .in WeighIn, .out Out}
WeighArg = struct{.n Int, .cap Int, .dim Int, .I WeighIn}
weigh_raw = (k : WeighPar) => {
  i = compute.range k.n
  sequence = i / (k.cap * k.dim)
  column = i - (i / k.dim) * k.dim
  value = compute.read ((compute.Read _)(.from k.in.k, .at i))
  query = compute.read ((compute.Read _)(.from k.in.q, .at (sequence * k.dim + column)))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value * query)))
}
weigh = compute.parallel weigh_raw "gpu"
SumPar = struct{.n Int, .cap Int, .dim Int, .in TallyIn, .out Out}
SumArg = struct{.n Int, .cap Int, .dim Int, .I TallyIn}
sum_raw = (k : SumPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.dim + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
sum = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity)") "gpu"
PeakPar = struct{.n Int, .cap Int, .in PeakIn, .out Out}
PeakArg = struct{.n Int, .cap Int, .I PeakIn}
peak_raw = (k : PeakPar) => {
  lane = compute.range k.n
  base = lane * k.cap
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, ((0.0 - 1.0) / 0.0) ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity)") "gpu"
ExpoPar = struct{.n Int, .cap Int, .in ExpoIn, .out Out}
ExpoArg = struct{.n Int, .cap Int, .I ExpoIn}
expo_raw = (k : ExpoPar) => {
  i = compute.range k.n
  sequence = i / k.cap
  value = compute.read ((compute.Read _)(.from k.in.s, .at i))
  top = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  shifted = value - top
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (exp shifted)))
}
expo = compute.parallel expo_raw "gpu"
TotalPar = struct{.n Int, .cap Int, .in TallyIn, .out Out}
TotalArg = struct{.n Int, .cap Int, .I TallyIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.cap + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu"
ScalePar = struct{.n Int, .cap Int, .dim Int, .in ScaleIn, .out Out}
ScaleArg = struct{.n Int, .cap Int, .dim Int, .I ScaleIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  sequence = i / (k.cap * k.dim)
  column = i - (i / k.dim) * k.dim
  position = (i / k.dim) - sequence * k.cap
  weight = compute.read ((compute.Read _)(.from k.in.s, .at (sequence * k.cap + position)))
  divisor = compute.read ((compute.Read _)(.from k.in.total, .at sequence))
  value = compute.read ((compute.Read _)(.from k.in.v, .at i))
  share = weight / divisor
  compute.write ((compute.Write _)(.to k.out.e, .at ((sequence * k.dim + column) * k.cap + position), .value (share * value)))
}
scale = compute.parallel scale_raw "gpu"
PosPar = struct{.n Int, .cap Int, .in PosIn, .out Out}
PosArg = struct{.n Int, .cap Int, .I PosIn}
pos_raw = (k : PosPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.cap + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
positions = compute.parallel (pos_raw ~ "claim:Reduction(combine,identity)") "gpu"
StepIn = struct{.k (compute.Buf Float), .v (compute.Buf Float), .q (compute.Buf Float)}
GArg = struct{.words Int, .slots Int, .sequences Int, .columns Int, .in StepIn}
step = (s : GArg) => {
  weighted = (compute.plrun weigh ((WeighArg)(.n s.words, .cap 3, .dim 2, .I WeighIn(.k s.in.k, .q s.in.q))) : Out)
  summed = (compute.plrun sum ((SumArg)(.n s.slots, .cap 3, .dim 2, .I TallyIn(.t weighted.e))) : Out)
  topped = (compute.plrun peak ((PeakArg)(.n s.sequences, .cap 3, .I PeakIn(.s summed.e))) : Out)
  shifted = (compute.plrun expo ((ExpoArg)(.n s.slots, .cap 3, .I ExpoIn(.s summed.e, .m topped.e))) : Out)
  divided = (compute.plrun total ((TotalArg)(.n s.sequences, .cap 3, .I TallyIn(.t shifted.e))) : Out)
  shared = (compute.plrun scale ((ScaleArg)(.n s.words, .cap 3, .dim 2, .I ScaleIn(.s shifted.e, .m topped.e, .total divided.e, .v s.in.v))) : Out)
  compute.plrun positions ((PosArg)(.n s.columns, .cap 3, .I PosIn(.w shared.e)))
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

/// What a softmax attention over these slots should answer, computed on the host.
fn expected(scores: &[[f32; 3]; 2], values: &[[[f32; 2]; 3]; 2]) -> [f32; 4] {
    let mut answered = [0.0f32; 4];
    for sequence in 0..2 {
        let top = scores[sequence]
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, f32::max);
        let total: f32 = scores[sequence]
            .iter()
            .map(|score| (score - top).exp())
            .sum();
        for column in 0..2 {
            answered[sequence * 2 + column] = (0..3)
                .map(|position| {
                    let share = (scores[sequence][position] - top).exp() / total;
                    share * values[sequence][position][column]
                })
                .sum();
        }
    }
    answered
}

/// Softmax attention runs as seven kernels in one submission, and its answer is the softmax's.
///
/// # Invariant
/// The scores are `1, 1, 2` and `2, 0, 1`, and the value cache is distinct from the key cache, so a
/// step that mixed the two up would answer other numbers. `exp` is an approximation on both sides,
/// so the comparison is within `1e-4` rather than exact.
#[test]
fn softmax_attention_runs_as_seven_kernels_in_one_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED softmax_attention_runs_as_seven_kernels_in_one_submission: no device");
        return;
    };
    let keys = [
        1.0f32, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0, 1.0, -1.0,
    ];
    let values = [
        1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 1.0, 0.0, 0.0, 1.0, 2.0, 2.0,
    ];
    let cache = session
        .upload(&keys)
        .unwrap_or_else(|refusal| panic!("the key cache loads: {refusal}"));
    let cached = session
        .upload(&values)
        .unwrap_or_else(|refusal| panic!("the value cache loads: {refusal}"));
    let prepared = session
        .prepare(ATTEND)
        .unwrap_or_else(|refusal| panic!("the attention step prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 7, "the held step is seven kernels");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(12),
            RunArgument::Count(6),
            RunArgument::Count(2),
            RunArgument::Count(4),
            RunArgument::Resident(cache),
            RunArgument::Resident(cached),
            floats(&[1.0, 1.0, 1.0, 0.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the attention step runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "seven kernels over two resident caches are one submission"
    );
    let answered = read(&session, only(&stepped));
    let scores = [[1.0f32, 1.0, 2.0], [2.0, 0.0, 1.0]];
    let rows = [
        [[1.0f32, 2.0], [3.0, 4.0], [5.0, 6.0]],
        [[1.0, 0.0], [0.0, 1.0], [2.0, 2.0]],
    ];
    let oracle = expected(&scores, &rows);
    for (index, (got, want)) in answered.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1.0e-4,
            "element {index}: the step answered {got} and the softmax is {want}"
        );
    }
    println!("softmax attention answered {answered:?} against the host's {oracle:?}");
    session.release(only(&stepped));
    session.release(cache);
    session.release(cached);
}

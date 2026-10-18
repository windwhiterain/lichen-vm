//! A transformer block — attention, residual, reduce-and-broadcast, feed-forward — in one step.

mod common;

use common::{floats, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

/// The block a real model repeats: attention, a residual, a reduce-and-broadcast and a projection.
///
/// # Invariant
/// Twelve kernels because a fold's body reads one buffer: every stage that multiplies two values does
/// it elementwise and binds both, so each fold sums a single buffer. The scale writes transposed and
/// the feed-forward spreads before it projects, which keeps both folds contiguous. A lane carries its
/// own sequence. The two caches arrive as the **residents** a previous step wrote.
pub const BLOCK: &str = r#"
--- compute = import "compute.lichen" ---
Out     = struct{.e (compute.Buf Float)}
WeighIn = struct{.k (compute.Buf Float), .q (compute.Buf Float)}
TallyIn = struct{.t (compute.Buf Float)}
PeakIn  = struct{.s (compute.Buf Float)}
ExpoIn  = struct{.s (compute.Buf Float), .m (compute.Buf Float)}
ScaleIn = struct{.s (compute.Buf Float), .m (compute.Buf Float), .total (compute.Buf Float), .v (compute.Buf Float)}
PosIn   = struct{.w (compute.Buf Float)}
AddIn   = struct{.a (compute.Buf Float), .b (compute.Buf Float)}
MixIn   = struct{.h (compute.Buf Float), .m (compute.Buf Float)}
SpreadIn = struct{.n (compute.Buf Float), .w (compute.Buf Float)}
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
AddPar = struct{.n Int, .in AddIn, .out Out}
AddArg = struct{.n Int, .I AddIn}
add_raw = (k : AddPar) => {
  i = compute.range k.n
  left = compute.read ((compute.Read _)(.from k.in.a, .at i))
  right = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (left + right)))
}
add = compute.parallel add_raw "gpu"
MixPar = struct{.n Int, .dim Int, .in MixIn, .out Out}
MixArg = struct{.n Int, .dim Int, .I MixIn}
mix_raw = (k : MixPar) => {
  i = compute.range k.n
  sequence = i / k.dim
  value = compute.read ((compute.Read _)(.from k.in.h, .at i))
  summary = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value + summary)))
}
mix = compute.parallel mix_raw "gpu"
SpreadPar = struct{.n Int, .dim Int, .in SpreadIn, .out Out}
SpreadArg = struct{.n Int, .dim Int, .I SpreadIn}
spread_raw = (k : SpreadPar) => {
  i = compute.range k.n
  sequence = i / (k.dim * k.dim)
  rest = i - sequence * (k.dim * k.dim)
  column = rest / k.dim
  inner = rest - column * k.dim
  hidden = compute.read ((compute.Read _)(.from k.in.n, .at (sequence * k.dim + inner)))
  weight = compute.read ((compute.Read _)(.from k.in.w, .at (column * k.dim + inner)))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (hidden * weight)))
}
spread = compute.parallel spread_raw "gpu"
ProjectPar = struct{.n Int, .dim Int, .in PosIn, .out Out}
ProjectArg = struct{.n Int, .dim Int, .I PosIn}
project_raw = (k : ProjectPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.dim + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
project = compute.parallel (project_raw ~ "claim:Reduction(combine,identity)") "gpu"
StepIn = struct{.x (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .q (compute.Buf Float), .w (compute.Buf Float)}
GArg = struct{.words Int, .slots Int, .sequences Int, .columns Int, .spread Int, .in StepIn}
step = (s : GArg) => {
  weighted = (compute.plrun weigh ((WeighArg)(.n s.words, .cap 3, .dim 2, .I WeighIn(.k s.in.k, .q s.in.q))) : Out)
  summed = (compute.plrun sum ((SumArg)(.n s.slots, .cap 3, .dim 2, .I TallyIn(.t weighted.e))) : Out)
  topped = (compute.plrun peak ((PeakArg)(.n s.sequences, .cap 3, .I PeakIn(.s summed.e))) : Out)
  shifted = (compute.plrun expo ((ExpoArg)(.n s.slots, .cap 3, .I ExpoIn(.s summed.e, .m topped.e))) : Out)
  divided = (compute.plrun total ((TotalArg)(.n s.sequences, .cap 3, .I TallyIn(.t shifted.e))) : Out)
  shared = (compute.plrun scale ((ScaleArg)(.n s.words, .cap 3, .dim 2, .I ScaleIn(.s shifted.e, .m topped.e, .total divided.e, .v s.in.v))) : Out)
  attended = (compute.plrun positions ((PosArg)(.n s.columns, .cap 3, .I PosIn(.w shared.e))) : Out)
  held = (compute.plrun add ((AddArg)(.n s.columns, .I AddIn(.a s.in.x, .b attended.e))) : Out)
  summarised = (compute.plrun sum ((SumArg)(.n s.sequences, .cap 3, .dim 2, .I TallyIn(.t held.e))) : Out)
  normalised = (compute.plrun mix ((MixArg)(.n s.columns, .dim 2, .I MixIn(.h held.e, .m summarised.e))) : Out)
  spread_out = (compute.plrun spread ((SpreadArg)(.n s.spread, .dim 2, .I SpreadIn(.n normalised.e, .w s.in.w))) : Out)
  compute.plrun project ((ProjectArg)(.n s.columns, .dim 2, .I PosIn(.w spread_out.e)))
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

/// The block, written again on the host, so the device's answer has an independent oracle.
///
/// # Invariant
/// It is the same twelve stages as the kernels: score, softmax, mix the values, add the residual,
/// reduce and broadcast, then spread by the weights and project. Two sequences of two units over a
/// cache of three positions, which is what makes every index below fixed.
fn expected(
    state: &[[f32; 2]; 2],
    keys: &[[[f32; 2]; 3]; 2],
    values: &[[[f32; 2]; 3]; 2],
    query: &[[f32; 2]; 2],
    feed: &[[f32; 2]; 2],
) -> [f32; 4] {
    let mut answered = [0.0f32; 4];
    for sequence in 0..2 {
        let scores: Vec<f32> = (0..3)
            .map(|position| {
                (0..2)
                    .map(|column| query[sequence][column] * keys[sequence][position][column])
                    .sum()
            })
            .collect();
        let top = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let total: f32 = scores.iter().map(|score| (score - top).exp()).sum();
        let attended: Vec<f32> = (0..2)
            .map(|column| {
                (0..3)
                    .map(|position| {
                        let share = (scores[position] - top).exp() / total;
                        share * values[sequence][position][column]
                    })
                    .sum()
            })
            .collect();
        let held: Vec<f32> = (0..2)
            .map(|unit| state[sequence][unit] + attended[unit])
            .collect();
        let summary: f32 = held.iter().sum();
        let normalised: Vec<f32> = held.iter().map(|unit| unit + summary).collect();
        for column in 0..2 {
            answered[sequence * 2 + column] = (0..2)
                .map(|inner| normalised[inner] * feed[column][inner])
                .sum();
        }
    }
    answered
}

/// A block of twelve kernels runs as one submission, and its answer is the host's own block.
///
/// # Invariant
/// The state, the query and the feed-forward weights are all distinct from each other and from both
/// caches, so a stage that read the wrong buffer would answer other numbers. `exp` is an
/// approximation on both sides, so the comparison is within `1e-4` rather than exact.
#[test]
fn a_block_of_twelve_kernels_runs_in_one_submission() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_block_of_twelve_kernels_runs_in_one_submission: no device");
        return;
    };
    let keys = [
        [[1.0f32, 0.0], [0.0, 1.0], [1.0, 1.0]],
        [[2.0, 0.0], [0.0, 2.0], [1.0, -1.0]],
    ];
    let values = [
        [[1.0f32, 2.0], [3.0, 4.0], [5.0, 6.0]],
        [[1.0, 0.0], [0.0, 1.0], [2.0, 2.0]],
    ];
    let state = [[1.0f32, 0.0], [0.0, 1.0]];
    let query = [[1.0f32, 1.0], [1.0, 0.0]];
    let feed = [[1.0f32, 2.0], [3.0, 1.0]];
    let flat = |rows: &[[f32; 2]; 2]| -> Vec<f32> { rows.iter().flatten().copied().collect() };
    let flattened = |rows: &[[[f32; 2]; 3]; 2]| -> Vec<f32> {
        rows.iter().flatten().flatten().copied().collect()
    };
    let held = session
        .upload(&flattened(&keys))
        .unwrap_or_else(|refusal| panic!("the key cache loads: {refusal}"));
    let cached = session
        .upload(&flattened(&values))
        .unwrap_or_else(|refusal| panic!("the value cache loads: {refusal}"));
    let prepared = session
        .prepare(BLOCK)
        .unwrap_or_else(|refusal| panic!("the block prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 12, "the held step is twelve kernels");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(12),
            RunArgument::Count(6),
            RunArgument::Count(2),
            RunArgument::Count(4),
            RunArgument::Count(8),
            floats(&flat(&state)),
            RunArgument::Resident(held),
            RunArgument::Resident(cached),
            floats(&flat(&query)),
            floats(&flat(&feed)),
        ])
        .unwrap_or_else(|refusal| panic!("the block runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "a whole block is one submission"
    );
    let answered = read(&session, only(&stepped));
    let oracle = expected(&state, &keys, &values, &query, &feed);
    for (index, (got, want)) in answered.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1.0e-4,
            "element {index}: the block answered {got} and the host's block {want}"
        );
    }
    println!("the block answered {answered:?} against the host's {oracle:?}");
    session.release(only(&stepped));
    session.release(held);
    session.release(cached);
}

/// Six blocks are six scoped steps, and the sixth crosses the dispatch cap of one submission.
///
/// # Invariant
/// The block is stepped six times with its own answer as the next residual, so six blocks cost six
/// submissions and seventy-two dispatches — more than one submission holds, which is why a stack is a
/// loop and not one graph. The answer is the host block iterated the same six times.
#[test]
fn a_stack_of_six_blocks_costs_six_submissions() {
    const DEPTH: usize = 6;
    const REPEATS: usize = 20;
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_stack_of_six_blocks_costs_six_submissions: no device");
        return;
    };
    let keys = [
        [[1.0f32, 0.0], [0.0, 1.0], [1.0, 1.0]],
        [[2.0, 0.0], [0.0, 2.0], [1.0, -1.0]],
    ];
    let values = [
        [[1.0f32, 2.0], [3.0, 4.0], [5.0, 6.0]],
        [[1.0, 0.0], [0.0, 1.0], [2.0, 2.0]],
    ];
    let state = [[1.0f32, 0.0], [0.0, 1.0]];
    let query = [[1.0f32, 1.0], [1.0, 0.0]];
    let feed = [[1.0f32, 2.0], [3.0, 1.0]];
    let flat = |rows: &[[f32; 2]; 2]| -> Vec<f32> { rows.iter().flatten().copied().collect() };
    let flattened = |rows: &[[[f32; 2]; 3]; 2]| -> Vec<f32> {
        rows.iter().flatten().flatten().copied().collect()
    };
    let as_rows = |words: &[f32]| -> [[f32; 2]; 2] { [[words[0], words[1]], [words[2], words[3]]] };
    let held = session
        .upload(&flattened(&keys))
        .unwrap_or_else(|refusal| panic!("the key cache loads: {refusal}"));
    let cached = session
        .upload(&flattened(&values))
        .unwrap_or_else(|refusal| panic!("the value cache loads: {refusal}"));
    let prepared = session
        .prepare(BLOCK)
        .unwrap_or_else(|refusal| panic!("the block prepares: {refusal}"));

    let arguments = |words: &[f32]| -> Vec<RunArgument> {
        vec![
            RunArgument::Count(12),
            RunArgument::Count(6),
            RunArgument::Count(2),
            RunArgument::Count(4),
            RunArgument::Count(8),
            floats(words),
            RunArgument::Resident(held),
            RunArgument::Resident(cached),
            floats(&flat(&query)),
            floats(&flat(&feed)),
        ]
    };

    let mut words = flat(&state);
    let mut submissions: u64 = 0;
    for block in 0..DEPTH {
        let stepped = prepared
            .step(arguments(&words))
            .unwrap_or_else(|refusal| panic!("block {block} runs: {refusal}"));
        assert_eq!(
            (stepped.cost.submissions, stepped.cost.fence_waits),
            (1, 1),
            "block {block} is one submission"
        );
        submissions += stepped.cost.submissions;
        let answered = only(&stepped);
        words = read(&session, answered);
        session.release(answered);
    }
    assert_eq!(
        submissions, DEPTH as u64,
        "six blocks over six submissions: seventy-two dispatches, past the sixty-four a step holds"
    );

    let mut oracle = flat(&state);
    for _ in 0..DEPTH {
        oracle = expected(&as_rows(&oracle), &keys, &values, &query, &feed).to_vec();
    }
    for (index, (got, want)) in words.iter().zip(oracle.iter()).enumerate() {
        assert!(
            (got - want).abs() <= 1.0e-4 * want.abs().max(1.0),
            "element {index}: six blocks answered {got} and the host's six {want}"
        );
    }

    let started = std::time::Instant::now();
    for _ in 0..REPEATS {
        let mut state = flat(&state);
        for _ in 0..DEPTH {
            let stepped = prepared
                .step(arguments(&state))
                .unwrap_or_else(|refusal| panic!("a stacked block runs: {refusal}"));
            let answered = only(&stepped);
            state = read(&session, answered);
            session.release(answered);
        }
        std::hint::black_box(&state);
    }
    let per_token = started.elapsed().as_secs_f64() * 1.0e3 / REPEATS as f64;
    println!(
        "six blocks: 72 dispatches over 6 submissions, {per_token:.2} ms a token, {:.2} ms a block",
        per_token / DEPTH as f64
    );
    session.release(held);
    session.release(cached);
}

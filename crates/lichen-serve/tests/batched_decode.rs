//! A whole decode step over three sequences: score, sample and append, all batched by slot.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// `examples/generate.lichen`'s three kernels, each turned inside out over a batch of sequences.
///
/// # Invariant
/// A lane carries its own sequence: the score is `S × rows` lanes and a lane's row is
/// `lane - sequence × rows`, the sampler is one lane per sequence, and the append is one lane per
/// cache slot, so its position comes from the `Int` buffer rather than from a scalar. The step takes
/// three counts because each dispatch's extent is one of them and an extent cannot be computed.
const BATCHED: &str = r#"
--- compute = import "compute.lichen" ---
ScoreIn  = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
ScoreOut = struct{.y (compute.Buf Float)}
ScorePar = struct{.n Int, .rows Int, .dim Int, .in ScoreIn, .out ScoreOut}
ScoreArg = struct{.n Int, .rows Int, .dim Int, .I ScoreIn}
score_raw = (k : ScorePar) => {
  lane = compute.range k.n
  sequence = lane / k.rows
  row = lane - sequence * k.rows
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at (sequence * k.dim + (s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
PickIn  = struct{.y (compute.Buf Float)}
Pick    = struct{.p (compute.Buf Float)}
PickPar = struct{.n Int, .rows Int, .in PickIn, .out Pick}
PickArg = struct{.n Int, .rows Int, .I PickIn}
greedy_raw = (k : PickPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from k.in.y, .at (base + s(0) - 1)))) ~ "combine"), (s(2) + ((1 * ((compute.read ((compute.Read _)(.from k.in.y, .at (base + s(0) - 1))) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.p, .at lane, .value (int2float (fold (k.rows, ((0.0 - 1.0) / 0.0) ~ "identity", 0)))))
}
greedy = compute.parallel (greedy_raw ~ "claim:Reduction(combine,identity)") "gpu"
AppIn  = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
KVOut  = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
AppPar = struct{.n Int, .cap Int, .dim Int, .in AppIn, .out KVOut}
AppArg = struct{.n Int, .cap Int, .dim Int, .I AppIn}
bappend_raw = (k : AppPar) => {
  lane = compute.range k.n
  per_sequence = k.cap * k.dim
  sequence = lane / per_sequence
  rest = lane - sequence * per_sequence
  position = rest / k.dim
  column = rest - position * k.dim
  mine = compute.read ((compute.Read _)(.from k.in.pos, .at sequence))
  fresh = position == mine
  old_k = compute.read ((compute.Read _)(.from k.in.k, .at lane))
  old_v = compute.read ((compute.Read _)(.from k.in.v, .at lane))
  new_k = if fresh then (compute.read ((compute.Read _)(.from k.in.nk, .at (sequence * k.dim + column)))) else old_k
  new_v = if fresh then (compute.read ((compute.Read _)(.from k.in.nv, .at (sequence * k.dim + column)))) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value (new_v ~ "element"))))
}
bappend = compute.parallel (bappend_raw ~ "claim:Reduction(element)") "gpu"
StepIn = struct{.w (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
GArg = struct{.n Int, .lanes Int, .cells Int, .in StepIn}
step = (s : GArg) => {
  scored = (compute.plrun score ((ScoreArg)(.n s.lanes, .rows 4, .dim 5, .I ScoreIn(.w s.in.w, .x s.in.nk))) : ScoreOut)
  tok = (compute.plrun greedy ((PickArg)(.n s.n, .rows 4, .I PickIn(.y scored.y))) : Pick)
  cached = (compute.plrun bappend ((AppArg)(.n s.cells, .cap 4, .dim 5, .I AppIn(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .pos s.in.pos))) : KVOut)
  (tok.p, cached.k, cached.v)
}
built = compute.graph step
built
"#;

/// `examples/generate.lichen`'s score weights, four rows of five.
const WEIGHTS: [f32; 20] = [
    2.0, 2.0, -3.0, 3.0, 1.0, 4.0, 2.0, -5.0, 3.0, 2.0, 0.0, 4.0, -6.0, 3.0, 3.0, 5.0, 4.0, -3.0,
    8.0, 4.0,
];

/// Five columns, four rows, four positions: the shape `generate.lichen` scores a token with.
const COLUMNS: usize = 5;
const ROWS: usize = 4;
const CAPACITY: usize = 4;

/// `values` as the packed bytes a `Float` argument carries.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// `positions` as the packed bytes an `Int` argument carries, eight bytes each.
fn positions(values: &[i64]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Int,
        data: values
            .iter()
            .flat_map(|position| position.to_le_bytes())
            .collect(),
    }
}

/// The floats a resident holds.
fn read(session: &Session, resident: ResidentBuffer) -> Vec<f32> {
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the value is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the step declared Float buffers, and this is {other:?}"),
    }
}

/// The three residents a step answered with: the token, the key cache and the value cache.
fn returns(stepped: &lichen_serve::Stepped) -> [ResidentBuffer; 3] {
    let [
        RunResult::Resident(token),
        RunResult::Resident(keys),
        RunResult::Resident(values),
    ] = &stepped.results[..]
    else {
        panic!("a step answers with its token and its two caches, and this one answered otherwise");
    };
    [*token, *keys, *values]
}

/// One sequence's step, as every argument a batched step takes with one sequence in the batch.
fn arguments_for(sequences: usize, keys: &[f32], values: &[f32]) -> Vec<RunArgument> {
    vec![
        RunArgument::Count(sequences as i64),
        RunArgument::Count((sequences * ROWS) as i64),
        RunArgument::Count((sequences * CAPACITY * COLUMNS) as i64),
        floats(&WEIGHTS),
        floats(&vec![0.0; sequences * CAPACITY * COLUMNS]),
        floats(&vec![0.0; sequences * CAPACITY * COLUMNS]),
        floats(keys),
        floats(values),
        positions(&vec![0; sequences]),
    ]
}

/// A sequence's token and cache slot do not depend on what else is in the batch.
///
/// # Invariant
/// The one-sequence run and the three-sequence run use the same prepared graph and differ only in
/// the counts and the buffer lengths, so a difference in the first sequence's token or its twenty
/// cache words is the batch crossing sequences. The three keys are distinct, so a slot that read
/// another sequence's key would score differently.
#[test]
fn a_batched_step_answers_a_sequence_as_if_it_were_alone() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_batched_step_answers_a_sequence_as_if_it_were_alone: no device");
        return;
    };
    let prepared = session
        .prepare(BATCHED)
        .unwrap_or_else(|refusal| panic!("the batched decode step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        3,
        "the held step is score, sample and append"
    );

    let alone = [1.0f32, 2.0, -1.0, -1.0, -1.0];
    let single = prepared
        .step(arguments_for(
            1,
            &alone,
            &alone.iter().map(|w| w * 10.0).collect::<Vec<_>>(),
        ))
        .unwrap_or_else(|refusal| panic!("the one-sequence step runs: {refusal}"));
    assert_eq!(
        (single.cost.submissions, single.cost.fence_waits),
        (1, 1),
        "a whole batched decode step is one submission"
    );
    let [single_token, single_keys, single_values] = returns(&single);
    let one_token = read(&session, single_token);
    let one_keys = read(&session, single_keys);
    assert_eq!(
        one_keys.len(),
        CAPACITY * COLUMNS,
        "one sequence owns four positions of five columns"
    );
    assert_eq!(
        &one_keys[0..5],
        &alone,
        "and its first position is the key this step appended"
    );

    let three: Vec<[f32; 5]> = vec![
        [1.0, 2.0, -1.0, -1.0, -1.0],
        [1.0, 1.0, 1.0, 1.0, 1.0],
        [0.0, 1.0, 0.0, 1.0, 0.0],
    ];
    let flat: Vec<f32> = three.iter().flatten().copied().collect();
    let tenfold: Vec<f32> = flat.iter().map(|word| word * 10.0).collect();
    let batched = prepared
        .step(arguments_for(3, &flat, &tenfold))
        .unwrap_or_else(|refusal| panic!("the three-sequence step runs: {refusal}"));
    assert_eq!(
        batched.cost.submissions, 1,
        "three sequences are still one submission"
    );
    let [batched_tokens, batched_keys, batched_values] = returns(&batched);
    let tokens = read(&session, batched_tokens);
    assert_eq!(tokens.len(), 3, "one token per sequence");
    assert_eq!(
        tokens[0], one_token[0],
        "the first sequence sampled the same token as when it was alone"
    );
    let keys = read(&session, batched_keys);
    let slot = &keys[0..CAPACITY * COLUMNS];
    assert_eq!(
        slot,
        &one_keys[..],
        "and its whole cache slot is the one it had when it was alone"
    );
    let values = read(&session, batched_values);
    assert_eq!(
        &values[0..COLUMNS],
        &tenfold[0..COLUMNS],
        "the value cache took the value word, which is not the key word"
    );
    assert_ne!(
        tokens[1], tokens[0],
        "a different key is a different token, and the two are distinct keys"
    );
    for resident in [
        single_token,
        single_keys,
        single_values,
        batched_tokens,
        batched_keys,
        batched_values,
    ] {
        session.release(resident);
    }
    println!(
        "one batched step sampled three sequences at one submission, and the first one's answer \
         matched its own run alone"
    );
}

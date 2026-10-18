//! One dispatch appends a token into every sequence's slot of one resident cache.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// The batched append: a token per sequence, written at `position` of that sequence's own slots.
///
/// # Invariant
/// The cache is `sequences × capacity × dim` slots held flat, so a lane is one slot: its sequence is
/// `lane / (capacity × dim)`, its position what is left divided by `dim`, its column the remainder.
/// A lane at the step's `t` takes the new word and every other lane is copied across. The step
/// dispatches twice because a one-node step cannot hand an answer back; the second scales by one.
const APPEND: &str = r#"
--- compute = import "compute.lichen" ---
KVIn   = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
AppIn  = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float)}
KVOut  = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
AppPar = struct{.n Int, .cap Int, .dim Int, .t Int, .in AppIn, .out KVOut}
AppArg = struct{.n Int, .cap Int, .dim Int, .t Int, .I AppIn}
bappend_raw = (k : AppPar) => {
  lane = compute.range k.n
  per_sequence = k.cap * k.dim
  sequence = lane / per_sequence
  rest = lane - sequence * per_sequence
  position = rest / k.dim
  column = rest - position * k.dim
  fresh = position == k.t
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at lane)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at lane)) : Float)
  new_k = if fresh then ((compute.read ((compute.Read _)(.from k.in.nk, .at (sequence * k.dim + column))) : Float)) else old_k
  new_v = if fresh then ((compute.read ((compute.Read _)(.from k.in.nv, .at (sequence * k.dim + column))) : Float)) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value (new_v ~ "element"))))
}
bappend = compute.parallel (bappend_raw ~ "claim:Reduction(element)") "gpu" KVOut
ScalePar = struct{.n Int, .in KVIn, .out KVOut}
ScaleArg = struct{.n Int, .I KVIn}
bscale_raw = (k : ScalePar) => {
  lane = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value (((compute.read ((compute.Read _)(.from k.in.k, .at lane)) : Float) * 1.0) ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value (((compute.read ((compute.Read _)(.from k.in.v, .at lane)) : Float) * 1.0) ~ "element"))))
}
bscale = compute.parallel (bscale_raw ~ "claim:Reduction(element)") "gpu" KVOut
StepIn = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float)}
GArg = struct{.n Int, .in StepIn}
step = (s : GArg) => {
  a = (compute.plrun bappend ((AppArg)(.n 24, .cap 4, .dim 2, .t s.n, .I AppIn(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv))) : KVOut)
  compute.plrun bscale ((ScaleArg)(.n 24, .I KVIn(.k a.k, .v a.v)))
}
built = compute.graph step
built
"#;

/// `values` as the packed bytes a `RunArgument::Buffer` carries.
fn packed(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: packed(values),
    }
}

/// The two residents a step answered with: the key cache and the value cache.
fn caches(stepped: &lichen_serve::Stepped) -> (ResidentBuffer, ResidentBuffer) {
    let [RunResult::Resident(keys), RunResult::Resident(values)] = &stepped.results[..] else {
        panic!("a step answers with the two caches it wrote, and this one answered otherwise");
    };
    (*keys, *values)
}

/// The cache a resident holds, as the floats the graph wrote.
fn read(session: &Session, resident: ResidentBuffer) -> Vec<f32> {
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the cache is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the cache declared a Float buffer, and this is {other:?}"),
    }
}

/// Three sequences fill their own slots of one cache, and no sequence writes another's.
///
/// # Invariant
/// The cache is three sequences of four positions of two columns, so slots 0 to 7 are the first
/// sequence's, 8 to 15 the second's and 16 to 23 the third's. Position `t` of sequence `s` is slots
/// `s × 8 + t × 2` and the one after it, so each step writes two slots per sequence and leaves the
/// rest where they were: the assertion says every written slot and every untouched one.
#[test]
fn one_dispatch_appends_a_token_per_sequence() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED one_dispatch_appends_a_token_per_sequence: no device");
        return;
    };
    let prepared = session
        .prepare(APPEND)
        .unwrap_or_else(|refusal| panic!("the batched append prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the append and the scale"
    );

    let empty = [0.0f32; 24];
    let first = prepared
        .step(vec![
            RunArgument::Count(0),
            floats(&empty),
            floats(&empty),
            floats(&[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]),
            floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    assert_eq!(
        (first.cost.submissions, first.cost.fence_waits),
        (1, 1),
        "three sequences appended in one submission"
    );
    let (keys, values) = caches(&first);
    let written = read(&session, keys);
    assert_eq!(
        [
            written[0],
            written[1],
            written[8],
            written[9],
            written[16],
            written[17]
        ],
        [10.0, 11.0, 12.0, 13.0, 14.0, 15.0],
        "each sequence's first position took its own token"
    );
    assert_eq!(
        [written[2], written[4], written[10], written[18]],
        [0.0; 4],
        "and the positions no sequence reached are still what they were"
    );

    let second = prepared
        .step(vec![
            RunArgument::Count(1),
            RunArgument::Resident(keys),
            RunArgument::Resident(values),
            floats(&[20.0, 21.0, 22.0, 23.0, 24.0, 25.0]),
            floats(&[7.0, 8.0, 9.0, 10.0, 11.0, 12.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));
    assert_eq!(
        (second.cost.submissions, second.cost.fence_waits),
        (1, 1),
        "the second step is one submission too, and none of it uploads the cache"
    );
    let (next_keys, next_values) = caches(&second);
    let written = read(&session, next_keys);
    assert_eq!(
        [written[0], written[1], written[2], written[3]],
        [10.0, 11.0, 20.0, 21.0],
        "the first sequence kept position zero and took position one"
    );
    assert_eq!(
        [written[16], written[17], written[18], written[19]],
        [14.0, 15.0, 24.0, 25.0],
        "and the third sequence did the same in its own slots"
    );
    assert_eq!(
        read(&session, next_values)[0..4],
        [1.0, 2.0, 7.0, 8.0],
        "the value cache followed the key cache"
    );
    for resident in [keys, values, next_keys, next_values] {
        session.release(resident);
    }
    println!("three sequences appended one token each into one cache, at one submission a step");
}

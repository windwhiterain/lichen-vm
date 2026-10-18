//! Each sequence appends at its own position, read from one `Int` buffer, in one dispatch.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// The append at a per-sequence position, so a batch whose sequences are at different points runs.
///
/// # Invariant
/// The positions ride in an `Int` buffer of one word per sequence — the metadata channel a graph's
/// fixed argument arity allows, since the buffer's length follows the batch and its count does not.
/// A lane takes the new word when its own position is the sequence's. The step dispatches twice
/// because a one-node step cannot hand an answer back; the second scales by one.
const APPEND: &str = r#"
--- compute = import "compute.lichen" ---
KVIn   = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
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
ScalePar = struct{.n Int, .in KVIn, .out KVOut}
ScaleArg = struct{.n Int, .I KVIn}
bscale_raw = (k : ScalePar) => {
  lane = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value ((compute.read ((compute.Read _)(.from k.in.k, .at lane)) * 1.0) ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value ((compute.read ((compute.Read _)(.from k.in.v, .at lane)) * 1.0) ~ "element"))))
}
bscale = compute.parallel (bscale_raw ~ "claim:Reduction(element)") "gpu"
StepIn = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
GArg = struct{.n Int, .in StepIn}
step = (s : GArg) => {
  a = (compute.plrun bappend ((AppArg)(.n s.n, .cap 4, .dim 2, .I AppIn(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .pos s.in.pos))) : KVOut)
  compute.plrun bscale ((ScaleArg)(.n s.n, .I KVIn(.k a.k, .v a.v)))
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

/// A `Float` buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: packed(values),
    }
}

/// An `Int` buffer argument of `positions`, one word per sequence.
///
/// # Invariant
/// An `Int` element is **eight** bytes ([`ScalarClass::byte_width`]), so a buffer packed from
/// shorter words would declare fewer elements than it holds and every read past the first would
/// answer zero: the count the runner sees is the byte length divided by the class width.
fn positions(values: &[i64]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Int,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
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

/// Three sequences at three different positions each write their own slot in one dispatch.
///
/// # Invariant
/// The cache is three sequences of four positions of two columns, so sequence `s` owns slots `8s` to
/// `8s + 7` and its position `p` is slots `8s + 2p` and `8s + 2p + 1`. The three sequences are at
/// positions `2`, `0` and `1` — not the same one — so the assertion says each wrote at its own
/// position and left the positions it had not reached alone.
#[test]
fn each_sequence_appends_at_its_own_position() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED each_sequence_appends_at_its_own_position: no device");
        return;
    };
    let prepared = session
        .prepare(APPEND)
        .unwrap_or_else(|refusal| panic!("the per-position append prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the append and the scale"
    );

    let empty = [0.0f32; 24];
    let first = prepared
        .step(vec![
            RunArgument::Count(24),
            floats(&empty),
            floats(&empty),
            floats(&[10.0, 11.0, 12.0, 13.0, 14.0, 15.0]),
            floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]),
            positions(&[2, 0, 1]),
        ])
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    assert_eq!(
        (first.cost.submissions, first.cost.fence_waits),
        (1, 1),
        "three sequences at three positions appended in one submission"
    );
    let (keys, values) = caches(&first);
    let written = read(&session, keys);
    assert_eq!(
        [written[4], written[5]],
        [10.0, 11.0],
        "the first sequence is at position 2, which is its slots 4 and 5"
    );
    assert_eq!(
        [written[8], written[9]],
        [12.0, 13.0],
        "the second sequence is at position 0, which is its slots 0 and 1, told as 8 and 9"
    );
    assert_eq!(
        [written[18], written[19]],
        [14.0, 15.0],
        "and the third is at position 1, its slots 2 and 3, told as 18 and 19"
    );
    assert_eq!(
        [written[0], written[1], written[10], written[11]],
        [0.0; 4],
        "the positions those sequences had not reached are still what they were"
    );
    assert_eq!(
        [read(&session, values)[4], read(&session, values)[8]],
        [1.0, 3.0],
        "the value cache followed the key cache, at each sequence's own position"
    );

    let second = prepared
        .step(vec![
            RunArgument::Count(24),
            RunArgument::Resident(keys),
            RunArgument::Resident(values),
            floats(&[20.0, 21.0, 22.0, 23.0, 24.0, 25.0]),
            floats(&[7.0, 8.0, 9.0, 10.0, 11.0, 12.0]),
            positions(&[3, 1, 2]),
        ])
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));
    let (next_keys, next_values) = caches(&second);
    let written = read(&session, next_keys);
    assert_eq!(
        [written[6], written[7]],
        [20.0, 21.0],
        "the first sequence advanced to position 3 and kept position 2"
    );
    assert_eq!(
        [written[4], written[5]],
        [10.0, 11.0],
        "which is still where it was"
    );
    assert_eq!(
        [written[10], written[11]],
        [22.0, 23.0],
        "the second sequence advanced to position 1 inside its own slots"
    );
    for resident in [keys, values, next_keys, next_values] {
        session.release(resident);
    }
    println!("three sequences at three different positions appended in one dispatch");
}

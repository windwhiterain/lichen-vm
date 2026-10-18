//! The real decode step, prepared once and stepped twice with the cache staying on the device.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// `examples/generate.lichen`'s three kernels unchanged on the device, and the step over them.
///
/// # Invariant
/// The bodies are that file's, with only the backend literal changed, so every expected number below
/// is its hand derivation of steps zero and one rather than a run's output.
const DECODE: &str = r#"
--- compute = import "compute.lichen" ---
KVIn  = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float)}
KVOut = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
ParK  = struct{.n Int, .cap Int, .dim Int, .t Int, .in KVIn, .out KVOut}
ArgK  = struct{.n Int, .cap Int, .dim Int, .t Int, .I KVIn}
append_raw = (k : ParK) => {
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  fresh = position == k.t
  new_k = if fresh then ((compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)) else old_k
  new_v = if fresh then ((compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at slot, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at slot, .value (new_v ~ "element"))))
}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "gpu" KVOut
Gin   = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
Gout  = struct{.y (compute.Buf Float)}
ParS  = struct{.n Int, .kdim Int, .in Gin, .out Gout}
ArgS  = struct{.n Int, .kdim Int, .I Gin}
logits_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (fold (k.kdim, 0.0 ~ "identity"))))
}
logits = compute.parallel (logits_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Gout
Row   = struct{.x (compute.Buf Float)}
Pick  = struct{.p (compute.Buf Float)}
Par   = struct{.n Int, .w Int, .in Row, .out Pick}
Arg   = struct{.n Int, .w Int, .I Row}
greedy_raw = (k : Par) => {
  lane = compute.range k.n
  base = lane * k.w
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float)) ~ "combine"), (s(2) + ((1 * (((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.p, .at lane, .value (int2float (fold (k.w, ((0.0 - 1.0) / 0.0) ~ "identity", 0)))))
}
greedy = compute.parallel (greedy_raw ~ "claim:Reduction(combine,identity)") "gpu" Pick
StepIn = struct{.w (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .nv (compute.Buf Float)}
GArg = struct{.n Int, .in StepIn}
step = (s : GArg) => {
  scored = (compute.plrun logits ((ArgS)(.n 4, .kdim 5, .I Gin(.w s.in.w, .x s.in.k))) : Gout)
  tok = (compute.plrun greedy ((Arg)(.n 1, .w 4, .I Row(.x scored.y))) : Pick)
  compute.plrun append ((ArgK)(.n 5, .cap 5, .dim 1, .t s.n, .I KVIn(.k s.in.k, .v s.in.v, .nk tok.p, .nv s.in.nv)))
}
built = compute.graph step
built
"#;

/// The weights and the three cache buffers `generate.lichen` starts from.
const WEIGHTS: [f32; 20] = [
    2.0, 2.0, -3.0, 3.0, 1.0, 4.0, 2.0, -5.0, 3.0, 2.0, 0.0, 4.0, -6.0, 3.0, 3.0, 5.0, 4.0, -3.0,
    8.0, 4.0,
];

/// Eight `Float` words as the packed bytes a `RunArgument::Buffer` carries.
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

/// The two residents a step answered with, in the source function's order: the key and the value.
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

/// Two decode steps through one prepared graph, with the cache never leaving the device.
///
/// # Invariant
/// Every expected number is `examples/generate.lichen`'s hand derivation: step zero scores
/// `[5, 8, 8, 4]`, samples `2` and writes it into the cache at position `2`; step one samples `0` and
/// writes it at position `3`. The second step's key and value arguments are the residents the first
/// step answered, so the cache is stateful on the device across steps.
#[test]
fn a_decode_step_runs_twice_with_its_cache_on_the_device() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_decode_step_runs_twice_with_its_cache_on_the_device: no device");
        return;
    };
    let prepared = session
        .prepare(DECODE)
        .unwrap_or_else(|refusal| panic!("the decode step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        3,
        "the held step is the score stage, the sampler and the append"
    );

    let empty = [0.0f32; 5];
    let first = prepared
        .step(vec![
            RunArgument::Count(2),
            floats(&WEIGHTS),
            floats(&[1.0, 2.0, -1.0, -1.0, -1.0]),
            floats(&empty),
            floats(&empty),
        ])
        .unwrap_or_else(|refusal| panic!("the first step runs: {refusal}"));
    assert_eq!(
        (first.cost.submissions, first.cost.fence_waits),
        (1, 1),
        "a whole decode step is one submission and one fence"
    );
    let (keys, values) = caches(&first);
    assert_eq!(
        read(&session, keys),
        vec![1.0, 2.0, 2.0, -1.0, -1.0],
        "step zero samples 2 and writes it at position 2, leaving the rest of the key cache alone"
    );
    assert_eq!(
        read(&session, values),
        empty.to_vec(),
        "and the value cache is untouched, because the new value is zero"
    );

    let second = prepared
        .step(vec![
            RunArgument::Count(3),
            floats(&WEIGHTS),
            RunArgument::Resident(keys),
            RunArgument::Resident(values),
            floats(&empty),
        ])
        .unwrap_or_else(|refusal| panic!("the second step runs: {refusal}"));
    assert_eq!(
        (second.cost.submissions, second.cost.fence_waits),
        (1, 1),
        "the second step is one submission too, and none of it uploads the cache"
    );
    let (next_keys, next_values) = caches(&second);
    assert_eq!(
        read(&session, next_keys),
        vec![1.0, 2.0, 2.0, 0.0, -1.0],
        "step one samples 0 and writes it at position 3, which is the derivation generate.lichen \
         states for its second step"
    );
    assert_eq!(
        read(&session, next_values),
        empty.to_vec(),
        "and the value cache is still untouched"
    );
    for resident in [keys, values, next_keys, next_values] {
        session.release(resident);
    }
    println!("two decode steps ran at one submission each, and the cache stayed on the device");
}

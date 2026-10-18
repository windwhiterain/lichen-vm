//! The per-position append every batch test runs, and the readings that decode what a step wrote.
//!
//! # Invariant
//! Shared by more than one test binary, so an item one of them does not use is not dead code.

#![allow(dead_code)]

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// The append at a per-sequence position, so a batch whose sequences are at different points runs.
///
/// # Invariant
/// A lane is one cache slot: its sequence is `lane / (cap × dim)`, its position the next division,
/// its column the remainder. A lane takes the new word when its own position is the sequence's, read
/// from an `Int` buffer of one word per sequence. The step dispatches twice because a one-node step
/// cannot hand an answer back, and the second scales by one so the answer is the append's.
pub const APPEND: &str = r#"
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

/// `values` as the packed bytes a `Float` argument carries.
pub fn packed(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

/// A `Float` buffer argument of `values`.
pub fn floats(values: &[f32]) -> RunArgument {
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
pub fn positions(values: &[i64]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Int,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The two residents a step answered with: the key cache and the value cache.
pub fn caches(stepped: &lichen_serve::Stepped) -> (ResidentBuffer, ResidentBuffer) {
    let [RunResult::Resident(keys), RunResult::Resident(values)] = &stepped.results[..] else {
        panic!("a step answers with the two caches it wrote, and this one answered otherwise");
    };
    (*keys, *values)
}

/// The cache a resident holds, as the floats the graph wrote.
pub fn read(session: &Session, resident: ResidentBuffer) -> Vec<f32> {
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the cache is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the cache declared a Float buffer, and this is {other:?}"),
    }
}

/// `examples/generate.lichen`'s three kernels, each turned inside out over a batch of sequences.
///
/// # Invariant
/// A lane carries its own sequence: the score is `S × rows` lanes and a lane's row is
/// `lane - sequence × rows`, the sampler is one lane per sequence, and the append is one lane per
/// cache slot, so its position comes from the `Int` buffer rather than from a scalar. The step takes
/// three counts because each dispatch's extent is one of them and an extent cannot be computed.
pub const BATCHED: &str = r#"
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
pub const WEIGHTS: [f32; 20] = [
    2.0, 2.0, -3.0, 3.0, 1.0, 4.0, 2.0, -5.0, 3.0, 2.0, 0.0, 4.0, -6.0, 3.0, 3.0, 5.0, 4.0, -3.0,
    8.0, 4.0,
];

/// Five columns, four rows, four positions: the shape `generate.lichen` scores a token with.
pub const COLUMNS: usize = 5;
pub const ROWS: usize = 4;
pub const CAPACITY: usize = 4;

/// The three residents a batched decode step answered with: its token and its two caches.
pub fn returns(stepped: &lichen_serve::Stepped) -> [ResidentBuffer; 3] {
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

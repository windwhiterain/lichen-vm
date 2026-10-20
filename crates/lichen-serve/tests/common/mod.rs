//! The per-position append every batch test runs, and the readings that decode what a step wrote.
//!
//! # Invariant
//! Shared by more than one test binary, so an item one of them does not use is not dead code.

#![allow(dead_code, unused_imports)]

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
  mine = (compute.read ((compute.Read _)(.from k.in.pos, .at sequence)) : Int)
  fresh = position == mine
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

pub use lichen_serve::{CAPACITY, COLUMNS, DECODE_STEP as BATCHED, ROWS, WEIGHTS};

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

//! An attention score over a cache that never leaves the device.

use super::common::{floats, read};
use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

/// The attention score of one query against every slot of its own sequence's cache.
///
/// # Invariant
/// Two dispatches: the first multiplies the cache by its sequence's query, one lane per cache word,
/// and the second folds over a slot's columns. A lane of the first is a `(sequence, position,
/// column)` of the cache; a lane of the second is one `(sequence, position)`. The cache arrives as
/// the **resident** a previous step wrote, so nothing returns to the host.
const ATTEND: &str = r#"
--- compute = import "compute.lichen" ---
Out    = struct{.e (compute.Buf Float)}
WeighIn = struct{.k (compute.Buf Float), .q (compute.Buf Float)}
SumIn  = struct{.t (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
WeighPar = struct{.n Int, .dim Int, .in WeighIn, .out Out}
WeighArg = struct{.n Int, .dim Int, .I WeighIn}
weigh_raw = (k : WeighPar) => {
  i = compute.range k.n
  sequence = i / (3 * k.dim)
  column = i - (i / k.dim) * k.dim
  value = (compute.read ((compute.Read _)(.from k.in.k, .at i)) : Float)
  query = (compute.read ((compute.Read _)(.from k.in.q, .at (sequence * k.dim + column))) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value * query)))
}
weigh = compute.parallel weigh_raw "gpu" Out
SumPar = struct{.n Int, .dim Int, .in SumIn, .out Out}
SumArg = struct{.n Int, .dim Int, .I SumIn}
sum_raw = (k : SumPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.dim + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
sum = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
CopyPar = struct{.n Int, .in CopIn, .out Out}
CopyArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopyPar) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float))))
}
copy = compute.parallel copy_raw "gpu" Out
StepIn = struct{.k (compute.Buf Float), .q (compute.Buf Float)}
GArg = struct{.n Int, .words Int, .in StepIn}
step = (s : GArg) => {
  weighted = (compute.plrun weigh ((WeighArg)(.n s.words, .dim 2, .I WeighIn(.k s.in.k, .q s.in.q))) : Out)
  summed = (compute.plrun sum ((SumArg)(.n s.n, .dim 2, .I SumIn(.t weighted.e))) : Out)
  compute.plrun copy ((compute.A CopIn)(.n s.n, .I CopIn(.d summed.e)))
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

/// A score is its query dotted with its own sequence's cache, and the cache is a resident.
///
/// # Invariant
/// Two sequences of three positions of two columns. Sequence zero's cache is `1,0`, `0,1`, `1,1`
/// against the query `1,1`, which scores `1, 1, 2`; sequence one's is `2,0`, `0,2`, `1,-1` against
/// `1,0`, which scores `2, 0, 1`. A score that read the other sequence's cache would not be these.
#[test]
fn a_score_reads_its_own_sequences_cache_on_the_device() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_score_reads_its_own_sequences_cache_on_the_device: no device");
        return;
    };
    let cached = session
        .upload(&[1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0, 0.0, 2.0, 1.0, -1.0])
        .unwrap_or_else(|refusal| panic!("the cache loads: {refusal}"));
    let prepared = session
        .prepare(ATTEND)
        .unwrap_or_else(|refusal| panic!("the attention step prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        3,
        "the held step weighs the cache, sums it and copies it"
    );
    let stepped = prepared
        .step(vec![
            RunArgument::Count(6),
            RunArgument::Count(12),
            RunArgument::Resident(cached),
            floats(&[1.0, 1.0, 1.0, 0.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the attention step runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "a score over a resident cache is one submission"
    );
    let answered = only(&stepped);
    assert_eq!(
        read(&session, answered),
        vec![1.0, 1.0, 2.0, 2.0, 0.0, 1.0],
        "each query dotted with its own sequence's three slots"
    );
    session.release(answered);
    session.release(cached);
    println!("a score read a resident cache and answered six dot products in one submission");
}

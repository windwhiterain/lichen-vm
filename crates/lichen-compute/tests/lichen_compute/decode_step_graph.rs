//! A whole decode step as one recorded graph, and two of them chained in the language.
//!
//! # Invariant
//! The step is `examples/generate.lichen`'s three kernels unchanged and on the device: the
//! score GEMV, the greedy sampler whose fold carries an `Int` slot beside two `Float` ones, and
//! the cache append. All three record into **one** graph. Every expected number is that file's
//! hand derivation of steps zero and one, never a run's output.

use super::common;

use lichen_compute::language::new_store;

/// The three kernels, `examples/generate.lichen`'s bodies, with only the backend literal changed.
const KERNEL: &str = r#"
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
weights = (compute.buffer ([2.0, 2.0, (0.0 - 3.0), 3.0, 1.0, 4.0, 2.0, (0.0 - 5.0), 3.0, 2.0, 0.0, 4.0, (0.0 - 6.0), 3.0, 3.0, 5.0, 4.0, (0.0 - 3.0), 8.0, 4.0] : array{Float, 20}) : (compute.Buf Float))
kbuf = (compute.buffer ([1.0, 2.0, (0.0 - 1.0), (0.0 - 1.0), (0.0 - 1.0)] : array{Float, 5}) : (compute.Buf Float))
vbuf = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 5}) : (compute.Buf Float))
nv   = (compute.buffer ([0.0, 0.0, 0.0, 0.0, 0.0] : array{Float, 5}) : (compute.Buf Float))
"#;

/// The step's declarations, shared by every body below: the position is the extent argument.
const STEP: &str = r#"
StepIn = struct{.w (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .nv (compute.Buf Float)}
GArg = struct{.n Int, .in StepIn}
step = (s : GArg) => {
  scored = (compute.plrun logits ((ArgS)(.n 4, .kdim 5, .I Gin(.w s.in.w, .x s.in.k))) : Gout)
  tok = (compute.plrun greedy ((Arg)(.n 1, .w 4, .I Row(.x scored.y))) : Pick)
  compute.plrun append ((ArgK)(.n 5, .cap 5, .dim 1, .t s.n, .I KVIn(.k s.in.k, .v s.in.v, .nk tok.p, .nv s.in.nv)))
}
built = compute.graph step
"#;

/// The step as three dispatches, which is the shape `examples/generate.lichen` performs.
const DIRECT: &str = r#"
scored = (compute.plrun logits ((ArgS)(.n 4, .kdim 5, .I Gin(.w weights, .x kbuf))) : Gout)
tok = (compute.plrun greedy ((Arg)(.n 1, .w 4, .I Row(.x scored.y))) : Pick)
cached = (compute.plrun append ((ArgK)(.n 5, .cap 5, .dim 1, .t 2, .I KVIn(.k kbuf, .v vbuf, .nk tok.p, .nv nv))) : KVOut)
(compute.collect scored.y : array{Float, 4}, compute.collect tok.p : array{Float, 1}, compute.collect cached.k : array{Float, 5})
"#;

/// One step as one graph: step zero, the cache after token `2` landed at position `2`.
const ONE_STEP: &str = r#"
compute.collect (compute.graphrun built (2, weights, kbuf, vbuf, nv)) : array{Float, 5}
"#;

/// Two steps chained in the language: step one samples `0` and writes it at position `3`.
const TWO_STEPS: &str = r#"
first = (compute.graphrun built (2, weights, kbuf, vbuf, nv) : array{(compute.Buf Float), 1})
compute.collect (compute.graphrun built (3, weights, first[0], vbuf, nv)) : array{Float, 5}
"#;

/// What the runner answered for `body`, with the type tail dropped so a class defect is not pinned.
fn numbers(body: &str) -> String {
    let source = format!("{KERNEL}{STEP}{body}");
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
        Ok(text) => text
            .rsplit_once(": ")
            .map_or(text.clone(), |(value, _)| value.to_string()),
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

/// The three dispatches answer step zero, which is what the graph legs are measured against.
#[test]
fn the_direct_chain_answers_the_hand_derivation() {
    let (_stub, _guard) = common::stub();
    assert_eq!(
        numbers(DIRECT),
        "([5.0, 8.0, 8.0, 4.0], [2.0], [1.0, 2.0, 2.0, -1.0, -1.0])",
        "the score row, the sampled token and the appended cache, as generate.lichen derives them"
    );
}

/// All three kernels record into one graph and answer the same cache.
#[test]
fn the_whole_step_records_into_one_graph() {
    let (_stub, _guard) = common::stub();
    assert_eq!(
        numbers(ONE_STEP),
        "[1.0, 2.0, 2.0, -1.0, -1.0]",
        "one graphrun carries the score, the sample and the append, so a step is one submission"
    );
}

/// The second graphrun reads the first's answer, so a loop needs no host round trip.
#[test]
fn a_second_step_reads_the_first_ones_answer() {
    let (_stub, _guard) = common::stub();
    assert_eq!(
        numbers(TWO_STEPS),
        "[1.0, 2.0, 2.0, 0.0, -1.0]",
        "step one samples 0 and writes it at position 3, which is generate.lichen's second step"
    );
}

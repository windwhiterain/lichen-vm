//! The toy model the demo serves and the tests measure: four rows of five columns, four positions.

/// A whole decode step over a batch: score, sample and append, each turned out over the sequences.
///
/// # Invariant
/// A lane carries its own sequence: the score is `sequences × rows` lanes and a lane's row is
/// `lane - sequence × rows`, the sampler is one lane per sequence, and the append is one lane per
/// cache slot, so its position comes from the `Int` buffer rather than from a scalar. The step takes
/// three counts because each dispatch's extent is one of them and an extent cannot be computed.
pub const DECODE_STEP: &str = r#"
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

/// How many words a position holds, and how many positions a sequence may reach.
pub const COLUMNS: usize = 5;
pub const CAPACITY: usize = 4;

/// How many rows the score spans, which is the vocabulary the sampler picks from.
pub const ROWS: usize = 4;

/// The score's coefficients, `examples/generate.lichen`'s own four rows of five.
pub const WEIGHTS: [f32; 20] = [
    2.0, 2.0, -3.0, 3.0, 1.0, 4.0, 2.0, -5.0, 3.0, 2.0, 0.0, 4.0, -6.0, 3.0, 3.0, 5.0, 4.0, -3.0,
    8.0, 4.0,
];

/// One row per token, so a sampled token is the key the next step appends.
///
/// # Invariant
/// The rows are one-hot, so two different tokens are two different keys and a demo can tell them
/// apart without the sampler's numbers having to be derived.
pub const EMBEDDING: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    1.0,
];

/// The key a token appends, which is `token`'s row of the table.
pub fn embedding_of(token: f32) -> [f32; COLUMNS] {
    let row = (token as usize) % ROWS;
    let mut word = [0.0f32; COLUMNS];
    word.copy_from_slice(&EMBEDDING[row * COLUMNS..(row + 1) * COLUMNS]);
    word
}

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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (sequence * k.dim + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu" ScoreOut
PickIn  = struct{.y (compute.Buf Float)}
Pick    = struct{.p (compute.Buf Float)}
PickPar = struct{.n Int, .rows Int, .in PickIn, .out Pick}
PickArg = struct{.n Int, .rows Int, .I PickIn}
greedy_raw = (k : PickPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.y, .at (base + s(0) - 1))) : Float)) ~ "combine"), (s(2) + ((1 * (((compute.read ((compute.Read _)(.from k.in.y, .at (base + s(0) - 1))) : Float) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.p, .at lane, .value (int2float (fold (k.rows, ((0.0 - 1.0) / 0.0) ~ "identity", 0)))))
}
greedy = compute.parallel (greedy_raw ~ "claim:Reduction(combine,identity)") "gpu" Pick
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

/// The block a real model repeats, over whatever width its host passes as `dim`.
///
/// # Invariant
/// Twelve kernels because a fold's body reads one buffer: every stage that multiplies two values does
/// it elementwise and binds both, so each fold sums a single buffer. The scale writes transposed and
/// the feed-forward spreads before it projects, which keeps both folds contiguous. Every extent is one
/// of the six counts, so **one source serves a model of any width**.
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
  value = (compute.read ((compute.Read _)(.from k.in.k, .at i)) : Float)
  query = (compute.read ((compute.Read _)(.from k.in.q, .at (sequence * k.dim + column))) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value * query)))
}
weigh = compute.parallel weigh_raw "gpu" Out
SumPar = struct{.n Int, .cap Int, .dim Int, .in TallyIn, .out Out}
SumArg = struct{.n Int, .cap Int, .dim Int, .I TallyIn}
sum_raw = (k : SumPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.dim + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
sum = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
PeakPar = struct{.n Int, .cap Int, .in PeakIn, .out Out}
PeakArg = struct{.n Int, .cap Int, .I PeakIn}
peak_raw = (k : PeakPar) => {
  lane = compute.range k.n
  base = lane * k.cap
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, ((0.0 - 1.0) / 0.0) ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
MaskIn  = struct{.s (compute.Buf Float), .length (compute.Buf Int)}
MaskPar = struct{.n Int, .cap Int, .in MaskIn, .out Out}
MaskArg = struct{.n Int, .cap Int, .I MaskIn}
mask_raw = (k : MaskPar) => {
  lane = compute.range k.n
  sequence = lane / k.cap
  position = lane - sequence * k.cap
  limit = (compute.read ((compute.Read _)(.from k.in.length, .at sequence)) : Int)
  visible = position < limit
  value = (compute.read ((compute.Read _)(.from k.in.s, .at lane)) : Float)
  hidden = (0.0 - 1.0) / 0.0
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (if visible then value else hidden)))
}
mask = compute.parallel mask_raw "gpu" Out
ExpoPar = struct{.n Int, .cap Int, .in ExpoIn, .out Out}
ExpoArg = struct{.n Int, .cap Int, .I ExpoIn}
expo_raw = (k : ExpoPar) => {
  i = compute.range k.n
  sequence = i / k.cap
  value = (compute.read ((compute.Read _)(.from k.in.s, .at i)) : Float)
  top = (compute.read ((compute.Read _)(.from k.in.m, .at sequence)) : Float)
  shifted = value - top
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (exp shifted)))
}
expo = compute.parallel expo_raw "gpu" Out
TotalPar = struct{.n Int, .cap Int, .in TallyIn, .out Out}
TotalArg = struct{.n Int, .cap Int, .I TallyIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.cap + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
ScalePar = struct{.n Int, .cap Int, .dim Int, .in ScaleIn, .out Out}
ScaleArg = struct{.n Int, .cap Int, .dim Int, .I ScaleIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  sequence = i / (k.cap * k.dim)
  column = i - (i / k.dim) * k.dim
  position = (i / k.dim) - sequence * k.cap
  weight = (compute.read ((compute.Read _)(.from k.in.s, .at (sequence * k.cap + position))) : Float)
  divisor = (compute.read ((compute.Read _)(.from k.in.total, .at sequence)) : Float)
  value = (compute.read ((compute.Read _)(.from k.in.v, .at i)) : Float)
  share = weight / divisor
  compute.write ((compute.Write _)(.to k.out.e, .at ((sequence * k.dim + column) * k.cap + position), .value (share * value)))
}
scale = compute.parallel scale_raw "gpu" Out
PosPar = struct{.n Int, .cap Int, .in PosIn, .out Out}
PosArg = struct{.n Int, .cap Int, .I PosIn}
pos_raw = (k : PosPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.cap + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
positions = compute.parallel (pos_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
AddPar = struct{.n Int, .in AddIn, .out Out}
AddArg = struct{.n Int, .I AddIn}
add_raw = (k : AddPar) => {
  i = compute.range k.n
  left = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : Float)
  right = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (left + right)))
}
add = compute.parallel add_raw "gpu" Out
MixPar = struct{.n Int, .dim Int, .in MixIn, .out Out}
MixArg = struct{.n Int, .dim Int, .I MixIn}
mix_raw = (k : MixPar) => {
  i = compute.range k.n
  sequence = i / k.dim
  value = (compute.read ((compute.Read _)(.from k.in.h, .at i)) : Float)
  summary = (compute.read ((compute.Read _)(.from k.in.m, .at sequence)) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value + summary)))
}
mix = compute.parallel mix_raw "gpu" Out
SpreadPar = struct{.n Int, .dim Int, .in SpreadIn, .out Out}
SpreadArg = struct{.n Int, .dim Int, .I SpreadIn}
spread_raw = (k : SpreadPar) => {
  i = compute.range k.n
  sequence = i / (k.dim * k.dim)
  rest = i - sequence * (k.dim * k.dim)
  column = rest / k.dim
  inner = rest - column * k.dim
  hidden = (compute.read ((compute.Read _)(.from k.in.n, .at (sequence * k.dim + inner))) : Float)
  weight = (compute.read ((compute.Read _)(.from k.in.w, .at (column * k.dim + inner))) : Float)
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (hidden * weight)))
}
spread = compute.parallel spread_raw "gpu" Out
ProjectPar = struct{.n Int, .dim Int, .in PosIn, .out Out}
ProjectArg = struct{.n Int, .dim Int, .I PosIn}
project_raw = (k : ProjectPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.dim + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
project = compute.parallel (project_raw ~ "claim:Reduction(combine,identity)") "gpu" Out
StepIn = struct{.x (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .q (compute.Buf Float), .w (compute.Buf Float), .length (compute.Buf Int)}
GArg = struct{.words Int, .slots Int, .sequences Int, .columns Int, .spread Int, .dim Int, .in StepIn}
step = (s : GArg) => {
  weighted = (compute.plrun weigh ((WeighArg)(.n s.words, .cap 3, .dim s.dim, .I WeighIn(.k s.in.k, .q s.in.q))) : Out)
  summed = (compute.plrun sum ((SumArg)(.n s.slots, .cap 3, .dim s.dim, .I TallyIn(.t weighted.e))) : Out)
  masked = (compute.plrun mask ((MaskArg)(.n s.slots, .cap 3, .I MaskIn(.s summed.e, .length s.in.length))) : Out)
  topped = (compute.plrun peak ((PeakArg)(.n s.sequences, .cap 3, .I PeakIn(.s masked.e))) : Out)
  shifted = (compute.plrun expo ((ExpoArg)(.n s.slots, .cap 3, .I ExpoIn(.s masked.e, .m topped.e))) : Out)
  divided = (compute.plrun total ((TotalArg)(.n s.sequences, .cap 3, .I TallyIn(.t shifted.e))) : Out)
  shared = (compute.plrun scale ((ScaleArg)(.n s.words, .cap 3, .dim s.dim, .I ScaleIn(.s shifted.e, .m topped.e, .total divided.e, .v s.in.v))) : Out)
  attended = (compute.plrun positions ((PosArg)(.n s.columns, .cap 3, .I PosIn(.w shared.e))) : Out)
  held = (compute.plrun add ((AddArg)(.n s.columns, .I AddIn(.a s.in.x, .b attended.e))) : Out)
  summarised = (compute.plrun sum ((SumArg)(.n s.sequences, .cap 3, .dim s.dim, .I TallyIn(.t held.e))) : Out)
  normalised = (compute.plrun mix ((MixArg)(.n s.columns, .dim s.dim, .I MixIn(.h held.e, .m summarised.e))) : Out)
  spread_out = (compute.plrun spread ((SpreadArg)(.n s.spread, .dim s.dim, .I SpreadIn(.n normalised.e, .w s.in.w))) : Out)
  compute.plrun project ((ProjectArg)(.n s.columns, .dim s.dim, .I PosIn(.w spread_out.e)))
}
built = compute.graph step
built
"#;

/// How many blocks one token passes through, which is the depth the demo serves.
pub const BLOCKS: usize = 2;

/// Each layer's query, one row of five per sequence, so the layers differ.
pub const QUERIES: [[f32; 10]; BLOCKS] = [
    [1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0],
    [0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0],
];

/// Each layer's feed-forward projection, five rows of five.
///
/// # Invariant
/// The layers' tables differ, so a state the second layer's weights produced is not what the first
/// layer's would have. The key and value caches are shared by the layers instead, which is the demo's
/// simplification: a model gives each layer its own and nothing in the path changes.
pub const PROJECTIONS: [[f32; 25]; BLOCKS] = [
    [
        1.0, 2.0, 0.0, 0.0, 0.0, 3.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 3.0,
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 4.0,
    ],
    [
        0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0,
        0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0,
    ],
];

/// A whole decode step that samples instead of taking the largest, all on the device.
///
/// # Invariant
/// Seven kernels: score, peak, the exponent at the host's temperature, total, shares, pick, append. The
/// pick walks the rows from the highest down, keeping the lowest index whose cumulative probability
/// still exceeds a threshold made from `sin` of the seed and the sequence, so a token follows from its
/// seed and no random number reaches the host.
pub const SAMPLE_STEP: &str = r#"
--- compute = import "compute.lichen" ---
ScoreIn  = struct{.w (compute.Buf Float), .x (compute.Buf Float)}
ScoreOut = struct{.y (compute.Buf Float)}
ScorePar = struct{.n Int, .rows Int, .dim Int, .in ScoreIn, .out ScoreOut}
ScoreArg = struct{.n Int, .rows Int, .dim Int, .I ScoreIn}
score_raw = (k : ScorePar) => {
  lane = compute.range k.n
  sequence = lane / k.rows
  row = lane - sequence * k.rows
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (sequence * k.dim + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu" ScoreOut
PeakIn  = struct{.s (compute.Buf Float)}
PeakPar = struct{.n Int, .rows Int, .in PeakIn, .out ScoreOut}
PeakArg = struct{.n Int, .rows Int, .I PeakIn}
peak_raw = (k : PeakPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.rows, ((0.0 - 1.0) / 0.0) ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity)") "gpu" ScoreOut
ExpoIn  = struct{.s (compute.Buf Float), .m (compute.Buf Float)}
ExpoPar = struct{.n Int, .rows Int, .temp Float, .in ExpoIn, .out ScoreOut}
ExpoArg = struct{.n Int, .rows Int, .temp Float, .I ExpoIn}
expo_raw = (k : ExpoPar) => {
  i = compute.range k.n
  sequence = i / k.rows
  value = (compute.read ((compute.Read _)(.from k.in.s, .at i)) : Float)
  top = (compute.read ((compute.Read _)(.from k.in.m, .at sequence)) : Float)
  shifted = (value - top) / k.temp
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (exp shifted)))
}
expo = compute.parallel expo_raw "gpu" ScoreOut
TotalPar = struct{.n Int, .rows Int, .in PeakIn, .out ScoreOut}
TotalArg = struct{.n Int, .rows Int, .I PeakIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _)(.from k.in.s, .at (lane * k.rows + (s(0) - 1)))) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.rows, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu" ScoreOut
SharePar = struct{.n Int, .rows Int, .in ExpoIn, .out ScoreOut}
ShareArg = struct{.n Int, .rows Int, .I ExpoIn}
share_raw = (k : SharePar) => {
  i = compute.range k.n
  sequence = i / k.rows
  weight = (compute.read ((compute.Read _)(.from k.in.s, .at i)) : Float)
  divisor = (compute.read ((compute.Read _)(.from k.in.m, .at sequence)) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (weight / divisor)))
}
share = compute.parallel share_raw "gpu" ScoreOut
Pick     = struct{.p (compute.Buf Float)}
PickPar  = struct{.n Int, .rows Int, .seed Float, .in PeakIn, .out Pick}
PickArg  = struct{.n Int, .rows Int, .seed Float, .I PeakIn}
pick_raw = (k : PickPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  drawn_lane = int2float lane
  noise = sin ((k.seed * 12.9898) + (drawn_lane * 78.233))
  drawn = (noise * 43758.545) - floor (noise * 43758.545)
  @loop fold = s => if s(0) == 0 then s(3) else fold (s(0) - 1, s(1), (s(2) + ((compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1))) : Float))) ~ "combine", (s(3) + ((1 * (s(2) <= (1.0 - s(1)))) * ((s(0) - 1) - s(3)))) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.p, .at lane, .value (int2float (fold (k.rows, drawn, 0.0 ~ "identity", 0)))))
}
pick = compute.parallel (pick_raw ~ "claim:Reduction(combine,identity)") "gpu" Pick
AppIn   = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
KVOut   = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
AppPar  = struct{.n Int, .cap Int, .dim Int, .in AppIn, .out KVOut}
AppArg  = struct{.n Int, .cap Int, .dim Int, .I AppIn}
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
StepIn = struct{.w (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
GArg = struct{.n Int, .lanes Int, .cells Int, .temp Float, .seed Float, .in StepIn}
step = (s : GArg) => {
  scored = (compute.plrun score ((ScoreArg)(.n s.lanes, .rows 4, .dim 5, .I ScoreIn(.w s.in.w, .x s.in.nk))) : ScoreOut)
  topped = (compute.plrun peak ((PeakArg)(.n s.n, .rows 4, .I PeakIn(.s scored.y))) : ScoreOut)
  shifted = (compute.plrun expo ((ExpoArg)(.n s.lanes, .rows 4, .temp s.temp, .I ExpoIn(.s scored.y, .m topped.y))) : ScoreOut)
  divided = (compute.plrun total ((TotalArg)(.n s.n, .rows 4, .I PeakIn(.s shifted.y))) : ScoreOut)
  shared = (compute.plrun share ((ShareArg)(.n s.lanes, .rows 4, .I ExpoIn(.s shifted.y, .m divided.y))) : ScoreOut)
  tok = (compute.plrun pick ((PickArg)(.n s.n, .rows 4, .seed s.seed, .I PeakIn(.s shared.y))) : Pick)
  cached = (compute.plrun bappend ((AppArg)(.n s.cells, .cap 4, .dim 5, .I AppIn(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .pos s.in.pos))) : KVOut)
  (tok.p, cached.k, cached.v)
}
built = compute.graph step
built
"#;

/// A step that appends a **run** of positions at once, which is what a prompt costs.
///
/// # Invariant
/// A lane is one cache slot and takes the new word when its position falls in
/// `[first, first + count)`, reading it from the run at `(sequence × count + offset) × dim + column`. A
/// count of one is the decode append and a count of the prompt fills it whole in one submission. The
/// second dispatch fills nothing, because a one-node step cannot hand an answer back.
pub const PREFILL_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
KVOut  = struct{.k (compute.Buf Float), .v (compute.Buf Float)}
FillPar = struct{.n Int, .cap Int, .dim Int, .count Int, .in In, .out KVOut}
FillArg = struct{.n Int, .cap Int, .dim Int, .count Int, .I In}
fill_raw = (k : FillPar) => {
  lane = compute.range k.n
  per_sequence = k.cap * k.dim
  sequence = lane / per_sequence
  rest = lane - sequence * per_sequence
  position = rest / k.dim
  column = rest - position * k.dim
  first = (compute.read ((compute.Read _)(.from k.in.pos, .at sequence)) : Int)
  after = 1 * (position >= first)
  before = 1 * (position < (first + k.count))
  fresh = (after * before) == 1
  slot = (sequence * k.count + (position - first)) * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at lane)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at lane)) : Float)
  new_k = if fresh then ((compute.read ((compute.Read _)(.from k.in.nk, .at slot)) : Float)) else old_k
  new_v = if fresh then ((compute.read ((compute.Read _)(.from k.in.nv, .at slot)) : Float)) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value (new_v ~ "element"))))
}
fill = compute.parallel (fill_raw ~ "claim:Reduction(element)") "gpu" KVOut
StepIn = struct{.k (compute.Buf Float), .v (compute.Buf Float), .nk (compute.Buf Float), .nv (compute.Buf Float), .pos (compute.Buf Int)}
GArg = struct{.n Int, .count Int, .in StepIn}
step = (s : GArg) => {
  a = (compute.plrun fill ((FillArg)(.n s.n, .cap 4, .dim 5, .count s.count, .I In(.k s.in.k, .v s.in.v, .nk s.in.nk, .nv s.in.nv, .pos s.in.pos))) : KVOut)
  compute.plrun fill ((FillArg)(.n s.n, .cap 4, .dim 5, .count 0, .I In(.k a.k, .v a.v, .nk s.in.nk, .nv s.in.nv, .pos s.in.pos)))
}
built = compute.graph step
built
"#;

/// A real width: the hidden size every layer of Qwen3-1.7B computes at.
pub const HIDDEN: usize = 2048;

/// Qwen3's RMSNorm: two kernels, because one lane cannot reduce a row and scale it at once.
///
/// # Invariant
/// The first kernel is a single lane folding the row's squares into one word, and the second is one
/// lane per element dividing by the root of that mean. The sum is the row's and the scale is the
/// element's, so the two are separate dispatches over the same two buffers.
pub const RMS_NORM_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In      = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
Squared = struct{.s (compute.Buf Float)}
Normed  = struct{.y (compute.Buf Float)}
SquarePar = struct{.n Int, .dim Int, .in In, .out Squared}
SquareArg = struct{.n Int, .dim Int, .I In}
squares_raw = (k : SquarePar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
squares = compute.parallel (squares_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Squared
ScaleIn  = struct{.s (compute.Buf Float), .x (compute.Buf Float), .w (compute.Buf Float)}
ScalePar = struct{.n Int, .dim Int, .in ScaleIn, .out Normed}
ScaleArg = struct{.n Int, .dim Int, .I ScaleIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  head = i / k.dim
  j = i - head * k.dim
  total = (compute.read ((compute.Read _)(.from k.in.s, .at head)) : Float)
  mean = total / (int2float k.dim)
  divisor = sqrt (mean + 0.000001)
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  weight = (compute.read ((compute.Read _)(.from k.in.w, .at j)) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((value * weight) / divisor)))
}
scale = compute.parallel scale_raw "gpu" Normed
GArg = struct{.n Int, .rows Int, .dim Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun squares ((SquareArg)(.n s.rows, .dim s.dim, .I In(.x s.in.x, .w s.in.w))) : Squared)
  compute.plrun scale ((ScaleArg)(.n s.n, .dim s.dim, .I ScaleIn(.s a.s, .x s.in.x, .w s.in.w)))
}
built = compute.graph step
built
"#;

/// One row of a projection: `y[j] = sum over i of x[i] · w[j][i]`, at the model's own width.
///
/// # Invariant
/// A lane is one row of `w` and folds the whole width, so the kernel is the shape every projection
/// in a layer has — `q`, `k`, `v`, `o`, `gate`, `up`, `down`, and the 151936-wide vocabulary — with
/// the row count and the width the caller's. The second dispatch copies the rows across, because a
/// one-node step cannot hand its answer back.
pub const MATVEC_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
Rows   = struct{.y (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
MatvecPar = struct{.n Int, .dim Int, .in In, .out Rows}
MatvecArg = struct{.n Int, .dim Int, .I In}
matvec_raw = (k : MatvecPar) => {
  row = compute.range k.n
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at row, .value (fold (k.dim, 0.0 ~ "identity"))))
}
matvec = compute.parallel (matvec_raw ~ "claim:Reduction(combine,identity)") "gpu" Rows
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.n Int, .dim Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun matvec ((MatvecArg)(.n s.n, .dim s.dim, .I In(.x s.in.x, .w s.in.w))) : Rows)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#;

/// Qwen3's rotation: `head_dim` 128, half of it turned, `theta` one million, the rotate-half form.
///
/// # Invariant
/// A lane is one element of one head and reads its partner at `i ± 64`, which stays inside the head
/// because the head's own offset cancels: the first half pairs forward and subtracts, the second pairs
/// back and adds. The angle is the position times `theta` raised to `-j/64`, which the language's own
/// `exp` and `ln` build, so no table reaches the device.
pub const ROPE_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.x (compute.Buf Float)}
Out    = struct{.y (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
RopePar = struct{.n Int, .position Int, .in In, .out Out}
RopeArg = struct{.n Int, .position Int, .I In}
rope_raw = (k : RopePar) => {
  i = compute.range k.n
  head = i / 128
  j = i - head * 128
  low = j < 64
  other_index = if low then (i + 64) else (i - 64)
  own = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  other = (compute.read ((compute.Read _)(.from k.in.x, .at other_index)) : Float)
  turned = if low then j else (j - 64)
  exponent = (0.0 - (int2float turned)) / 64.0
  scale = exp (exponent * (ln (1000000.0)))
  angle = (int2float k.position) * scale
  cosine = cos angle
  sine = sin angle
  signed = if low then (0.0 - 1.0) else 1.0
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((own * cosine) + ((signed * other) * sine))))
}
rope = compute.parallel rope_raw "gpu" Out
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.n Int, .position Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun rope ((RopeArg)(.n s.n, .position s.position, .I In(.x s.in.x))) : Out)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#;

/// SwiGLU's product: `silu(gate) · up`, which is the elementwise half of the feed-forward.
///
/// # Invariant
/// `silu(x) = x / (1 + exp(-x))`, and the two inputs are the two projections' answers over the same
/// element, so a lane reads both at its own index. The second dispatch copies across, because a
/// one-node step cannot hand its answer back.
pub const SWIGLU_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.g (compute.Buf Float), .u (compute.Buf Float)}
Out    = struct{.m (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
GluePar = struct{.n Int, .in In, .out Out}
GlueArg = struct{.n Int, .I In}
glue_raw = (k : GluePar) => {
  i = compute.range k.n
  gate = (compute.read ((compute.Read _)(.from k.in.g, .at i)) : Float)
  up = (compute.read ((compute.Read _)(.from k.in.u, .at i)) : Float)
  open = gate / (1.0 + (exp (0.0 - gate)))
  compute.write ((compute.Write _)(.to k.out.m, .at i, .value (open * up)))
}
glue = compute.parallel glue_raw "gpu" Out
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun glue ((GlueArg)(.n s.n, .I In(.g s.in.g, .u s.in.u))) : Out)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.m)))
}
built = compute.graph step
built
"#;

/// Qwen3-1.7B's own layer, one token: twenty-one dispatches from a hidden state to the next.
///
/// # Invariant
/// The order is the model's — normalise, project the query, key and value, normalise each head of the
/// two that are normalised, rotate both at the token's position, spread the eight keys' values over the
/// sixteen queries, project out, add the residual, normalise, project the gate and the up, multiply
/// them through the sigmoid, project down and add the second residual.
pub const LAYER_STEP: &str = r#"
--- compute = import "compute.lichen" ---
SqIn   = struct{.x (compute.Buf Float)}
SqOut  = struct{.s (compute.Buf Float)}
SqPar  = struct{.n Int, .dim Int, .in SqIn, .out SqOut}
SqArg  = struct{.n Int, .dim Int, .I SqIn}
squares_raw = (k : SqPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
squares = compute.parallel (squares_raw ~ "claim:Reduction(combine,identity,element)") "gpu" SqOut
ScIn   = struct{.s (compute.Buf Float), .x (compute.Buf Float), .w (compute.Buf Float)}
ScOut  = struct{.y (compute.Buf Float)}
ScPar  = struct{.n Int, .dim Int, .in ScIn, .out ScOut}
ScArg  = struct{.n Int, .dim Int, .I ScIn}
scale_raw = (k : ScPar) => {
  i = compute.range k.n
  head = i / k.dim
  j = i - head * k.dim
  total = (compute.read ((compute.Read _)(.from k.in.s, .at head)) : Float)
  mean = total / (int2float k.dim)
  divisor = sqrt (mean + 0.000001)
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  weight = (compute.read ((compute.Read _)(.from k.in.w, .at j)) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((value * weight) / divisor)))
}
scale = compute.parallel scale_raw "gpu" ScOut
MvIn   = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
MvOut  = struct{.y (compute.Buf Float)}
MvPar  = struct{.n Int, .dim Int, .in MvIn, .out MvOut}
MvArg  = struct{.n Int, .dim Int, .I MvIn}
matvec_raw = (k : MvPar) => {
  row = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at row, .value (fold (k.dim, 0.0 ~ "identity"))))
}
matvec = compute.parallel (matvec_raw ~ "claim:Reduction(combine,identity)") "gpu" MvOut
RpIn   = struct{.x (compute.Buf Float)}
RpOut  = struct{.y (compute.Buf Float)}
RpPar  = struct{.n Int, .position Int, .in RpIn, .out RpOut}
RpArg  = struct{.n Int, .position Int, .I RpIn}
rope_raw = (k : RpPar) => {
  i = compute.range k.n
  head = i / 128
  j = i - head * 128
  low = j < 64
  other_index = if low then (i + 64) else (i - 64)
  own = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  other = (compute.read ((compute.Read _)(.from k.in.x, .at other_index)) : Float)
  turned = if low then j else (j - 64)
  exponent = (0.0 - (int2float turned)) / 64.0
  factor = exp (exponent * (ln (1000000.0)))
  angle = (int2float k.position) * factor
  cosine = cos angle
  sine = sin angle
  signed = if low then (0.0 - 1.0) else 1.0
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((own * cosine) + ((signed * other) * sine))))
}
rope = compute.parallel rope_raw "gpu" RpOut
SpIn   = struct{.v (compute.Buf Float)}
SpOut  = struct{.y (compute.Buf Float)}
SpPar  = struct{.n Int, .in SpIn, .out SpOut}
SpArg  = struct{.n Int, .I SpIn}
spread_raw = (k : SpPar) => {
  i = compute.range k.n
  head = i / 128
  key = head / 2
  j = i - head * 128
  value = (compute.read ((compute.Read _)(.from k.in.v, .at (key * 128 + j))) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value value))
}
spread = compute.parallel spread_raw "gpu" SpOut
AdIn   = struct{.a (compute.Buf Float), .b (compute.Buf Float)}
AdOut  = struct{.y (compute.Buf Float)}
AdPar  = struct{.n Int, .in AdIn, .out AdOut}
AdArg  = struct{.n Int, .I AdIn}
add_raw = (k : AdPar) => {
  i = compute.range k.n
  left = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : Float)
  right = (compute.read ((compute.Read _)(.from k.in.b, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (left + right)))
}
add = compute.parallel add_raw "gpu" AdOut
GlIn   = struct{.g (compute.Buf Float), .u (compute.Buf Float)}
GlOut  = struct{.m (compute.Buf Float)}
GlPar  = struct{.n Int, .in GlIn, .out GlOut}
GlArg  = struct{.n Int, .I GlIn}
glue_raw = (k : GlPar) => {
  i = compute.range k.n
  gate = (compute.read ((compute.Read _)(.from k.in.g, .at i)) : Float)
  up = (compute.read ((compute.Read _)(.from k.in.u, .at i)) : Float)
  open = gate / (1.0 + (exp (0.0 - gate)))
  compute.write ((compute.Write _)(.to k.out.m, .at i, .value (open * up)))
}
glue = compute.parallel glue_raw "gpu" GlOut
LIn    = struct{.x (compute.Buf Float), .i (compute.Buf Float), .q (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .qn (compute.Buf Float), .kn (compute.Buf Float), .o (compute.Buf Float), .p (compute.Buf Float), .g (compute.Buf Float), .u (compute.Buf Float), .d (compute.Buf Float)}
GArg   = struct{.position Int, .in LIn}
step = (s : GArg) => {
  s1 = (compute.plrun squares ((SqArg)(.n 1, .dim 2048, .I SqIn(.x s.in.x))) : SqOut)
  n1 = (compute.plrun scale ((ScArg)(.n 2048, .dim 2048, .I ScIn(.s s1.s, .x s.in.x, .w s.in.i))) : ScOut)
  qs = (compute.plrun matvec ((MvArg)(.n 2048, .dim 2048, .I MvIn(.x n1.y, .w s.in.q))) : MvOut)
  ks = (compute.plrun matvec ((MvArg)(.n 1024, .dim 2048, .I MvIn(.x n1.y, .w s.in.k))) : MvOut)
  vs = (compute.plrun matvec ((MvArg)(.n 1024, .dim 2048, .I MvIn(.x n1.y, .w s.in.v))) : MvOut)
  sq = (compute.plrun squares ((SqArg)(.n 16, .dim 128, .I SqIn(.x qs.y))) : SqOut)
  qn = (compute.plrun scale ((ScArg)(.n 2048, .dim 128, .I ScIn(.s sq.s, .x qs.y, .w s.in.qn))) : ScOut)
  sk = (compute.plrun squares ((SqArg)(.n 8, .dim 128, .I SqIn(.x ks.y))) : SqOut)
  kn = (compute.plrun scale ((ScArg)(.n 1024, .dim 128, .I ScIn(.s sk.s, .x ks.y, .w s.in.kn))) : ScOut)
  qr = (compute.plrun rope ((RpArg)(.n 2048, .position s.position, .I RpIn(.x qn.y))) : RpOut)
  kr = (compute.plrun rope ((RpArg)(.n 1024, .position s.position, .I RpIn(.x kn.y))) : RpOut)
  at = (compute.plrun spread ((SpArg)(.n 2048, .I SpIn(.v vs.y))) : SpOut)
  os = (compute.plrun matvec ((MvArg)(.n 2048, .dim 2048, .I MvIn(.x at.y, .w s.in.o))) : MvOut)
  hs = (compute.plrun add ((AdArg)(.n 2048, .I AdIn(.a s.in.x, .b os.y))) : AdOut)
  s4 = (compute.plrun squares ((SqArg)(.n 1, .dim 2048, .I SqIn(.x hs.y))) : SqOut)
  n2 = (compute.plrun scale ((ScArg)(.n 2048, .dim 2048, .I ScIn(.s s4.s, .x hs.y, .w s.in.p))) : ScOut)
  gs = (compute.plrun matvec ((MvArg)(.n 6144, .dim 2048, .I MvIn(.x n2.y, .w s.in.g))) : MvOut)
  us = (compute.plrun matvec ((MvArg)(.n 6144, .dim 2048, .I MvIn(.x n2.y, .w s.in.u))) : MvOut)
  ms = (compute.plrun glue ((GlArg)(.n 6144, .I GlIn(.g gs.y, .u us.y))) : GlOut)
  ds = (compute.plrun matvec ((MvArg)(.n 2048, .dim 6144, .I MvIn(.x ms.m, .w s.in.d))) : MvOut)
  compute.plrun add ((AdArg)(.n 2048, .I AdIn(.a hs.y, .b ds.y)))
}
built = compute.graph step
built
"#;

/// Attention over two positions, which is the first place the query and the key decide anything.
///
/// # Invariant
/// The three dispatches are the model's own: every query head scores both cached positions against
/// its grouped key head, the two scores become a softmax, and the answer mixes the two values by it.
/// Each lane writes exactly once, because a lane is one `(head, position)` in the first two and one
/// element of one head in the third.
pub const ATTEND_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In       = struct{.q (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float)}
Scored   = struct{.s (compute.Buf Float)}
Weighted = struct{.w (compute.Buf Float)}
Mixed    = struct{.o (compute.Buf Float)}
ScorePar = struct{.n Int, .in In, .out Scored}
ScoreArg = struct{.n Int, .I In}
score_raw = (k : ScorePar) => {
  lane = compute.range k.n
  head = lane / 2
  at = lane - head * 2
  offset = at * 1024 + (head / 2) * 128
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.q, .at (head * 128 + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.k, .at (offset + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at lane, .value (fold (128, 0.0 ~ "identity") * 0.0883883476)))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Scored
WeightPar = struct{.n Int, .in Scored, .out Weighted}
WeightArg = struct{.n Int, .I Scored}
weight_raw = (k : WeightPar) => {
  lane = compute.range k.n
  head = lane / 2
  first = (compute.read ((compute.Read _)(.from k.in.s, .at (head * 2))) : Float)
  second = (compute.read ((compute.Read _)(.from k.in.s, .at (head * 2 + 1))) : Float)
  top = max first second
  high = exp (first - top)
  low = exp (second - top)
  total = high + low
  mine = if lane - (head * 2) == 0 then high else low
  compute.write ((compute.Write _)(.to k.out.w, .at lane, .value (mine / total)))
}
weight = compute.parallel weight_raw "gpu" Weighted
MixIn = struct{.w (compute.Buf Float), .v (compute.Buf Float)}
MixPar = struct{.n Int, .in MixIn, .out Mixed}
MixArg = struct{.n Int, .I MixIn}
mix_raw = (k : MixPar) => {
  lane = compute.range k.n
  head = lane / 128
  column = lane - head * 128
  offset = (head / 2) * 128 + column
  first = (compute.read ((compute.Read _)(.from k.in.w, .at (head * 2))) : Float)
  second = (compute.read ((compute.Read _)(.from k.in.w, .at (head * 2 + 1))) : Float)
  early = (compute.read ((compute.Read _)(.from k.in.v, .at offset)) : Float)
  late = (compute.read ((compute.Read _)(.from k.in.v, .at (1024 + offset))) : Float)
  compute.write ((compute.Write _)(.to k.out.o, .at lane, .value ((first * early) + (second * late))))
}
mix = compute.parallel mix_raw "gpu" Mixed
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun score ((ScoreArg)(.n s.n, .I In(.q s.in.q, .k s.in.k, .v s.in.v))) : Scored)
  b = (compute.plrun weight ((WeightArg)(.n s.n, .I Scored(.s a.s))) : Weighted)
  compute.plrun mix ((MixArg)(.n 2048, .I MixIn(.w b.w, .v s.in.v)))
}
built = compute.graph step
built
"#;

/// Attention over a cache of any length: the four dispatches a growing context needs.
///
/// # Invariant
/// The position count is the caller's, so nothing here is written for two: one dispatch scores every
/// `(head, position)` pair by folding the head's width, one sums each head's exponentials by folding the
/// positions, one divides them into weights, and one mixes the cached values by folding the positions
/// again. A lane writes exactly once in each, so the four are separate dispatches.
pub const ATTEND_CACHE_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In       = struct{.q (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float)}
Scored   = struct{.s (compute.Buf Float)}
Totaled  = struct{.t (compute.Buf Float)}
Weighted = struct{.w (compute.Buf Float)}
Mixed    = struct{.o (compute.Buf Float)}
ScorePar = struct{.n Int, .p Int, .in In, .out Scored}
ScoreArg = struct{.n Int, .p Int, .I In}
score_raw = (k : ScorePar) => {
  lane = compute.range k.n
  head = lane / k.p
  at = lane - head * k.p
  offset = at * 1024 + (head / 2) * 128
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.q, .at (head * 128 + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.k, .at (offset + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at lane, .value (fold (128, 0.0 ~ "identity") * 0.0883883476)))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Scored
TotalIn  = struct{.s (compute.Buf Float)}
TotalPar = struct{.n Int, .p Int, .in TotalIn, .out Totaled}
TotalArg = struct{.n Int, .p Int, .I TotalIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.s, .at (lane * k.p + (s(0) - 1)))) : Float)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.t, .at lane, .value (fold (k.p, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu" Totaled
WeightIn  = struct{.s (compute.Buf Float), .t (compute.Buf Float)}
WeightPar = struct{.n Int, .p Int, .in WeightIn, .out Weighted}
WeightArg = struct{.n Int, .p Int, .I WeightIn}
weight_raw = (k : WeightPar) => {
  lane = compute.range k.n
  head = lane / k.p
  whole = (compute.read ((compute.Read _)(.from k.in.t, .at head)) : Float)
  part = exp ((compute.read ((compute.Read _)(.from k.in.s, .at lane)) : Float))
  compute.write ((compute.Write _)(.to k.out.w, .at lane, .value (part / whole)))
}
weight = compute.parallel weight_raw "gpu" Weighted
MixIn  = struct{.w (compute.Buf Float), .v (compute.Buf Float)}
MixPar = struct{.n Int, .p Int, .in MixIn, .out Mixed}
MixArg = struct{.n Int, .p Int, .I MixIn}
mix_raw = (k : MixPar) => {
  lane = compute.range k.n
  head = lane / 128
  column = lane - head * 128
  key = head / 2
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (head * k.p + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.v, .at (((s(0) - 1) * 1024 + key * 128) + column))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.o, .at lane, .value (fold (k.p, 0.0 ~ "identity"))))
}
mix = compute.parallel (mix_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Mixed
GArg = struct{.n Int, .p Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun score ((ScoreArg)(.n s.n, .p s.p, .I In(.q s.in.q, .k s.in.k, .v s.in.v))) : Scored)
  b = (compute.plrun total ((TotalArg)(.n 16, .p s.p, .I TotalIn(.s a.s))) : Totaled)
  c = (compute.plrun weight ((WeightArg)(.n s.n, .p s.p, .I WeightIn(.s a.s, .t b.t))) : Weighted)
  compute.plrun mix ((MixArg)(.n 2048, .p s.p, .I MixIn(.w c.w, .v s.in.v)))
}
built = compute.graph step
built
"#;

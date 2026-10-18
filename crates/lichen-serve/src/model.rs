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
  value = compute.read ((compute.Read _)(.from k.in.k, .at i))
  query = compute.read ((compute.Read _)(.from k.in.q, .at (sequence * k.dim + column)))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value * query)))
}
weigh = compute.parallel weigh_raw "gpu"
SumPar = struct{.n Int, .cap Int, .dim Int, .in TallyIn, .out Out}
SumArg = struct{.n Int, .cap Int, .dim Int, .I TallyIn}
sum_raw = (k : SumPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.dim + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
sum = compute.parallel (sum_raw ~ "claim:Reduction(combine,identity)") "gpu"
PeakPar = struct{.n Int, .cap Int, .in PeakIn, .out Out}
PeakArg = struct{.n Int, .cap Int, .I PeakIn}
peak_raw = (k : PeakPar) => {
  lane = compute.range k.n
  base = lane * k.cap
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, ((0.0 - 1.0) / 0.0) ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity)") "gpu"
MaskIn  = struct{.s (compute.Buf Float), .length (compute.Buf Int)}
MaskPar = struct{.n Int, .cap Int, .in MaskIn, .out Out}
MaskArg = struct{.n Int, .cap Int, .I MaskIn}
mask_raw = (k : MaskPar) => {
  lane = compute.range k.n
  sequence = lane / k.cap
  position = lane - sequence * k.cap
  limit = compute.read ((compute.Read _)(.from k.in.length, .at sequence))
  visible = position < limit
  value = compute.read ((compute.Read _)(.from k.in.s, .at lane))
  hidden = (0.0 - 1.0) / 0.0
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (if visible then value else hidden)))
}
mask = compute.parallel mask_raw "gpu"
ExpoPar = struct{.n Int, .cap Int, .in ExpoIn, .out Out}
ExpoArg = struct{.n Int, .cap Int, .I ExpoIn}
expo_raw = (k : ExpoPar) => {
  i = compute.range k.n
  sequence = i / k.cap
  value = compute.read ((compute.Read _)(.from k.in.s, .at i))
  top = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  shifted = value - top
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (exp shifted)))
}
expo = compute.parallel expo_raw "gpu"
TotalPar = struct{.n Int, .cap Int, .in TallyIn, .out Out}
TotalArg = struct{.n Int, .cap Int, .I TallyIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.t, .at (lane * k.cap + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu"
ScalePar = struct{.n Int, .cap Int, .dim Int, .in ScaleIn, .out Out}
ScaleArg = struct{.n Int, .cap Int, .dim Int, .I ScaleIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  sequence = i / (k.cap * k.dim)
  column = i - (i / k.dim) * k.dim
  position = (i / k.dim) - sequence * k.cap
  weight = compute.read ((compute.Read _)(.from k.in.s, .at (sequence * k.cap + position)))
  divisor = compute.read ((compute.Read _)(.from k.in.total, .at sequence))
  value = compute.read ((compute.Read _)(.from k.in.v, .at i))
  share = weight / divisor
  compute.write ((compute.Write _)(.to k.out.e, .at ((sequence * k.dim + column) * k.cap + position), .value (share * value)))
}
scale = compute.parallel scale_raw "gpu"
PosPar = struct{.n Int, .cap Int, .in PosIn, .out Out}
PosArg = struct{.n Int, .cap Int, .I PosIn}
pos_raw = (k : PosPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.cap + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.cap, 0.0 ~ "identity"))))
}
positions = compute.parallel (pos_raw ~ "claim:Reduction(combine,identity)") "gpu"
AddPar = struct{.n Int, .in AddIn, .out Out}
AddArg = struct{.n Int, .I AddIn}
add_raw = (k : AddPar) => {
  i = compute.range k.n
  left = compute.read ((compute.Read _)(.from k.in.a, .at i))
  right = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (left + right)))
}
add = compute.parallel add_raw "gpu"
MixPar = struct{.n Int, .dim Int, .in MixIn, .out Out}
MixArg = struct{.n Int, .dim Int, .I MixIn}
mix_raw = (k : MixPar) => {
  i = compute.range k.n
  sequence = i / k.dim
  value = compute.read ((compute.Read _)(.from k.in.h, .at i))
  summary = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (value + summary)))
}
mix = compute.parallel mix_raw "gpu"
SpreadPar = struct{.n Int, .dim Int, .in SpreadIn, .out Out}
SpreadArg = struct{.n Int, .dim Int, .I SpreadIn}
spread_raw = (k : SpreadPar) => {
  i = compute.range k.n
  sequence = i / (k.dim * k.dim)
  rest = i - sequence * (k.dim * k.dim)
  column = rest / k.dim
  inner = rest - column * k.dim
  hidden = compute.read ((compute.Read _)(.from k.in.n, .at (sequence * k.dim + inner)))
  weight = compute.read ((compute.Read _)(.from k.in.w, .at (column * k.dim + inner)))
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (hidden * weight)))
}
spread = compute.parallel spread_raw "gpu"
ProjectPar = struct{.n Int, .dim Int, .in PosIn, .out Out}
ProjectArg = struct{.n Int, .dim Int, .I PosIn}
project_raw = (k : ProjectPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (lane * k.dim + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.e, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
project = compute.parallel (project_raw ~ "claim:Reduction(combine,identity)") "gpu"
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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at (sequence * k.dim + (s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.dim, 0.0 ~ "identity"))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
PeakIn  = struct{.s (compute.Buf Float)}
PeakPar = struct{.n Int, .rows Int, .in PeakIn, .out ScoreOut}
PeakArg = struct{.n Int, .rows Int, .I PeakIn}
peak_raw = (k : PeakPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1)))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.rows, ((0.0 - 1.0) / 0.0) ~ "identity"))))
}
peak = compute.parallel (peak_raw ~ "claim:Reduction(combine,identity)") "gpu"
ExpoIn  = struct{.s (compute.Buf Float), .m (compute.Buf Float)}
ExpoPar = struct{.n Int, .rows Int, .temp Float, .in ExpoIn, .out ScoreOut}
ExpoArg = struct{.n Int, .rows Int, .temp Float, .I ExpoIn}
expo_raw = (k : ExpoPar) => {
  i = compute.range k.n
  sequence = i / k.rows
  value = compute.read ((compute.Read _)(.from k.in.s, .at i))
  top = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  shifted = (value - top) / k.temp
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (exp shifted)))
}
expo = compute.parallel expo_raw "gpu"
TotalPar = struct{.n Int, .rows Int, .in PeakIn, .out ScoreOut}
TotalArg = struct{.n Int, .rows Int, .I PeakIn}
total_raw = (k : TotalPar) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.s, .at (lane * k.rows + (s(0) - 1))))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.y, .at lane, .value (fold (k.rows, 0.0 ~ "identity"))))
}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity)") "gpu"
SharePar = struct{.n Int, .rows Int, .in ExpoIn, .out ScoreOut}
ShareArg = struct{.n Int, .rows Int, .I ExpoIn}
share_raw = (k : SharePar) => {
  i = compute.range k.n
  sequence = i / k.rows
  weight = compute.read ((compute.Read _)(.from k.in.s, .at i))
  divisor = compute.read ((compute.Read _)(.from k.in.m, .at sequence))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (weight / divisor)))
}
share = compute.parallel share_raw "gpu"
Pick     = struct{.p (compute.Buf Float)}
PickPar  = struct{.n Int, .rows Int, .seed Float, .in PeakIn, .out Pick}
PickArg  = struct{.n Int, .rows Int, .seed Float, .I PeakIn}
pick_raw = (k : PickPar) => {
  lane = compute.range k.n
  base = lane * k.rows
  drawn_lane = int2float lane
  noise = sin ((k.seed * 12.9898) + (drawn_lane * 78.233))
  drawn = (noise * 43758.545) - floor (noise * 43758.545)
  @loop fold = s => if s(0) == 0 then s(3) else fold (s(0) - 1, s(1), (s(2) + (compute.read ((compute.Read _)(.from k.in.s, .at (base + s(0) - 1))))) ~ "combine", (s(3) + ((1 * (s(2) <= (1.0 - s(1)))) * ((s(0) - 1) - s(3)))) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.p, .at lane, .value (int2float (fold (k.rows, drawn, 0.0 ~ "identity", 0)))))
}
pick = compute.parallel (pick_raw ~ "claim:Reduction(combine,identity)") "gpu"
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
  first = compute.read ((compute.Read _)(.from k.in.pos, .at sequence))
  after = 1 * (position >= first)
  before = 1 * (position < (first + k.count))
  fresh = (after * before) == 1
  slot = (sequence * k.count + (position - first)) * k.dim + column
  old_k = compute.read ((compute.Read _)(.from k.in.k, .at lane))
  old_v = compute.read ((compute.Read _)(.from k.in.v, .at lane))
  new_k = if fresh then (compute.read ((compute.Read _)(.from k.in.nk, .at slot))) else old_k
  new_v = if fresh then (compute.read ((compute.Read _)(.from k.in.nv, .at slot))) else old_v
  (compute.write ((compute.Write _)(.to k.out.k, .at lane, .value (new_k ~ "element"))), compute.write ((compute.Write _)(.to k.out.v, .at lane, .value (new_v ~ "element"))))
}
fill = compute.parallel (fill_raw ~ "claim:Reduction(element)") "gpu"
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
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at 0, .value (fold (k.dim, 0.0 ~ "identity"))))
}
squares = compute.parallel (squares_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
ScaleIn  = struct{.s (compute.Buf Float), .x (compute.Buf Float), .w (compute.Buf Float)}
ScalePar = struct{.n Int, .dim Int, .in ScaleIn, .out Normed}
ScaleArg = struct{.n Int, .dim Int, .I ScaleIn}
scale_raw = (k : ScalePar) => {
  i = compute.range k.n
  total = compute.read ((compute.Read _)(.from k.in.s, .at 0))
  mean = total / (int2float k.dim)
  divisor = sqrt (mean + 0.000001)
  value = compute.read ((compute.Read _)(.from k.in.x, .at i))
  weight = compute.read ((compute.Read _)(.from k.in.w, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((value * weight) / divisor)))
}
scale = compute.parallel scale_raw "gpu"
GArg = struct{.n Int, .dim Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun squares ((SquareArg)(.n 1, .dim s.dim, .I In(.x s.in.x, .w s.in.w))) : Squared)
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
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.w, .at (row * k.dim + (s(0) - 1)))) * compute.read ((compute.Read _)(.from k.in.x, .at ((s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at row, .value (fold (k.dim, 0.0 ~ "identity"))))
}
matvec = compute.parallel (matvec_raw ~ "claim:Reduction(combine,identity,element)") "gpu"
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = compute.read ((compute.Read _)(.from k.in.d, .at i))
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu"
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
  own = compute.read ((compute.Read _)(.from k.in.x, .at i))
  other = compute.read ((compute.Read _)(.from k.in.x, .at other_index))
  turned = if low then j else (j - 64)
  exponent = (0.0 - (int2float turned)) / 64.0
  scale = exp (exponent * (ln (1000000.0)))
  angle = (int2float k.position) * scale
  cosine = cos angle
  sine = sin angle
  signed = if low then (0.0 - 1.0) else 1.0
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((own * cosine) + ((signed * other) * sine))))
}
rope = compute.parallel rope_raw "gpu"
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = compute.read ((compute.Read _)(.from k.in.d, .at i))
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu"
GArg = struct{.n Int, .position Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun rope ((RopeArg)(.n s.n, .position s.position, .I In(.x s.in.x))) : Out)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#;

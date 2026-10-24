# The serving frontier: the units it changed, and the ones lichen has

> Status: **research and roadmap.** A reading of what production LLM serving does outside this
> tree, mapped against what this tree measures, ending in one slice named as the next build.
> Nothing here is a plan of record for the kernel work: the operator ledger stays
> [llm-operators](llm-operators.md), the fusion design stays [graph-fusion](graph-fusion.md), and
> the kernel-shape roadmap stays [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md). This note is
> the serving layer's reading of those, plus the techniques neither note covers.
>
> Points at: `crates/lichen-serve` (`qwen.rs`, `service.rs`, the example server),
> `crates/lichen-graph-ir` (`Policy`, `Scalar`, `Count`), `crates/lichen-runtime` (the passes,
> the artifacts), `crates/lichen-compute-gpu` (`dispatch.rs`).
>
> Companions: [qwen-inference](qwen-inference.md) (the served model's measurements),
> [inference-service](inference-service.md) (the schedule and its rule), [graph-fusion](graph-fusion.md)
> (the fusion substrate), [llm-operators](llm-operators.md) (the operator ladder),
> [shared-memory-and-barriers](shared-memory-and-barriers.md) (the tile mechanism),
> [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) (the kernel-surface gaps).

**The claim this note argues** is not that the systems cited below are faster. It is that every
durable win in LLM serving came from **changing the unit of work**, and that lichen's units sit one
scale too small in three places: a **dispatch** where the field uses a **tile**, a **request** where
the field uses an **iteration**, and a **prompt** where the field uses a **chunk**. The first is an
operator/fusion problem, the second and third are scheduling problems, and the measured numbers in
§2 say which of the three lichen-serve should attack next.

## 1. The units the field changed

| the unit, before → after | who made it stick | what it bought | the mechanism it needed |
|---|---|---|---|
| one request → **one iteration** | Orca (OSDI'22), then vLLM | a finished sequence leaves the batch without draining it | a scheduler that forms a batch every step |
| a sequence's cache → **a page** | [PagedAttention / vLLM](https://arxiv.org/abs/2309.06180) | near-zero KV fragmentation, sharing by block | an indirection from logical position to physical block |
| a prompt → **a chunk** | [Sarathi-Serve](https://arxiv.org/abs/2403.02310) | prefill no longer stalls decodes; tail latency under load | a prefill that can resume at a position |
| one replica → **a prefill worker and a decode worker** | [DistServe](https://arxiv.org/abs/2401.09670), [Mooncake](https://arxiv.org/abs/2407.00079), Splitwise | each phase sized and scheduled for its own bottleneck | KV transfer between workers |
| one token → **a verified draft of tokens** | [speculative decoding](https://arxiv.org/abs/2211.17192), [Medusa](https://arxiv.org/abs/2401.10774), [EAGLE-3](https://arxiv.org/abs/2503.01840) | fewer target-model steps per token | a draft model or head, and a verification step |
| a whole-sequence attention → **a tile with an online softmax** | [FlashAttention](https://arxiv.org/abs/2205.14135), [-2](https://arxiv.org/abs/2307.08691), [-3](https://arxiv.org/abs/2407.08608) | attention becomes bandwidth-bound instead of materialising the score matrix | shared memory, cross-lane reduction, async copy |
| a prefix → **a radix-tree node** | [SGLang / RadixAttention](https://arxiv.org/abs/2312.07104) | repeated chat prefixes are computed once | a cache keyed by token sequence, with sharing |
| a kernel launch → **a captured graph** | CUDA graphs, adopted by every serving stack | launch overhead leaves the step | whole-graph recording |
| a weight → **8 or 4 bits** | [GPTQ](https://arxiv.org/abs/2210.17323), [AWQ](https://arxiv.org/abs/2306.00978), [FP8 in DeepSeek-V3](https://arxiv.org/abs/2412.19437) | a layer reads half or a quarter of the bytes | a packed class and a dequantising read |

Two readings of that table matter before any technique is copied.

**The left column is where the win is.** FlashAttention is not a faster dot product; it is a
different *unit* for attention (a tile that never writes the score matrix). PagedAttention is not a
faster cache; it is a different *unit* for memory (a page behind an indirection). Chunked prefill is
not a faster prefill; it is a different *unit* for a request (a resumable chunk). A serving system
that keeps its units and copies the arithmetic gets the constants and misses the wins.

**The right column is what a unit change costs.** Every row's enabling mechanism is a cross-lane or
cross-request capability, not an algebraic identity: shared memory and barriers (FlashAttention),
a page table (PagedAttention), a resumable position (chunked prefill), a network (disaggregation),
a draft model (speculation). That is why [graph-fusion](graph-fusion.md)'s first draft — a fixed
tensor-operator vocabulary with `ml.map`/`ml.reduce` — was rejected: the unit is the mechanism, and
a vocabulary cannot supply one.

## 2. What lichen-serve measures today

The numbers below are this tree's, each with the note that owns it. They are the only baseline the
roadmap is allowed to argue against.

| reading | value | the note |
|---|---|---|
| a dispatch's floor, input resident | **~0.33–0.4 ms** | [qwen-inference](qwen-inference.md), [inference-service](inference-service.md) |
| nodes in one layer's steps | **30** at one sequence (three norms ×2, seven projections ×2, two rotations ×2, attention 4, the product 2) | `prepare_steps` at this tip |
| a token's dispatch floor | **840 × ~0.4 ms ≈ 336 ms** of the 0.58 s warm token | the probe, and §10's token |
| one step, one request, with the stages timed | **0.708 s**, of which **0.250 s** is around them | same |
| one step, eight sequences, warm | **0.86 s** | same |
| one step, one sequence, resident | **0.576 s** | same |
| a submission/fence per stage | **15 stage calls a layer**, each with a readback | `Qwen::layer_batch` |
| the batch's marginal cost | one step serves **16 sequences at a 1-sequence step's cost** | [inference-service](inference-service.md) |
| aggregate throughput past eight clients | **flat** — 8 clients is the service's worth | [qwen-inference](qwen-inference.md) |
| a prompt's fill | **one token a row a step**: `Batched::step` advances every live row one position | `qwen.rs` |
| resident weights vs host-visible | **2.23x** | [qwen-inference](qwen-inference.md) |
| resident cache vs re-uploaded | **98–99% of a step** | [inference-service](inference-service.md) |
| one submission's dispatch ceiling | **64**, refused above it | [inference-service](inference-service.md) |

Three conclusions follow directly, and they are the frame for everything below.

- **The step is not dispatch-bound.** 840 dispatches at the floor are **~336 ms of the 0.58 s warm
  token**; the other ~240 ms is the work *around* the dispatches: the fetches, the uploads, the host
  arithmetic between stages, and the per-sequence loops. A fusion that halves the dispatches and
  leaves the surroundings alone therefore has a ceiling of about three fifths of the step.
  ([qwen-inference](qwen-inference.md)'s 281 dispatches is an earlier layer shape's count; the
  prepared node counts are the current reading, and the shipped layer has fifteen stage calls, not
  ten.)
- **The batch is not the lever.** Sixteen sequences cost one sequence's submission and fence, and
  throughput is flat past eight clients because the step rate is the bound. Widening the batch buys
  latency, not tokens. **The per-step clock is the throughput.**
- **The prompt is served in the worst possible chunk.** `Batched` advances a filling row one token
  a step, so a prompt of `P` tokens costs `P` whole-model steps before it answers, and those steps
  are the same steps the decoders wait in. The field's answer is a chunk of a few dozen to a few
  hundred tokens a step, which needs a table of prefill costs to choose from.

## 3. Operators: what the frontier runs, and what a buffer map can say

### 3.1 Attention is a tile

The FlashAttention line is the clearest case of a unit change. A naive attention materialises
`scores = QKᵀ`, softmaxes it, then multiplies by `V`; each step writes and rereads an
`n × n` matrix. FlashAttention tiles the sequence, keeps `Q`, `K`, `V` and the running softmax
statistics in shared memory or registers, and never writes the matrix. FlashAttention-2 reworked
the work partitioning and the non-matmul FLOPs; FlashAttention-3 added warp specialisation, TMA
asynchrony and FP8, reporting 1.5–2x over FA2 on an H100 and 740 TFLOP/s of FP16 (75% of peak)
([2407.08608](https://arxiv.org/abs/2407.08608)). Decode has its own split — FlashDecoding's
partition over the KV axis, because a decode step has one query and a long key axis — and every
serving stack now ships both shapes.

What the tile needs, and what lichen has, is the whole question:

| the tile needs | lichen's state |
|---|---|
| a cross-lane reduction inside one dispatch | **landed at the plan level**: `Workgroup` arrays, `OpControlBarrier`, the barrier scan, a declared staging plan, and `plan_staging` runs the closed form on a device ([shared-memory-and-barriers](shared-memory-and-barriers.md) §9) |
| a language surface for the lane group | **missing**: `compute.parallel` is a buffer map, one lane per index, straight-line body; a `@loop` folds inside a lane and cannot be nested inside another nest's step ([llm-operators](llm-operators.md), [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md)) |
| a runtime extent the plan can tile | **refused by name**: "its launch extent is not readable here, and a tiled fold is built for a whole number of tiles" ([qwen-inference](qwen-inference.md)) |
| dynamic addressing into the cache | **expressible**: `y[i] = x[tbl[i]]` answers `[60, 10, 50, 20, 40, 30]`, and one lane past the end answers `0` ([inference-service](inference-service.md) §3) |
| an output the same dispatch reads | **not in the contract**: `.in`/`.out` are disjoint by construction, and output aliasing is a backend decision stated in [inference-service](inference-service.md) §6 |

So the FlashAttention shape is **not a missing operator; it is a missing surface over a landed
mechanism**. This is the same conclusion [qwen-inference](qwen-inference.md) reaches from the
other side: a projection can already be tiled when its launch extent is a literal and its
identity's class matches, and what remains is the argument list of the rewritten graph.

### 3.2 The projection is two kernels in one costume

A prefill projection is a GEMM: many rows against one weight, compute-bound, and the right unit is
a tile of outputs with `K` split or streamed. A decode projection is a GEMV: one row, bandwidth-
bound, and the right unit is the whole weight read once. Production stacks specialise the two
(CUTLASS/CuBLASLt for prefill, a specialised GEMV or a fused attention for decode), and the
distinction is forced by the arithmetic intensity, not by taste.

lichen has the decode shape: `MATVEC_STEP` launches one lane per output element and folds the
weight's row inside the lane. Measured, it is flat at **~0.4 ms a dispatch and 85 GB/s** at 4096
rows, a quarter of the card ([qwen-inference](qwen-inference.md)). That flatness is the tell: the
dispatch is over before the row count can use the card, because the parallelism is `rows`, and the
fold inside a lane is serial. A prefill row count of 64–512 has the same problem. The tiled
projection — lanes cooperating on one row, each taking a slice of `K`, then a cross-lane reduction —
is the same mechanism FlashAttention needs, which is why the two arrive together or not at all.

### 3.3 The glue is where the bandwidth is

RMSNorm, the residual add, RoPE, SwiGLU, the softmax's scale — in a production stack these are
fused into the GEMM's epilogue or the attention kernel, because each is a full read and write of a
tensor for a handful of FLOPs. lichen runs them as their own dispatches, and their measured cost is
not arithmetic: the layer's elementwise stages are a dispatch each, and the host round trip between
them is the [§2](#2-what-lichen-serve-measures-today) 0.25 s.

The frontier's answer is **fusion by producer-consumer pairs, not by graph pattern**: a norm whose
only consumer is a projection, a residual whose only consumer is the next norm. Flashlight and
FlexAttention make the same point for attention variants — the compiler fuses and tiles the
computation instead of selecting a template ([2511.02043](https://arxiv.org/abs/2511.02043)).

### 3.4 Precision is a serving operator

Every efficiency number in serving is bytes first. The checkpoint lichen serves is bf16 on disk and
f32 on the wire; the weights are 3.3 GB at half width and 6.6 GB at full, which is why half width
was the difference between 13.4 s and 6.3 s a token, and why the f32→bf16 widening had to be
removed from the per-token path ([qwen-inference](qwen-inference.md)). The next step in the field
is a packed class the kernel reads directly — FP8 with per-block scales (DeepSeek-V3), INT4
weight-only with a dequantising read (GPTQ/AWQ) — and for the KV cache, FP8/INT8 with per-channel
scales (KIVI, [2402.02750](https://arxiv.org/abs/2402.02750)). What lichen has is `BFloat16` as a
`ScalarClass`; what it lacks is a pack that states a narrower class for a weight and a kernel that
reads it (the bf16 pack exists and narrows a projection; the tiled body's class conflict is
measured and recorded in [qwen-inference](qwen-inference.md)).

On this card the arithmetic is blunt: 6 GB holds 3.3 GB of bf16 weights and about 1.3 GB of pool,
so the KV cache lives on the host and a long context pays its upload. Every halving of the weight
class is a layer that reads half the bytes and a context that fits twice as far.

### 3.5 The head and the sampler

The vocabulary head is the largest single tensor (593 MB bf16) and it is read once a token, which
makes it the one place where a per-token read dominates. The sampler (greedy, top-k, top-p, logprobs)
is already a device step in this tree and is not a bottleneck; the frontier's contributions here are
penalties and constrained decoding, neither of which changes the step's cost.

### 3.6 What the kernel surface can and cannot say

A reading of the surface, not of the device:

| the surface says | the consequence |
|---|---|
| one invocation per index, straight-line body | a reduction's parallelism is its output's, not its input's |
| reads from bound buffers, writes to **fresh** output buffers | an in-place accumulation is unspellable; a cache rewrite must copy |
| a `@loop` folds inside one lane | online softmax's `(max, sum, acc)` state has no cross-lane home |
| a nested `@loop` in a fold's step is refused | the one-kernel attention is not writable, and the roof is a rule, not a bug ([llm-operators](llm-operators.md)) |
| a runtime scalar is a push constant | a per-step value the device computed must live in a buffer ([inference-service](inference-service.md) §4) |
| staging is a declared plan over a workgroup | the tile exists below the surface but not in it |
| an out-of-buffer read answers `0` | a padded batch and a masked window need no branch |

The gap is therefore specific: **the tile mechanism is built, the language cannot yet name it, and
the plans are fed by literal extents.** Everything in §7 that touches attention or GEMM is gated on
that one sentence.

## 4. Fusion

### 4.1 The three fusion units

Production fusion happens at three granularities, and they are not interchangeable.

1. **The chain.** Two elementwise producers into one consumer, a reduction into its normaliser.
   This is the classic fusion a graph pass finds, and lichen's region merge is exactly this: a
   region two edges arrive at merges when every arrival is named and proven, and a chain merges at
   any length. RMSNorm and the LayerNorm diamond both merge ([llm-operators](llm-operators.md)).
2. **The tile.** A loop blocked over a workgroup, with the intermediate in shared memory or
   registers. This is FlashAttention, a split-K GEMM, a softmax whose maximum is a workgroup
   reduction. It is not a graph rewrite: the two outputs are not nodes, they are lanes.
3. **The launch.** Many dispatches into one submission (CUDA graphs, lichen's `Policy::Batch`).
   This removes host and driver overhead, not memory traffic, and its measured worth here is
   bounded: a recorded step is one submission and one fence however many dispatches it holds, but
   a composed graph of nine dispatches was **slower** than the same work as separate steps
   (5.97 s against 4.52 s, [qwen-inference](qwen-inference.md)). The launch unit is not the
   performance unit.

### 4.2 Where production compilers put the boundary

| system | the IR the fusion is decided in | what it buys |
|---|---|---|
| XLA | HLO, with layout as a first-class citizen | fusion, layout, whole-program scheduling |
| TorchInductor | a loop-level IR over tiles, lowered to Triton | epilogue fusion and pointwise chains at the loop level |
| TVM | TensorIR with schedules | tile sizes and memory scopes are the schedule |
| Triton | a block-level language | the programmer writes the tile and the compiler maps it |
| [TileLang](https://github.com/tile-ai/tilelang) / [ThunderKittens](https://arxiv.org/abs/2410.20399) | a tile-level DSL | explicit shared-memory tiles, async copy, warp cooperation |
| FlexAttention / [Flashlight](https://arxiv.org/abs/2511.02043) | a template or a compiled attention program | attention variants fused and tiled without a kernel per variant |
| [FlashInfer](https://arxiv.org/abs/2501.01005) | a kernel *library* with a serving-aware API | prefill/decode/page attention as separate tuned kernels |

The pattern is that the successful fusers decide **inside a loop nest**, where the memory hierarchy
is visible. An operator-graph fuser can only see that two operators are adjacent; it cannot see that
the intermediate fits in shared memory, or that the reduction can be split without a second pass.

### 4.3 Why the operator vocabulary lost, in one example

FlashAttention is the proof by counterexample. If attention were `matmul → softmax → matmul`, a
graph fuser could pattern-match it. Production attention is not that: it carries a causal mask, a
sliding window, ALiBi, softcap, a bias tensor, a paged cache, a grouped KV head, and a quantised
KV — and every variant changes the tile's loop bounds or its mask, not the operator graph. The
field's answer was to make the *kernel* programmable (FlexAttention, Flashlight, Triton) rather
than to grow the operator vocabulary. [graph-fusion](graph-fusion.md) §1 reached the same verdict
before this survey and for the same reason; the cited systems are the evidence that the verdict is
the field's, not this tree's preference.

### 4.4 lichen's fusion is algebra at the graph and tiles at the plan

The tree has both halves, at different layers:

- **At the graph**: claims and interface laws license a merge, and the region merge pass fires on
  chains and fan-ins. A proof is keyed by a digest, so a merged region can be checked rather than
  trusted. This is stronger than a pattern match and weaker than a compiler: it decides *whether*
  two nodes may fuse, not *how* the fused body is tiled.
- **At the plan**: the tiling pass reads a declared plan, stages a reduction over a lane group,
  and emits a body with `Workgroup` arrays and a barrier. Measured, a projection fires the pass
  when its launch extent is a literal and its identity's class matches, and a hand-declared plan
  runs the closed form on the device ([shared-memory-and-barriers](shared-memory-and-barriers.md) §9).
  This is the tile half.

What is missing between them is the surface: the plan can tile a body the language cannot yet
write, and the graph a pass rewrites takes arguments the caller's program does not have. Both are
plumbing-with-a-theory, not new mathematics, and both are prerequisites for every §7 slice that
touches attention, GEMM or prefill.

### 4.5 The measured negative, and what it licenses

The one fusion experiment the serving path has run is a nine-dispatch graph replacing the stages
after attention, and it was **slower**: 5.97 s against 4.52 s ([qwen-inference](qwen-inference.md)).
Two readings, both useful:

- **Launch fusion is not a throughput lever here.** The step already pays one submission and one
  fence per stage under `Policy::Batch`; composing stages into one graph removes submissions, and
  the measurement says submissions are not where the seconds are. What the composed graph adds is
  its own scheduling and argument marshalling.
- **The numbers that would move are the fetches and the bytes**, not the launches. A stage chain
  that keeps its intermediates resident and never brings them to the host is a different experiment,
  and §7's first slice is exactly it.

## 5. Scheduling

### 5.1 The request is not the unit

Orca introduced iteration-level scheduling: a batch is formed every iteration from the requests
that are live, a finished request leaves without draining the batch, and a waiting one joins
without a restart. Every production stack does this now, under the name continuous batching.
lichen-serve does it too: `Batched` holds the live rows, `retire` moves the last row into the gap,
and `admit` joins a new row at the next step (`qwen.rs`; measured in [qwen-inference](qwen-inference.md)).
The measured consequence is §2's: at eight rows the step is at its floor, and a ninth client only
queues.

### 5.2 The byte is not the unit

PagedAttention's contribution is an indirection. A sequence's cache is a list of fixed-size blocks;
a block table maps a logical position to a physical block; allocation is on demand and sharing is
free. The measured numbers are about waste (a few per cent against 60–80% for a naive reservation
in the paper's workloads), and the mechanism is a gather.

lichen can already address through a buffer (`y[i] = x[tbl[i]]`), and [inference-service](inference-service.md) §6
argues that paging changes only the address arithmetic inside the kernel: the step keeps its ABI,
the cache stays resident, and a page table swaps in behind it. What paging needs from lichen is the
**write side**: an append that writes a page rather than rewriting the whole cache, which is
either an output alias (backend decision, §6 of the same note) or a per-page copy.

### 5.3 The prompt is not the unit

[Chunked prefill](https://arxiv.org/abs/2403.02310) splits a prompt into roughly equal chunks,
runs one chunk a step, and keeps the decodes in the same batch. The two wins are that a long
prompt no longer starves the decoders, and that a prompt's time-to-first-token stops being
`prompt length × step time`. Its cost is a prefill kernel that can resume at a position, which is
a shape question: the chunk's queries attend over the cache written by the previous chunks, and the
chunk's tokens are a *block* of queries rather than one.

lichen's current shape is the degenerate chunk of one: `Batched::step` feeds a filling row one
prompt token a step. It is stall-free (the decoders ride the same step) but it makes a prompt of a
thousand tokens a thousand steps before its first word. The frontier's chunk needs a query block,
a causal mask inside the chunk, and a prefill-shaped projection; that is why §7 puts it behind the
tiled projection rather than beside it.

### 5.4 The replica is not the unit

Disaggregation (DistServe, Mooncake, Splitwise) runs prefill and decode on separate workers,
because their optimal batch shapes and their bottlenecks differ, and a decode's latency is ruined
by a long prefill sharing its device. Mooncake adds a KV-cache-centric store and reports serving
real traces at a fraction of the hardware; DistServe optimises goodput under TTFT/TPOT SLOs. The
mechanism is a KV transfer, and the premise is two devices.

This machine has one 8 GB card, so disaggregation is **not a slice here**; it is a reason the
single-device scheduler must approximate its effect (chunking, priorities), not a thing to build.

### 5.5 The token is not the unit

Speculative decoding drafts `k` tokens with a cheap model or head and verifies them with one target
pass, accepting the longest correct prefix. Medusa adds heads to the target; EAGLE-3 predicts tokens
directly and fuses multi-layer features, reporting up to 6.5x on chat and reasoning models and 1.38x
throughput at batch 64 in SGLang ([2503.01840](https://arxiv.org/abs/2503.01840)). The cost is a
draft model (or heads) and a verification pass whose work is the batch's, not the request's.

For lichen the premise is missing twice: there is no draft checkpoint, and the step's bottleneck is
its host round trips rather than its arithmetic — a speculative step would still pay them. It is a
later slice, not an early one.

### 5.6 The call is not the unit

CUDA graphs capture a sequence of launches and replay it with one submission. lichen's
`Policy::Batch` is the same idea one layer up: a recorded graph's dispatches go into one submission
and one fence wait, measured at 2 submissions/2 fences against `Async`'s 4/5 for two nodes and
16/30 for sixteen ([inference-service](inference-service.md)). The measured saving on identical work
was 1.11 ms of a 2.66 ms step (41.7%), and the 64-dispatch ceiling is a descriptor-pool size, not an
architecture ([inference-service](inference-service.md) §5). So the launch unit is **already
captured**; what remains is to stop paying a separate submission for every readback, which is a
property of the caller's loop, not of the graph.

### 5.7 The prefix is not the unit (a decision, recorded)

SGLang's RadixAttention keeps a radix tree of token sequences and reuses a shared prefix's KV cache,
which is the single largest win for chat workloads with a fixed template. It was designed, presented
and **deferred by the user**: *"先不做缓存，去做别的"* ([qwen-inference](qwen-inference.md), "What was
decided"). The deferral stands; this note records it as a decision rather than an omission, and the
roadmap does not re-open it. The measured fact behind it is in the same note: a repeated 2 000-token
conversation is 200 s of prefill every time it is sent, so the day the deferral is revisited, the
payoff is real and the design is already on record.

### 5.8 Policies: what a scheduler can still do here

| policy | the frontier's version | what lichen's numbers say |
|---|---|---|
| FCFS, iteration-level | the default (Orca) | landed |
| chunked prefill | Sarathi-Serve | missing; `Batched`'s chunk is one |
| priority / SLA | vLLM priorities, DistServe goodput | not needed at eight clients |
| shortest-remaining-first | SRPT-style | a queue of eight makes it moot |
| multi-queue / MLFQ | Llumnix-style migration, [2406.03243](https://arxiv.org/abs/2406.03243) | a second device would be needed |
| prefix-aware routing | SGLang radix tree | deferred by decision |
| admission control | the measured 503 past 64 waiting | landed (the queue refuses rather than drops) |

The scheduler's own reading is therefore short: **the only policy with a measured deficit is the
prompt's unit**, and it is gated on the prefill kernel, not on a queue policy.

## 6. The map

`status` is the tree's, not the paper's: **landed** means measured here, **partial** means the
mechanism exists below the surface, **missing** means neither.

| technique | the unit | lichen mechanism | status | payoff on §2 |
|---|---|---|---|---|
| region fusion (chain/fan-in) | the chain | claims, region merge pass | **landed** | small: the stages are already few |
| launch fusion | the launch | `Policy::Batch`, recorded step | **landed** | measured: 41.7% of a synthetic step, none of a composed real one |
| elementwise+reduction tile | the tile | staging plan, `Workgroup`, barrier | **partial** (plan only) | the 0.25 s around the stages, if fused |
| FlashAttention-shaped attention | the tile | the above + a query block + a page/stride read | **missing** | 4 dispatches → 1, and the score buffer's traffic |
| tiled GEMM (prefill) | the tile | the above + a split-`K` plan + runtime extent | **missing** | the prefill's arithmetic intensity |
| paged KV | the page | indirect read + a write path (alias or copy) | **partial** (read proven) | long context, fragmentation, sharing |
| continuous batching | the iteration | `Batched` + queue | **landed** | the batch's marginal cost already 0 |
| chunked prefill | the chunk | needs the prefill kernel + a resumable position | **missing** | TTFT of a long prompt |
| speculative decoding | the draft | none | **missing** | step count, gated on a draft model |
| prefix caching | the prefix | none (deferred) | **deferred by decision** | repeated prefix's prefill |
| disaggregation | the replica | none (one card) | **not applicable** | — |
| quantised weights / KV | the byte | `BFloat16` class; no narrower pack | **partial** | bytes of every layer read |
| resident session state | the step | `RunArgument::Resident`, `place` | **landed** for weights, **not for stages** | §7's first slice |
| one readback a step | the step | `Prepared::step` + `Session::fetch` | **landed** | measured 2 submissions against 4 |

## 7. The roadmap, in the order the evidence puts it

Each slice names its prerequisite, the measurement that decides whether it paid, and the exit
criterion. The order is by measured payoff per unit of new mechanism, not by frontier fashion.

### S1 — one fetch a token, not one a stage (this round's build)

**What.** `Qwen::layer_batch` currently runs 15 stage calls a layer, and every one of them fetches
its answer to the host, releases it, and uploads the result to the next stage. The intermediates are
kilobytes; the round trip is a submission and a fence, and the measured share around the stages is
**0.25 s of a 0.708 s step**. Pass each stage's `RunResult::Resident` to the next stage's
`RunArgument::Resident` instead, release it when its last consumer has run, and fetch once — at the
layer's end or, better, once a token.

**Prerequisite.** None beyond the resident channel, which exists: `RunArgument::Resident`,
`Session::fetch`, `Session::release`, and `Prepared::step` leaving the result resident.

**Measurement.** The same prompt's tokens must be identical, and the step's wall time and submission
count must fall. The control is the same binary with the chaining off.

**Exit.** Tokens unchanged; submissions per step down by the number of stages that no longer fetch;
step time reduced; the model still answers the checkpoint's own tokens.

### S2 — a row's rotation in one dispatch, and the batch's attention in one dispatch

`layer_batch` loops the rotation over sequences (because the rotation names one position) and the
attention over sequences (because each cache is its own). A positions buffer and a
`sequence × cache` stride would make each one dispatch for the whole batch. At eight rows this
removes roughly seven of eight rotation calls and seven of eight attention calls a layer; the
rotation is three dispatches and the attention four, so the saving is on the order of 50 dispatches
a layer, 1 400 a token — more than the whole step's floor today.

**Prerequisite.** `ROPE_STEP` reads a position per row (a one-line kernel change: a `positions`
buffer indexed by the row), and `ATTEND_CACHE_STEP` takes a per-row length and stride. Both are
buffer-map shapes the surface already has.

### S3 — a tiled projection, with a runtime extent

The tiling pass already fires for a literal extent and a matching identity's class; a hand-declared
plan already stages a reduction on the device. The missing pieces are (a) a plan that accepts a
runtime extent by launching over a fixed number of tiles and masking the last, and (b) the
argument list of the rewritten graph, which today does not match the program the caller wrote.
This is the single change that unblocks the prefill and the FlashAttention shape.

**Prerequisite.** [shared-memory-and-barriers](shared-memory-and-barriers.md) §9's surface; the
quantities are the plan's lane-group width and a `Count` the plan can divide.

### S4 — paged KV

The read side is proven; the write side is the contract's output placement (an alias) or a per-page
copy, and the decision between them is [inference-service](inference-service.md) §6's. Its payoff
here is not throughput at eight short sequences; it is the context a 6 GB card can hold and the
fragmentation a variable-length batch leaves.

### S5 — chunked prefill

Behind S3: a query block, a causal mask inside the block, and a scheduler that gives a chunk a
step. The measurement is time-to-first-token at prompt lengths 32, 256 and 2 048, and the decoders'
inter-token time under the same load.

### S6 — a fused attention

Behind S3 and S4: the online softmax tile with a paged key read. The measurement is the step's
dispatch count and wall time at context 2 048, against the four-dispatch path.

### S7 — a narrower weight class

FP8 or INT4 weight-only, with a dequantising read and a per-block scale. This is the one slice whose
payoff is not gated on the host loop: a layer reads half or a quarter of the bytes, and the card
fits a longer context. It needs a `ScalarClass` variant, a pack that states it, and an emitter that
reads it — the same shape as the bf16 pack, one class down.

### S8 — speculation, prefix caching

Deferred, for the reasons recorded: no draft checkpoint; the user's decision on prefix caching. The
note does not re-open either.

## 8. What this survey rejects

- **An operator vocabulary as the fusion substrate.** Rejected in [graph-fusion](graph-fusion.md)
  before this survey, and the field's attention variants are the evidence.
- **Graph-level composition as a performance lever.** Measured slower here (5.97 s against 4.52 s);
  the launch is already one submission under `Batch`, and the seconds are in the readbacks and the
  bytes.
- **Fusion that does not touch the tile.** A chain merge saves a buffer; it does not change the
  parallelism of a reduction, which is what makes a projection or an attention slow.
- **Copying the disaggregation, tensor-parallel or pipeline-parallel playbooks onto one card.**
  They are multi-device mechanisms; on this machine they are a way to be slower with more code.
- **Speculation before the step is cheap.** The draft's acceptance rate does not fix a step whose
  cost is host round trips.
- **Prefix caching, for now.** The user's decision, not an oversight; the design is on record and
  the payoff is quantified.

## 9. Sources

Papers and systems named above, in the order they appear:

- Orca: *A Distributed Serving System for Transformer-Based Generative Models*, OSDI 2022 —
  [USENIX](https://www.usenix.org/conference/osdi22/presentation/yu).
- *Efficient Memory Management for Large Language Model Serving with PagedAttention* —
  [2309.06180](https://arxiv.org/abs/2309.06180).
- *Taming Throughput-Latency Tradeoff in LLM Inference with Sarathi-Serve* —
  [2403.02310](https://arxiv.org/abs/2403.02310).
- *DistServe: Disaggregating Prefill and Decoding for Goodput-optimized LLM Serving* —
  [2401.09670](https://arxiv.org/abs/2401.09670); *Mooncake* —
  [2407.00079](https://arxiv.org/abs/2407.00079); *Splitwise* —
  [2311.18677](https://arxiv.org/abs/2311.18677).
- *Fast Inference from Transformers via Speculative Decoding* —
  [2211.17192](https://arxiv.org/abs/2211.17192); *Medusa* —
  [2401.10774](https://arxiv.org/abs/2401.10774); *EAGLE-3* —
  [2503.01840](https://arxiv.org/abs/2503.01840).
- *FlashAttention* — [2205.14135](https://arxiv.org/abs/2205.14135); *FlashAttention-2* —
  [2307.08691](https://arxiv.org/abs/2307.08691); *FlashAttention-3* —
  [2407.08608](https://arxiv.org/abs/2407.08608).
- *SGLang: Efficient Execution of Structured Language Model Programs* (RadixAttention) —
  [2312.07104](https://arxiv.org/abs/2312.07104).
- *GPTQ* — [2210.17323](https://arxiv.org/abs/2210.17323); *AWQ* —
  [2306.00978](https://arxiv.org/abs/2306.00978); *KIVI* —
  [2402.02750](https://arxiv.org/abs/2402.02750); *DeepSeek-V3* —
  [2412.19437](https://arxiv.org/abs/2412.19437).
- *GQA* — [2305.13245](https://arxiv.org/abs/2305.13245); *DeepSeek-V2 (MLA)* —
  [2405.04434](https://arxiv.org/abs/2405.04434); *Native Sparse Attention* —
  [2502.11089](https://arxiv.org/abs/2502.11089); *Mamba-2* —
  [2405.21060](https://arxiv.org/abs/2405.21060).
- *Qwen3 Technical Report* — [2505.09388](https://arxiv.org/abs/2505.09388).
- *FlexAttention / Flashlight* — [2511.02043](https://arxiv.org/abs/2511.02043);
  *FlashInfer* — [2501.01005](https://arxiv.org/abs/2501.01005);
  *ThunderKittens* — [2410.20399](https://arxiv.org/abs/2410.20399).
- *Llumnix: Dynamic Scheduling for Large Language Model Serving* —
  [2406.03243](https://arxiv.org/abs/2406.03243).
- Triton, TVM, MLIR, XLA and CUDA graphs are cited by their systems rather than by a paper:
  [Triton](https://github.com/triton-lang/triton), [TVM](https://arxiv.org/abs/1802.04799),
  [MLIR](https://arxiv.org/abs/2002.11054), [OpenXLA](https://openxla.org/xla),
  [CUDA graphs](https://docs.nvidia.com/cuda/cuda-c-programming-guide/index.html#cuda-graphs).

## 10. What this round lands

The survey was written first, and reading the serving path against the current surface turned up a
**stale model before any new feature could be measured**: `crates/lichen-serve/src/model.rs` was
written against the **superseded `.out`-as-a-parameter-group** kernel I/O surface
([kernel-buffer-declarations](kernel-buffer-declarations.md) supersedes it). Its buffer fields were
spelled `compute.Buf Float`, and its kernels declared `.out` in the parameter struct and wrote
through `k.out.<field>`. At this tip (a641ff3e) neither spelling compiles: every one of the
thirteen model programs refused to prepare — the whole serving path stopped before its first token,
and nothing in the tree noticed because the serving tests that need the checkpoint are not run by
default.

The migration landed with this note, and it is the round's prerequisite rather than its goal:

- a buffer's type is `(compute.Buf ((compute.BufParam _)(.type T, .length _)))`;
- a kernel's parameter carries **only inputs**;
- the body declares its output with `compute.buf`, writes it, and **returns the write's value** in
  the output struct — the surface's rule that a write is the value of an output position, not an
  effect ([code-audit D17](code-audit.md));
- a two-buffer kernel returns both writes in one struct.

All thirteen programs prepare again, with the node counts the shipped steps have:

| step | nodes | step | nodes |
|---|---|---|---|
| `RMS_NORM_STEP` | 2 | `DECODE_STEP` | 3 |
| `MATVEC_STEP` / `_BATCH` / `_ROW` | 2 | `PREFILL_STEP` | 2 |
| `ROPE_STEP` | 2 | `SAMPLE_STEP` | 7 |
| `ATTEND_STEP` | 3 | `LAYER_STEP` / `BLOCK` | 13 |
| `ATTEND_CACHE_STEP` | 4 | `SWIGLU_STEP` | 2 |

The surface migration changes no arithmetic and no dispatch count: a declared output buffer is the
same allocation the old parameter group's `.out` field named. What it restores is the ability to
measure, and the model answers the checkpoint's own tokens again (recorded in
[qwen-inference](qwen-inference.md)).

**S1 is the next build**, because it is the largest measured share that needs no new surface, and
its verification is the model's own tokens. The checkpoint is restored by `scripts/dump_bf16.py`,
which flattens the public safetensors shards into the loader's layout — the layout is the
`SHAPES` contract in `qwen.rs`, and the two must move together.

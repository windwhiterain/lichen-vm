# The serving performance boundary, measured

This note is the measured edge of what the current stack does, and the list of framework
capabilities each edge asks for. It is not a roadmap by taste: every item names the probe, the
number, and the feature the number is short of. [serving-frontier](serving-frontier.md) is the
survey and the map; this is the ruler.

## The three probes

- `token_cost` — one stream, two words, the per-stage split.
- `batch_cost` — eight rows at one position, the same split.
- `prefill_cost` — time to the first answer at 8, 32 and 128 prompt tokens.

All on the RTX 3060 Ti, `Narrowed` weights held in host memory, model from `LICHEN_MODEL_DIR`.

## What a token costs

Four tokens of a single stream: **2.324 s inside the stages of 2.402 s wall** (97%), a warm token
**0.54 s**. The split:

| stage | time | share |
|---|---|---|
| projection | 1.664 s | **69%** |
| head | 0.380 s | **16%** |
| norm | 0.200 s | 8% |
| rotation | 0.035 s | 1.5% |
| attention | 0.029 s | 1.2% |
| product | 0.015 s | 0.6% |

A token is **420 steps** (15 a layer, 28 layers), so a step is **~1.2 ms** and the projections are
196 of them. The weights a token reads are 2.8 GB at **~6.7 GB/s**, against the **85 GB/s** the same
kernel reaches when its reads are many ([qwen-inference](qwen-inference.md)). The head reads its
622 MB at ~6.5 GB/s.

## What the edges are

1. **The step, not the node, is the unit of cost.** Submissions fell 64% from the S2 work for 15%
   of the clock; 280 fewer nodes (S1b) bought 3%; and a projection split into eight partial folds
   plus a sum — correct tokens, one more node a step — read **1.658 s against 1.664 s**, no change.
   A step is ~1.2 ms whatever it contains, and 420 of them are the token. The capability under it
   is a **step that holds a whole layer**: the language and the recorder already build such a step
   (`LAYER_STEP`, `BLOCK`), so this is a serving-side composition, not a new primitive — except for
   the class rule below.
2. **A dispatch reads its buffers at one class.** The rope batch step proved it: with bf16 values
   and float positions, the positions were read as bf16. That rule is what blocks a composed layer
   today: its intermediates are float while its weights are bf16, and one dispatch cannot read both
   at their own classes. The capability is **per-buffer classes in one dispatch** (or a
   class-homogeneous graph), and it is the gate on composition.
3. **A runtime extent keeps the projection off the tiling pass.** The projection is 69% of a token
   and runs at 6.7 GB/s because each lane folds 2048 elements serially; splitting the fold did not
   move it, so the cost is the per-lane serial read, which a workgroup tiling would cut. The tiling
   pass fires for a **literal** extent and a matching identity's class, and the projection's extents
   are arguments. The capability is **tiling at a runtime extent** ([serving-frontier](serving-frontier.md) S3).
4. **Prefill is serial per position.** Time to the first answer is **4.3 s at 8 prompt tokens, 15.3 s
   at 32, 60.0 s at 128** — 0.47 s a token, the same layer pass repeated per position (8/32/128
   steps, 5 156/20 612/82 436 submissions). The capability is a **layer pass over a block of
   positions** with a causal attention: the projection already takes rows; the attention does not
   take a block, and the serve does not call it with one.
5. **The KV cache is host memory, re-uploaded every layer every token.** Each attention step copies
   its whole cache into an argument. At the four positions measured that is invisible; at 2 048 it
   is 8 MB a row a layer — 64 MB a token a layer at eight rows. The capability is a **resident or
   paged cache**, written by the append and read by the attention in place
   ([serving-frontier](serving-frontier.md) S4).
6. **The head is a full 622 MB read a token.** It is 16% of the token and grows with the vocabulary,
   not with the context. Greedy sampling could take a two-stage pass (a top-k over blocks, then the
   full logits of the survivors), which is a serving-side feature and not a framework one.

## The list, ranked by measured share

| # | capability | edge it moves | measured now |
|---|---|---|---|
| 1 | per-buffer classes in a dispatch | composed steps (edge 1) | 420 steps a token |
| 2 | tiling at a runtime extent | the projection (edge 3) | 1.664 s of 2.324 s |
| 3 | layer pass over a prompt block + causal attention | prefill (edge 4) | 0.47 s a position |
| 4 | resident or paged KV | long context (edge 5) | 8 MB a row a layer |
| 5 | two-stage head | the vocabulary read (edge 6) | 0.380 s of 2.324 s |

## What is already at its floor

The dispatch count is not the lever any more: the rotation and attention batching cut 64% of the
submissions for 15% of the clock, and the readbacks are 3%. **No further dispatch-count work is
worth doing** before edges 1 and 2 move.

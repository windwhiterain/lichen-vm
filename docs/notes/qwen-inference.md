# Qwen3-1.7B on the device: the first real tensor

> Status: **first milestone.** One real stage of one real model runs on the device and matches the
> host. Everything else is ahead.

## The model

`Qwen3ForCausalLM`, 28 layers, `hidden_size` 2048, `intermediate_size` 6144, **16 query heads over 8
key/value heads** (GQA), `head_dim` 128, RoPE at `theta = 1e6` with no scaling, SwiGLU, tied
embeddings, `vocab_size` 151936, bf16 on disk. Its weights are **4.06 GB** in two shards
(`3,441,185,608` + `622,329,984` bytes, both matching their manifest byte for byte), which is
**8.13 GB at f32** — more than the 8 GiB of the card it is meant for, so the model is streamed **a
layer at a time** rather than held resident.

Every tensor name was read from the shipped index, so the architecture above is the file's, not a
recollection: each layer carries `input_layernorm`, `self_attn.{q,k,v,o}_proj`, **`self_attn.{q,k}_norm`**
(a per-head norm), `post_attention_layernorm` and `mlp.{gate,up,down}_proj`.

## The reference

`.model/read_tensor.py` reads a safetensors shard directly — eight bytes of header length, the JSON
header, then the data — and converts bf16 by the shift it is: the top sixteen bits of the f32. It
computes the host's answer in **f64**, rounded once.

Measured on row `9707` of `model.embed_tokens.weight` with layer 0's real gain: the row's root mean
square is `0.032053217`, and the normed row answers `-0.023084413, 0.026700413, -0.072866450, …`
summing to `1.907063760`.

## The kernel, and what it proves

`RMS_NORM_STEP` is two kernels, because one lane cannot reduce a row and scale it at once: a single lane
folds the row's squares in f32, and one lane per element divides by the root of their mean. Measured:

| input | device | host | difference |
|---|---|---|---|
| the deterministic 2048-wide pattern | `0.0070658587` at element 1 | `0.007065859` | below `1e-9` |
| **row 9707 of the real embedding** | `-0.023084456` at element 0 | `-0.023084413` | `4.3e-8` |

So a real model tensor reaches a device kernel and comes back the host's number, at the model's own
width — 2048, not a toy.

## The projection, which is every projection

`MATVEC_STEP` is `y[j] = sum over i of x[i] · w[j][i]`: a lane is one row of `w` and folds the whole
width. That is the shape of **every** projection a layer has — `q`, `k`, `v`, `o`, `gate`, `up`, `down`,
and the 151936-wide vocabulary — so one kernel covers them with the row count and the width as the
caller's. Measured on 2048 rows over 2048 inputs with a pattern that is exact in f32, so a reordered sum
is the only difference available: rows `0`, `1` and `2047` answer `-0.5`, `-1.84375` and `-1.84375`,
which are the host's own dot products.

## Getting the weights, measured

Four things cost a round and are worth not re-deriving:

| | |
|---|---|
| **Never park the files in a worktree.** | `git worktree remove --force` deletes what is inside it, and 3.78 GiB of downloads went with the worktree that held them. They live in `<checkout>/.model/`, which `.gitignore` names, and the tests read `LICHEN_MODEL_DIR` first. |
| **`Start-Job` is not a background job.** | It is a child of the `pwsh` the harness starts, so it dies when that call returns — the file froze at the byte the call ended on. Only the harness's own background call persists. |
| **Sequential 16 MiB ranges are the way; the chunk size matters more than the connection count.** | Eight concurrent ranged `curl`s sat at zero bytes for seventy seconds, and 64 MiB ranges stalled at the same offset twice. Sixteen-MiB ranges fetched the same host at **7.1 MB/s** and finished both shards (3,441,185,608 and 622,329,984 bytes, each matching its manifest). Everything else was noise. |
| **Do not kill `curl` while diagnosing.** | Every cleanup that stopped the processes also stopped the transfer: the same shard reached 117 MB, then 175 MB, then 106 MB on three attempts, and each of those was a transfer this session ended. |

The CDN itself is fine: `curl -I -L` reaches a `302` to `cdn-lfs-cn-1.modelscope.cn` and answers
`Content-Length: 3441185608`, and a single ranged read of the same URL returns `206`.

## A whole layer answers the host

Layer 0 of the real model, one token at position 5, from the model's own twelve tensors: **the composed
path answers the host's hidden state**, element by element within `1.6e-6` and summing to `64.155712`
where the host sums to `64.155605`. It is composed of the steps that each already answer the host — the
norm, the projection, the SwiGLU product — plus the grouped spread between the value and the output
projection, where eight key heads serve sixteen queries two to one.

Two measurements of the interface came out of it, both committed as tests:

| | |
|---|---|
| **A struct's fields arrive in descending name order.** | `{.a, .b, .c}` passed `1, 2, 3` answers `321` — `abi_order`. |
| **A struct of twelve buffer fields binds two of them.** | The same probe with the layer's own twelve field names answers with only `.x` and `.k` bound; the other ten read zero. So the whole layer cannot be **one** step yet, and the fused program is ignored rather than deleted, so the limit stays visible. |

## Attention over a cache

`ATTEND_STEP` is the first place the query and the key decide anything: every query head scores the
cached positions against its **grouped** key head (sixteen queries over eight keys, so two share), the
scores become a softmax, and the answer mixes the values by it. Measured over two positions, where the
queries make the softmax sharp — the first head weights them `0.998` against `0.002` — the answer is the
host's within `1e-5`, and both sides sum to `82.973585`.

The same attention inside a whole layer, over two tokens of the real model, answers the host''s hidden
state within `1.1e-6`: the second token''s query reads both cached positions, so this is the first test
that fails when the query, the key, the rotation or the grouping is wrong.

That closes the last shape a layer needs.

## The head, and a token

The vocabulary projection is the tied embedding matrix, so a lane is one of the model's 151936 entries
folding the hidden width. Measured against the host's own dot products over the same two inputs, the
logits agree within `1.1e-6` and the largest is the **same entry** on both sides (9708, `3.0415757`
against the host's `3.0415742`) — which is the whole of a greedy sampler: the head names the next token.

## The whole model, and a token

Twenty-eight layers, streamed a layer at a time because the whole model is 8.13 GB at f32 and the card is
smaller, answer the host's logits within `2.7e-4` and name the **same token**: entry `25`, `12.312286`
against the host's `12.312018`. Per-layer the two agree to five decimals — `0.545759` against `0.545759`,
`335.892` against `335.892`, `66.373` against `66.375`.

That agreement also settles a question the numbers raised: the hidden state's root mean square climbs to
**335** in the middle layers and drops to 66 at the end. Both implementations do it identically, so it is
the model's own behaviour — the "massive activations" these checkpoints are known for — not an error.

A forward is 171 s on this machine, which is the number to attack next.

## Generation

Two positions generated greedily through the whole model, the second reading the cache the first filled,
name the host's tokens: `[25, 358]` on both sides, the logits within `2.7e-4`. Starting from the row
`9707` the loop produces `9707 → 25 → 358`, and the second token is the one that proves the cache is
read: a loop that attended only to the new position would name something else.

A property of the rotation falls out of the same run and is worth recording: at **one** position the
answer does not depend on the position at all — `0.545759` and `12.312018` are identical at positions 0
and 5 — because the rotate-half form turns every head's key and query by the same angle, and a dot
product is invariant under that.

`ATTEND_CACHE_STEP` makes the cache's length the caller's: one dispatch scores every `(head, position)`
pair by folding the head's width, one sums each head's exponentials by folding the positions, one divides
them into weights, and one mixes the cached values by folding the positions again. Measured over three
positions, where the third carries a weight of `2.8e-6` — small enough that dropping it would hide in a
loose check, which is why the last head's values near `-0.76` are asserted too — the answer is the host's
within `1e-5`.

## What a token costs, and where

A token costs **167 s**, and the cost is **reading the weights off disk**, not the upload and not the
arithmetic. The first measurement of this was 33 MB/s across the vocabulary head (1.24 GB in 38 s) and a
forward (5.4 GB in 171 s), and it was read as an upload rate; a projection at the model's own width, whose
matrix was already in memory, moves 16 MB in **0.01 s** — a gigabyte a second — so the rate was the disk's
and what a token pays is 28 file reads of 192 MB each.

That correction changes what to fix. The host has 15.6 GB with about 5 GB free, so the weights at f32
(6.6 GB) do not fit in memory, but **at bf16 they are 3.3 GB and do**. They are bf16 on disk already, so
the shape of the fix is to read them once, hold the halves, and upload those.

That makes the next step a memory question rather than a kernel one. The weights are 5.4 GB at f32, and at
bf16 they are 2.7 GB, which is what the
precision path exists for. Reaching it needs two things the interface does not have yet, both measured:

| | |
|---|---|
| **A pack's class must reach a node from the serve path.** | `pack_placement`'s six tests read the pass report through `module.extension_diagnostics`, and they pass; the same channel came back empty through `Session::prepare_with_packs`. The reason is where the lines are recorded: `compute.rs:5680` fills them from `take_graph_pass_lines` on the **graph-run** path, and every serve program ends in `compute.graph step`, which records a graph and runs nothing. `pack_placement`'s program ends in `compute.graphrun` with `compute.collect`, which is what forces the run. So a `compute.graph` tail can never report, and the accessor that would have carried it was reverted rather than shipped returning nothing. |
| **A bf16 result must be readable.** | *This was wrong: it already is.* `dispatch.rs`'s read-back widens a `BFloat16` buffer's halves to `f32` and answers `ScalarData::Float`, so nothing is missing there. What is missing is a class per buffer in the emitter: the artifact writer recomputes a node's output classes from its body, so a pack narrows the inputs and the node ends up mixed, and a mixed node answers wrongly rather than being refused. |

So the token rate is gated on the engine, not on the model: the layers, the cache and the loop are all
verified against the host.

## What a token costs now

Two factors, measured one at a time on the same token (`9707 -> 25` both ways, so the narrowed model keeps
the choice):

| build | width | one token |
|---|---|---|
| debug | f32 | 243 s |
| release | f32 | **13.4 s** |
| release | bf16 halves | **6.3 s** |

The build was the larger half: the f32 path's cost was a per-layer loop turning 200 MB of little-endian
words into `f32` in an unoptimised build, which the release profile does eighteen times faster. The width
is the other: the checkpoint's own halves are read and handed to the device **unconverted**, so a layer
costs its 96 MB read and no conversion at all, and the projection that consumes them is the one a pack
narrowed.

What is left per layer is three uploads of the weights at about 2 GB/s, twelve dispatches, and one 96 MB
read — and the dispatches are **not** the lever the count suggests: fusing the half after the attention
into one graph of nine dispatches was measured correct (the same tokens) and **slower**, 5.97 s against
4.52 s. So a submission and a fence cost less than a composed graph's own scheduling, and what a token
still pays is the bytes: the weights it hands over, and the file it no longer reads because the halves are
held in memory.

## What the token's seconds are

Read off per-stage timers on the held model, two tokens at half width:

| stage | two tokens | per token |
|---|---|---|
| the layer loop | 9.82 s | 4.91 s |
| — of which reading a layer's tensors | 5.02 s | 2.51 s |
| — of which reading an answer back | 0.13 s | 0.07 s |

So half of a layer's time is **making a copy of the file it already holds**: `Tensors::Narrowed` clones the
96 MB a layer is stored in, and `RunArgument::Buffer` takes its bytes by value, so the slice handed to a
dispatch is a second copy. 28 layers of that is 2.7 GB of `memcpy` a token, and it is the largest single
line in the budget.

Both copies are gone. A layer now **borrows** the copy the model keeps, which took a warm token from 4.82 s
to 2.64 s — the 2.2 s the timers had attributed to the clone almost exactly. And the second one was not an
interface change at all: `graph::run` took its arguments **by value** and then cloned each payload out of
them, so moving the bytes instead of copying them took the token to **1.61 s**. Two copies of a 96 MB file,
one line each, were worth three seconds.

The third copy was the last one: a payload is now **shared** (`Arc<[u8]>`) rather than owned, so the
eleven slices a layer is split into are handed to dispatches by reference count, and the token fell from
1.64 s to **1.18 s**. Sharing turned out to be cheaper than borrowing: a shared payload needs no lifetime
through `Value`, and the forty-one places that build one kept their own constructors.

What is left per layer is the upload of the weights it reads and the dispatches that read them.

## A weight can stay on the device

The card is **6 GB**, measured with `nvidia-smi`; the 4 GB this work believed for several rounds came from
WMI's `AdapterRAM`, a 32-bit field that saturates at 4 GiB on every larger card. What a card can actually
hold is not its size either — see [Where the card's memory went](#where-the-cards-memory-went) below.

The upload stopped being a token's cost the day a host could **place** bytes rather than copy them through a
program: `Session::place(bytes, class)` copies four bytes at a time — a copy does not care what the bytes
mean — and then restates the resident's extent and class, on the backend and on the handle the caller
holds. Twelve layers placed took a warm token from 1.18 s to **0.98 s**, and all twenty-eight fit the local
card once the runs stopped keeping what they did not return.

The restatement is the whole trick and it has two halves. A pack states a class for a node it matches, and
matching needs a **claim**, which a plain copy does not have — that is why a narrowed upload was not
expressible before. It did not need to be: the class of a placed buffer is not the copy program's to state,
it is the caller's to state after the copy. Both records must move together, because a fetch reads the
handle's count and the dispatcher reads the backend's, and one restated without the other is a buffer read
four bytes an element by whoever asked the wrong one.

### Where the card's memory went

The card is 5994 MiB of device-local heap (heap 0 of `vkGetPhysicalDeviceMemoryProperties`; heap 1 is
8002 MiB of host-visible memory and is where staging lives). For a long time the weights were measured as
needing far more of it than they do: twenty layers plus the head is 2.51 GB of tensors, and the process
drew **5929 MiB** — the whole card — for them.

Counting every device-local byte by the tag that allocated it (`crates/lichen-compute-gpu/src/accounting.rs`,
off atomics, so a report can be taken while a submission is being recorded) put the difference where it
belonged. `Session::place` runs the two-dispatch `UPLOAD` program, and **every dispatch's output is issued
as a resident id**; only the last one is the answer the caller keeps. The first dispatch's output is the
same size as the tensor it is copying, and nothing ever released it: `graph::run` returned the graph's own
returns and dropped the rest of the value table on the floor. So each `place` kept **two** tensors where it
needed one, and 20 layers plus the head showed 5027 MiB resident where 2514 MiB of weights were placed — the
missing 2513 MiB was one orphaned copy of every tensor, and the driver refused at 24 layers because the
request was twice the model.

`graph::run` now hands back every buffer the run produced that the caller did not bring in and that the
graph does not return. Twenty layers plus the head fall to **2514 MiB resident** and 3773 MiB of
device-local bytes in all, the other 1259 MiB of which is the pool, bounded at `RECYCLED_PER_SIZE` buffers
per size and class. What a `place` compiles is not part of it: the pipeline cache is keyed on content
digests, so 309 prepares of the same source leave **one** pipeline.

All twenty-eight layers plus the head now place on the 6 GB card: **3282 MiB** of weights resident, 1259 MiB
pooled, **4541 MiB** of device-local bytes in all against 5994 MiB of heap, and `nvidia-smi` peaks at
4771 MiB where it used to exhaust the card at twenty layers. `LICHEN_RESIDENT_LAYERS=28 LICHEN_ONLY_PLACED=1
cargo test --release -p lichen-serve --test resident_token` passes, still naming `[9707, 25, 358]`; the
tokens were 2.9 s and 5.3 s across six runs, which is the afternoon's weather and not a property of the
model — another session was on the card throughout.

## Why the projection is not tiled, in the pass's own words

The projection is prepared once and run at every shape, so its extent is a runtime `Count` — and that is
exactly what the tiling pass refuses, printed by `Prepared::report`:

    narrowed pass: node 0: pack "narrow" was refused: its launch extent is not readable here,
                   and a tiled fold is built for a whole number of tiles
    narrowed pass: node 0: the pack states BFloat16

So the fold was never spread across the workgroup, and the reading that held the fold responsible for the
per-row price was wrong for a reason the pass states better than any probe: a fold whose extent arrives at
run time cannot be cut into tiles at prepare time.

A second reading corrected the first. A projection whose **launch extent** is a literal, with its fold left
dynamic, prepares — and the launch-extent refusal is gone — but the pack is then refused for a second reason:

    node 0: pack "narrow" was refused: the reduction's combine is in BFloat16 and its identity in Float,
            and a fold's identity is the value its combine is applied to

which is the last link and a small one: the program states its identity as a `Float` literal while the pack
narrows the combine to `BFloat16`, so the two do not agree and the plan is refused before any tiling is
considered. Both conditions have to hold — a launch extent a tile can divide, and an identity of the class
the fold combines in — and neither alone is what this note first wrote down.

## What the hand-off probe could not settle

A chain of four projections, once through the host and once passing residents on: **0.004 s** against
**0.026 s**. Both are wrong in opposite directions and neither answers the question. The host chain's fetch
does not wait, so its four milliseconds are the submissions and not the copies; the device chain places five
buffers and releases them every repeat, which the model never does, and pays for that instead. That is the
third probe this note has written about the 0.250 s a step spends around its stages, and the honest reading
after all three is:

- the **ratios inside one run** are the only figures this machine can be trusted for — its card is shared
  with every other session on it — and the one that survived is the 35% a step spends around its stages;
- reproducing that 35% from outside the model needs the model's own composition, not a synthetic chain;
- and the direction that removes both the stages' dispatches and most of their hand-offs is the one the work
  now takes: a layer as one kernel, which needs the barrier the [staging table](shared-memory-and-barriers.md)
  leaves one row short of.



The stage calls are timed now, so a step can be split without reading a machine's clock from outside: inside
one step of one request, **0.458 s of 0.708 s** is inside `Prepared::step` and **0.250 s** is around it —
uploads, read-backs, tensor setup and allocation. That share is measured within one run, which is what makes
it worth more than the absolute numbers: the same probe's resident step read 0.576 s one round and 1.605 s the
next on a machine other sessions compile on, so every **absolute** figure in this note carries that spread
and the **ratios inside a run** do not.

What it means for the mega kernel: fusing a layer's ten stages into one removes the stages' own dispatch
count, which is the part this note has a floor for, and it does **not** by itself remove the 35% around them.
Both are wanted, and the one measured without any new language surface is the 0.250 s.



One dispatch with a row, so that nothing but the dispatch itself is being measured: **0.329 ms**, and the
same **0.33 ms** at 4096 rows. `Prepared::step` under `Policy::Batch` records every node into one submission
and waits once, so a stage is one submission and a step's 281 stages are 281 of them — which puts the floor
at **93 ms** for a step, against the **576 ms** a step actually takes with the layers resident. The six times
between them are not in a kernel: the projection reaches 85 GB/s when its reads are many, and the floor
does not move when they are few. They are in what the model does around each stage, and that is the one
thing this file has not yet measured from inside.

## The dispatch count, and what fusing it would take

A step makes **281 dispatches** — a layer is ten: three norm, four projection, two rope, one attention — and
the measured floor for one dispatch is about 0.4 ms, so the count alone is roughly 126 ms of a 576 ms step.
The user's direction for that is **kernel fusion, a mega kernel**: one dispatch a layer rather than ten,
which needs the stages to see each other's writes, which needs a barrier, which is the capability
[shared-memory-and-barriers](shared-memory-and-barriers.md) calls the largest single item in its axis and
whose staging table is the map of how far it has got.

That table, read rather than assumed — and then checked against the tree, which is the part that matters,
because the table's own step 1 is **stale**: it still lists the revert of the zero-width `State` token as
outstanding, and `ScalarClass` in the tree has three variants, `Int`, `Float` and `BFloat16`, with no `State`
among them. So every step of that staging has landed: steps 2, 3 and 4 (the lane-group width, the slice vocabulary with the barrier as
data, the barrier scan); step 5's GPU half landed on its own (a `Workgroup` variable per slot, an
`OpControlBarrier`, no change to the binding layout); step 7's placement landed at `77fafaf`; and step 8 --
a `Reduction` tiled across the workgroup rather than folded per lane -- landed at `d3f984f`. So the
capability is far enough along that the serving path **already installs** the tiling pass
(`crates/lichen-serve/src/lib.rs`), which is what makes the next reading interesting: the projection is
still per-lane, and the [staging table](shared-memory-and-barriers.md#9-the-staging) says why — the pass
replaces `Folding::per_lane`'s schedule, while a program that writes `fold (k.dim, …)` folds the whole
extent in every lane and cannot be tiled into something it does not already answer.

So the staging is complete where this note needs it, and it was then **run rather than read**: the spike is
not a new program, it is `crates/lichen-compute/tests/lichen_compute/plan_staging.rs`, which drives a
declared plan on a device, and the run answers

    lichen_compute::plan_staging::a_declared_plan_stages_the_reduction_and_answers_the_closed_form ... ok
    test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 250 filtered out

That is the capability this note needed a promise for: a slice written, kept in a workgroup, read back after
a barrier, and answered as the **closed form** rather than as the silent `0` the tree's own note records for
a read with no data path. The next step is therefore a real one rather than a feasibility question — a
projection written as a staged kernel, its lanes cooperating on one output row instead of walking the row
each on its own, measured against the projection this service ships today.

## What a burst is answered with

A request waits on a `oneshot` rather than a blocking receive, so a waiting handler yields its runtime thread
instead of holding it: with a blocking wait, sixteen waiters filled the pool and the rest of a burst was
dropped at the listener, forty-four of a hundred connections in the measurement that found it. With the
asynchronous wait the same hundred connections were answered **64 with 200 and 36 with 503**, and none were
dropped — the queue holds sixty-four, the worker takes eight into a batch, and what is left over is refused
with an explicit error rather than an empty answer. Before that, a full queue answered 200 with empty text,
which a client cannot tell from a model that said nothing.

A slow stream does not hold the batch either: each stream's channel is unbounded and its lifetime is the
tokens a request may produce, so a client that reads slowly costs its own buffer and not the worker.

## The service

`qwen-serve -- <port>` answers the OpenAI surface: `GET /v1/models`, `POST /v1/chat/completions` and
`POST /v1/completions`, each with or without `stream`. A chat request is templated the way the checkpoint was
tuned — `<|im_start|>role` then the content then `<|im_end|>` — encoded by the checkpoint's own tokenizer, and
answered with `temperature` mapped to the sampler's and `max_tokens` to the token count.

Two things decide the shape. **One worker owns the model**, because a generation is host-blocking calls on one
session and a second thread would share the device and its pool with it: requests queue, and a client's
position in the queue is its latency. And **a prompt fills the cache before any token is chosen** — a model
started from the prompt's last token alone answers as if the request were one word, which read as
`"
: 10"` until the prefill landed and as `"Hello! How can I assist"` after it.

A Chinese prompt works, and the way it failed is worth keeping: the byte table was GPT-2's, which names an
unprintable byte `256 + byte`, while this checkpoint's vocabulary numbers them in order from 256 and stops at
323 — so every continuation byte, which is most of a Chinese character's bytes, named a symbol no token has.
The encoder dropped the unknown symbol rather than refusing it, so the loss was silent and every ASCII row
still passed. A symbol the vocabulary lacks is now an error, a piece the split refuses is an error, and the
ids for `你好，世界` are checked against a second implementation reading the same vocabulary.

What the surface still does not have is true batching, because the batched head disagrees with the single one
at two rows while the batched layer agrees with it at every one of the twenty-eight.

### A batch's second row, and where it was lost

`Qwen::generate_batch` runs the sequences' rows in one dispatch wherever the step names no position and no
cache, and its test says so by running each start alone and comparing. With `STARTS = [9707]` it passed,
because **a single input row is the one case where the two index expressions agree**: `Qwen::projected`
passes `per` — the outputs one input row produces — and a launch of `rows = n · per` outputs therefore
sees `row / per = 0` for every row.

At two starts the second row was exactly zero, and it was **not** the kernel, the emitter's addressing, or
the scalar argument's arrival. A probe (`crates/lichen-serve/tests/probe_batch_row.rs`) runs
`MATVEC_BATCH_STEP` at `rows = 6`, `dim = 2`, `per = 3` and answers `[1, 2, 2, 3, 4, 6]` — through
`compute.plrun` and through `compute.graphrun` under the `BFloat16` pack alike — so `per` is read at the
value the host passed, and `MATVEC_STEP` over the same launch of six answers all six rows.

The weight was indexed by `row · dim`, which is only right while `row < per`. A batch shares **one** weight
across its sequences, and that weight carries `per` rows, so an output belongs to the weight row
`row − (row / per) · per` and to the input row `row / per`. Past the first sequence the weight read ran off
the end of the tensor and — an out-of-range read answering zero, as the probe's own short buffer showed —
the fold wrote its identity. `key`, `value`, `query`, `gate`, `up` and `down` all shared the one expression,
which is why the whole row went quiet rather than one stage of it.

**What it buys**, narrow and held in memory, warm, `generate_batch(&[9707, 785], 3)` against the two
`generate(.., 3)` calls it replaces, measured twice and in both orders: the batch **3.43 s, 3.62 s, 3.79 s,
4.71 s** and the two alone **6.77 s, 7.15 s, 7.35 s, 7.59 s** — **1.6× to 2.2×**, and the order of the pair
changes nothing. The batch pays each layer's weights once rather than once a sequence; the rotation and the
attention still name one position and one cache, so those loop per sequence. The tokens are
`[9707, 25, 358, 2776]` and `[785, 25, 330, 785]`, each what its start answers alone.

## A batch of different lengths

`Batched` holds requests at their own positions: `admit` fills a prompt's cache, `step` gives every live
request one token and advances it, `answer` reads back what a request has said. Two requests of different
prompt lengths, batched, answer exactly what each answers alone — that is `continuous_batch`'s assertion, and
it is what the single-position batch before it could not tell apart.

The barrier was smaller than it looked. The rotation reads one position per call, which looks like a
single-position kernel, but a batch already calls it once a sequence: the only thing that had to change was
`layer_batch` taking a position *per row* rather than one. The caches are held layer by layer across the
live requests, so a step is still one dispatch a layer, and retiring a request moves the last live one into
its place while keeping the identifier a caller holds.

What a step costs and what it buys: `batch_generate` over two sequences measured **1.6×–2.2×** against the
two runs it replaces, and the service's worker now groups its queue onto `Batched`, up to eight requests a
step, with a sampler per request rather than one per batch. Two requests sent together — `Hello` and
`The capital of France is`, four tokens each — were answered in **4 s** rather than the sum of their separate
runs, the second of them ending `Paris`.

## A baseline under load

Thirty-two requests of eight tokens each, at concurrency eight, measured through the service's own
histogram on an otherwise quiet machine: **p50 11.86 s, p95 and p99 33.55 s**, 256 tokens in **66 s** across
**35 batch steps** — about **3.9 tokens a second** in aggregate against the ~0.8 a single request gets alone.
The gap between p50 and p95 is the batch's own, and it is the thing to watch: a request waits for the batch
it joined, so the tail is the queue's depth rather than the model's speed.

The instrument cost three attempts to make honest. Its buckets were powers of two, then quarters of a power
of two over a ladder that ended at four seconds, so every request fell into the last bucket and every
quantile came back the same number. A ladder has to reach past the slowest request it means to describe: it
now runs from a microsecond to about seventy minutes in 128 rungs, and a quantile is the first rung whose
cumulative count reaches it.

## A soak, and what it says is not leaking

Two hundred requests of two tokens at concurrency eight, sampled after every fifty: **admitted 200, finished
200**, nothing live at the end, and the card's used memory **4752 → 4752 MiB** across all four batches. The
service holds no per-request state once an answer is delivered — the resident table, the batch's retirement
and the tokens' probabilities all give their memory back, which is what the earlier leak
([`graph::run`'s orphans](qwen-inference.md)) made worth measuring rather than assuming.

The distribution over those two hundred: **p50 2.49 s, p95 7.05 s, p99 28.22 s**.

## Why the weights by column took three tries

A projection reads its weight one row an output, so a lane's neighbours read addresses a whole row apart —
the access a device serves worst. Storing those weights by column makes the same read contiguous across
lanes, and the price is one transposition at load. Two attempts to land it disagreed with the host's tokens,
and the reason was found by comparing the shipped program against a program a test had already verified:
the address was right and the **weights were not all in the layout it named**.

The layers are not the model's only projection. The **vocabulary head** is a second consumer of the same
program, it is loaded from its own file rather than from a layer's, and it is read by `embed_row` a row at a
time — so transposing it costs 593 MB of host memory that a repeated conversation prefix would not have to
pay, and a global address change that does not account for it reads a different matrix. The shape that
works: the layers by column and the head by row, with the head running the row-major program the tree
already had and nothing else using.

What the layout buys, measured on the service with the same hundred requests at concurrency eight: a step
fell from **1.08 s to 0.86 s** and the aggregate rate rose from **4.76 to 5.88 tokens a second** — **1.24x**,
not the two orders of magnitude the card's unused bandwidth suggested. So the read the device served worst
was not the whole of the cost. What is left is the fold itself: every lane walks its weight's `dim` elements
one at a time, 2048 of them for most projections, and whether the reduction claim splits that walk across
lanes is the question the next measurement has to answer.

A projection on its own, measured with its weights **placed once and stepped over as residents**, which
is what the model does — the same program with a buffer handed in on every step measures the upload
instead, and did: 33.5 MB a run read as 28 ms for 4096 rows. Resident, the price per row falls away and the
**time stops falling**: 64 rows 0.366 ms, 256 rows 0.453 ms, 1024 rows 0.411 ms, 4096 rows 0.395 ms — flat at
about **0.4 ms a dispatch**, at 84.9 GB/s of reads when the rows are many, a quarter of the card. So the fold
is not where the tokens go: a step's 860 ms holds some **340 dispatches**, and at this floor they account for
about 136 ms. The rest is a per-stage question the next measurement has to ask, stage by stage.

The release a step's own buffers go through is not it either: the same probe keeping its answers and
letting them be released measures 0.43 ms against 0.43 ms. And the narrowed path is not slower than the
float one — 0.96 ms against 0.91 ms at 4096 rows.

The measurement has a floor that is not the device's, though, and this file has to say so: the same probe's
numbers moved by four between two runs — 0.42 ms against 0.9 to 3.5 ms for the same shapes — while other
sessions on this machine were compiling. So the per-dispatch price the model pays is **not established** to
be four times the probe's, which was the reading one round took: it is a spread this measurement cannot
separate from a busy host.

Where the weights live is worth **2.23x**, measured on one step of one request with the same program and the
same shapes: **1.283 s** with the layers held in host memory and handed over as host-visible buffers, and
**0.576 s** with them resident. That is the cheapest part of the answer so far, and it is also the reason
the numbers in this note disagreed with each other for several rounds: the serving binary places its layers,
so its step is 0.86 s over eight sequences, while the tests hold theirs, so their step is over a second for
one. A deployment that cannot keep every layer resident pays this ratio rather than failing, which is what a
smaller card would see.

Reading a result back is not it: the same probe fetching each answer adds **0.041 ms** to a dispatch of
0.4 ms, and at 4096 rows 0.060 ms. So the round trip a stage makes is cheap, and what is left is neither the
fold, nor the layout, nor the read-back. The next probe therefore belongs **inside** the model rather than
beside it: `StepCost` already carries the `submissions`, the `fence_waits` and the wall time of each run, so
one instrumented layer answers whether the 860 ms is dispatch count, fence waiting, or work this probe's
shapes never reproduced.



Two paths read two layouts, which is what the attempts finally settled on: a **held** layer is stored by
column and read by the column-major program, a layer **read from the file** is stored by row and read by
the row-major one, and the vocabulary head is always the latter because `embed_row` reads it a row at a
time. A single global address change cannot be right for both, and that is why the first two tries failed.

## Steady state, which is the number to compare against

The same service, fifty requests of warm-up thrown away and then a hundred measured by subtracting the
histogram's buckets before from its buckets after: **p50 and p95 both 3.53 s, p99 4.19 s**, 200 tokens in
42 s — **4.76 tokens a second** in aggregate. The tail is what changed: the cold run's p99 was **28.22 s** and
the warm one's is **4.19 s**, because a cold request waits for the driver's caches while a warm one waits only
for its batch. p50 and p95 landing in one bucket is the other half of the same fact — an eight-deep batch of
two-token requests is a steady queue, so the spread is the batch's step, not the model's variance.

 The tail is a handful of
requests from the first batch, before the driver's caches warm; the histogram cannot say which, and the
number to compare against is the same service under a steady load rather than after a cold start.

## What was decided, and what was not

The user's verdict on prefix caching, recorded because it rules out the next big optimisation for now:
**not yet** — "先不做缓存，去做别的". The design was presented as three shapes (whole-prompt exact, block
level, or the chat template's fixed head) with the recommendation of landing exact-plus-LRU first and
measuring a hit rate before paying for block matching, and the answer was to spend the time elsewhere. So a
repeated conversation prefix is prefilled again on every request, and that is a decision rather than an
oversight.

What came instead: a request may keep a nucleus of its candidates (`top_p`), and `/metrics` carries a
histogram of a request's wall time — buckets that count what a scraper reads cumulatively, a `_count` and a
`_sum` beside them, which is the shape a duration needs before any optimisation can be argued about.

## Running it

`crates/lichen-serve/src/qwen.rs` holds the model: the loader, the twenty-eight streamed layers, the
cache and the greedy loop. The binary is thin and takes the token to start from and how many to
generate:

```
cargo run -p lichen-serve --bin qwen-generate -- 9707 2
generating 2 token(s) after 9707
tokens: [9707, 25, 358]
  token 1 took 235.2 s
  token 2 took 285.4 s
520.7 s in all, 260.3 s per token
```

Those are the host's own tokens, and the test of the same name asserts them — through this module rather
than a copy of the loop, so the binary and the check cannot drift.

The model's attention is `ATTEND_CACHE_STEP`, whose position count is the cache's, so no length is a
special case: the first token's own softmax over one weight is the same dispatch as the eleventh's. The
test walks four positions and asserts the host's whole chain, `9707 → 25 → 358 → 2776 → 4460`, which
also pins the short length: the first two tokens are the ones the two-position step answered, so a
regression in either half fails the same assertion.

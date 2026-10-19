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
graph does not return. Twenty layers plus the head fall to **2514 MiB resident**, and the 629 MiB the pool
holds is the pool, bounded at `RECYCLED_PER_SIZE` buffers per size and class. What a `place` compiles is not
part of it: the pipeline cache is keyed on content digests, so 309 prepares of the same source leave
**one** pipeline.

All twenty-eight layers plus the head now place on the 6 GB card: **3282 MiB** of weights resident, 1259 MiB
pooled, **4541 MiB** of device-local bytes in all against 5994 MiB of heap, and `nvidia-smi` peaks at
4750 MiB where it used to exhaust the card at twenty layers. `LICHEN_RESIDENT_LAYERS=28 LICHEN_ONLY_PLACED=1
cargo test --release -p lichen-serve --test resident_token` passes, still naming `[9707, 25, 358]`, at
2.8 s and 3.0 s a token.

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

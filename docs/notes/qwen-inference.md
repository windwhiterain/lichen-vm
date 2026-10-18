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

That closes the last shape a layer needs. What remains is the loop around it: 28 layers in sequence, the
151936-wide vocabulary projection and sampling, and the precision path (built and merged, not yet used —
everything here runs at f32, which is why the weights are streamed).

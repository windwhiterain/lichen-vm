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

## What is ahead

The rest of a layer — RoPE, the GQA attention over a growing cache, the SwiGLU — then 28 layers, then
the 151936-wide vocabulary projection and sampling. The precision path is built and merged but not used
here: this milestone runs at f32, which is why the model is streamed.

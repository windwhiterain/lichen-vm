# lichen-serve

An OpenAI-compatible serving path over Qwen3-1.7B, running on this tree's own kernels: the model's
layers are lichen programs the language compiles to SPIR-V, and nothing here calls a framework.

## What it needs

The checkpoint's tensors, converted once, and the tokenizer:

| file, under `LICHEN_MODEL_DIR` | what it is |
|---|---|
| `tokenizer.json` | the checkpoint's own, 10.9 MB |
| `embed_tokens_weight_bf16.bin` | the vocabulary head, 593 MB, tied to the embedding |
| `norm_weight_bf16.bin` | the final norm |
| `layer00_bf16.bin` … `layer27_bf16.bin` | one layer a file, 96 MB each |

`.model/` in this repository holds them; `.model/dump_bf16.py` writes them from the safetensors checkpoint.
A `Width::Wide` model reads f32 files of the same names instead, which is 6.6 GB and only for checking an
answer against the half-width one.

## Building and running

```
cargo build --release -p lichen-serve --bin qwen-serve
LICHEN_MODEL_DIR=C:\resource\lichen-vm\.model ./target/release/qwen-serve 8099
```

It prints `the model is loaded; the queue is open` once the weights are on the device — 28 layers placed,
**4540 MiB of the card's 5994** — and then `listening on http://127.0.0.1:8099/v1`.

## What it answers

```
curl -s http://127.0.0.1:8099/v1/models

curl -s -X POST http://127.0.0.1:8099/v1/chat/completions -H "content-type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}],"max_tokens":6,"temperature":0}'

curl -s -N -X POST http://127.0.0.1:8099/v1/chat/completions -H "content-type: application/json" \
  -d '{"messages":[{"role":"user","content":"Hello"}],"max_tokens":6,"stream":true}'

curl -s http://127.0.0.1:8099/metrics
```

`/v1/completions` takes a `prompt` instead of `messages`. A request may state `temperature`, `top_k`,
`top_p`, `max_tokens`, `stream` and `stop` — one string, or an array of them; every other field is accepted
and ignored. An answer's `finish_reason` is `length` when it reached `max_tokens` and `stop` when it met its
`stop`, the checkpoint's own turn marker, or one of the strings the request named. A streamed answer ends
with a chunk carrying that `finish_reason` and then `data: [DONE]`.

## What it does behind a request

A queue in front of one worker, which owns the model. The worker groups up to eight requests into one batch
and advances them a position at a time, each request at its own position and under its own sampler; a long
prompt fills beside the requests already answering rather than holding them out. Sixty-four requests may
wait; a hundredth is refused with a `503` and an error object rather than dropped at the listener.

`/metrics` exposes what it did, in the exposition format:

| name | what it counts |
|---|---|
| `lichen_requests_admitted` | requests taken onto the queue |
| `lichen_requests_refused` | requests refused because the queue was full |
| `lichen_requests_finished` | requests answered to their end |
| `lichen_tokens_answered` | tokens answered |
| `lichen_batch_steps` | batch steps taken |
| `lichen_batch_live` | requests live in the batch, now |
| `lichen_step_nanoseconds` | time spent inside batch steps |

## What it has not got

- No prefix caching, so a repeated conversation prefix is prefilled again every request.
- No `n` and no `logprobs`.
- A `stop` string that spans two tokens is cut from the answer's text, but a streaming client may have
  received the tokens that spell it before the cut is decided.
- No keep-alive or HTTP/2 configuration: axum's defaults.
- The tokenizer is re-read at start and held; it is not shared between processes.

## Where the measurements are

`docs/notes/qwen-inference.md` is the record: what a token costs and why, what the weights do, where the
card's memory went, and how the batched and streaming paths were measured.

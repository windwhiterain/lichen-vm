#!/usr/bin/env python3
"""Flatten a Qwen3-1.7B safetensors checkpoint into the files `lichen-serve` reads.

The loader names the layout (see `Qwen` in `crates/lichen-serve/src/qwen.rs`): one file a
layer, every tensor a projection reads, concatenated in the order `SHAPES` lists them, and
the checkpoint's own halves copied unconverted. This script is the other half of that
contract, so the two must move together.

    python scripts/dump_bf16.py --checkpoint .model/checkpoint --out .model

The checkpoint directory holds the shards, `model.safetensors.index.json` and
`tokenizer.json`; the output directory receives `layer00_bf16.bin` … `layer27_bf16.bin`,
`embed_tokens_weight_bf16.bin`, `norm_weight_bf16.bin` and the tokenizer.
"""

import argparse
import json
import shutil
import struct
from pathlib import Path

import numpy as np

# The model's shape, in the order `qwen.rs`'s `SHAPES` states. A tensor is (suffix, rows, columns).
LAYERS = 28
HIDDEN = 2048
INTERMEDIATE = 6144
KEY_WIDTH = 1024
GAIN = 2048
NORM = 128

LAYER_TENSORS = [
    ("input_layernorm.weight", GAIN, 1),
    ("self_attn.q_proj.weight", HIDDEN, HIDDEN),
    ("self_attn.k_proj.weight", KEY_WIDTH, HIDDEN),
    ("self_attn.v_proj.weight", KEY_WIDTH, HIDDEN),
    ("self_attn.q_norm.weight", NORM, 1),
    ("self_attn.k_norm.weight", NORM, 1),
    ("self_attn.o_proj.weight", HIDDEN, HIDDEN),
    ("post_attention_layernorm.weight", GAIN, 1),
    ("mlp.gate_proj.weight", INTERMEDIATE, HIDDEN),
    ("mlp.up_proj.weight", INTERMEDIATE, HIDDEN),
    ("mlp.down_proj.weight", HIDDEN, INTERMEDIATE),
]


def shard_table(path):
    """One safetensors shard's tensor table: name -> (start, end, shape)."""
    with open(path, "rb") as handle:
        length = struct.unpack("<Q", handle.read(8))[0]
        header = json.loads(handle.read(length))
    return {
        name: (entry["data_offsets"][0], entry["data_offsets"][1], entry["shape"])
        for name, entry in header.items()
        if name != "__metadata__"
    }


class Checkpoint:
    """The index and the shards, memory-mapped so one layer is read without the rest."""

    def __init__(self, root):
        self.root = Path(root)
        index = json.loads((self.root / "model.safetensors.index.json").read_text())
        self.where = index["weight_map"]
        self.shards = {}

    def tensor(self, name):
        """The tensor's own bytes, as the shard stores them (`BF16` halves)."""
        shard = self.where[name]
        if shard not in self.shards:
            path = self.root / shard
            with open(path, "rb") as handle:
                length = struct.unpack("<Q", handle.read(8))[0]
            table = shard_table(path)
            self.shards[shard] = (np.memmap(path, dtype=np.uint8, mode="r"), 8 + length, table)
        array, start, table = self.shards[shard]
        begin, end, shape = table[name]
        return np.asarray(array[start + begin : start + end]), shape

    def layer(self, index):
        """One layer's eleven tensors, in the loader's order, as raw halves."""
        pieces = []
        for suffix, rows, columns in LAYER_TENSORS:
            name = f"model.layers.{index}.{suffix}"
            raw, shape = self.tensor(name)
            expected = [rows] if columns == 1 else [rows, columns]
            if shape != expected:
                raise SystemExit(f"{name}: {shape} is not {expected}")
            pieces.append(raw)
        return pieces


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--checkpoint", default=".model/checkpoint")
    parser.add_argument("--out", default=".model")
    parser.add_argument("--width", default="bf16", choices=["bf16", "f32"])
    parser.add_argument("--layers", type=int, default=LAYERS)
    parser.add_argument("--skip-embedding", action="store_true")
    options = parser.parse_args()

    checkpoint = Checkpoint(options.checkpoint)
    out = Path(options.out)
    out.mkdir(parents=True, exist_ok=True)

    def write(name, pieces):
        data = b"".join(pieces)
        (out / name).write_bytes(data)
        print(f"{name}: {len(data)} bytes")

    def widen(raw):
        """The f32 value of a bf16 pair, which is the half's bits shifted into place."""
        halves = np.frombuffer(raw, dtype="<u2")
        return (halves.astype(np.uint32) << 16).view("<f4").tobytes()

    suffix = "_bf16" if options.width == "bf16" else ""
    for index in range(options.layers):
        pieces = checkpoint.layer(index)
        if options.width == "f32":
            pieces = [widen(piece) for piece in pieces]
        write(f"layer{index:02}{suffix}.bin", pieces)

    if not options.skip_embedding:
        embed = checkpoint.tensor("model.embed_tokens.weight")[0]
        norm = checkpoint.tensor("model.norm.weight")[0]
        if options.width == "f32":
            embed, norm = widen(embed), widen(norm)
        write(f"embed_tokens_weight{suffix}.bin", [embed])
        write(f"norm_weight{suffix}.bin", [norm])
        tokenizer = Path(options.checkpoint) / "tokenizer.json"
        if tokenizer.exists():
            shutil.copyfile(tokenizer, out / "tokenizer.json")
            print("tokenizer.json")


if __name__ == "__main__":
    main()

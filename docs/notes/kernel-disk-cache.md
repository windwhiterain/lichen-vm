# The assembled-kernel disk cache

> Status: current
> Points at: `crates/lichen-runtime/src/module_cache.rs` (the in-process map, the
> configured directory, the counters), `crates/lichen-runtime/src/module_cache/disk.rs`
> (the file format, the read, the write),
> `crates/lichen-runtime/src/kernel_registry.rs` (`launch_set_digest`),
> `crates/lichen-compute/src/language.rs` (`new_store` names the slot),
> `crates/lichen-compute/examples/kernel_disk_cache_probe.rs` (the three runs).

`cached_module` kept one assembled wasm module per kernel **in this process**,
so a restart re-assembled every kernel from its fragment. This note records the
disk layer that sits under that map, and one finding the brief it came from
turned out to rest on.

## What is on disk

```
<device root>/kernels/<compiler slot>/<launch-set digest>.wasm
```

`new_store(Some(root))` builds the device under `root`
(`PackageStore::with_cache_dir`), so the kernels sit beside the device's own
`artifacts/`, never among them. The last component is
`lichen_utils::cache::compiler_slot_key(DEFAULT_CORE_REPO, &["compute@<version>"])`
— the derivation the package manager and the shipping compiler read too,
because **the assembler is one of those binaries**: module bytes written by
another assembler are bytes this one cannot vouch for.

The plugin list is `compute@<lichen-compute's version>`. `register_compute`
registers the native package at `compute.lichen`, and `compiler_slot_key` takes
pre-spelled parts joined by `&`; the package manager spells one
`<alias>@<version>`, so this list names the vocabulary whose assembler wrote
the bytes. A second plugin composed over the bare language lands in a slot of
its own.

## The key is content, and so is the id now

**When this slice was briefed, `KernelId` was not `fragment_digest`.**
`intern_kernel` handed out a fresh number per kernel from a monotone counter and
used the digest only as the intern index, so the same kernel carried a different
id in a process that compiled another kernel first — while `code-audit.md` `D15`
recorded the intended semantics ("ids become a content digest with an intern
index and a unique-id fallback on a genuine collision") and
`compute.rs`/`write_kernel_id` documented the id as travelling between
processes. **`D15` landed in the same series**: `intern_kernel` now returns the
entry's digest as the id (`id_for_digest`); a digest already held by a different
entry is a genuine collision and probes to the next free slot, so an id can
never alias another kernel.

Named by the id alone, the cache served **the wrong module**, measured with
`kernel_disk_cache_probe reverse` (the same two programs, compiled the other
way round, against a cache one program had already filled):

```
1. the second root    value 22: Int  misses 1  disk hits 0  disk writes 1
2. cross-kernel       value 80: Int  misses 1  disk hits 1  disk writes 1
```

`80` is `11 + 11` — the *other* program's module, read out of this one's file,
validated (its own hash is intact) and run. `D15` names this exact shape as the
failure the content addressing exists to prevent: "the failure mode is a
recompile, never a wrong kernel".

So the file is named by `launch_set_digest(root)` instead: sha256 over the
`fragment_digest` of every fragment the launch set reaches, folded in visit
order. A module is a function of that ordered list and of nothing else — the
emitter reads no claim, no binding fact and no device — so the name changes
exactly when the module does. With it the same run reads `42`, correctly.

**The residual is gone with `D15`.** Before it, a cross-calling fragment embedded
the callee's *slot* in its body, so the root's digest depended on the order its
process compiled in and the reverse run wrote a third file holding the same
module under another name — a miss, never a wrong module. With content ids the
body embeds the callee's *digest*, so the walk is order-independent: the reverse
run now answers `22` and `42` from the same **5 files**, misses `0`, disk hits
`1` and `2`. The cache is still keyed by `launch_set_digest` rather than the root
id alone: the root id folds the entry's **claim**, which the emitter never reads,
so the launch-set digest remains the module's own name.

## The payload, and why it is a miss and not an error

```
sha256(module bytes) | 32 bytes | module bytes
```

On load, both the hash and `wasmi::Module::new` must accept the file. The
directory is a writable shared space, so anything either check refuses — a
truncated write, junk over a payload, a module this engine will not load — is a
miss: assemble again, write the file again. The probe's `heal` run overwrites
85 bytes of one payload with `0xa5` and the next process answers the same `22`
from a freshly assembled module, with the file back to its original 117 bytes.

Writes land in `<name>.wasm.tmp` and are renamed over the final name, so a
concurrent reader sees a whole file or none. The directory is created on the
first write. Every failure is a miss; nothing here panics.

## The counters

Three, all `Relaxed` atomics beside one another:

| counter | counts |
|---|---|
| `module_cache_misses()` | **assembles** this process ran (a disk hit is not one) |
| `module_cache_disk_hits()` | modules served out of the directory |
| `module_cache_disk_writes()` | files that landed (the rename is the last step) |

## The three runs

`kernel_disk_cache_probe <cold|warm|heal|reverse>` against one fixed directory,
each evaluating the same two programs (a cross-calling kernel pair, and a second
root) twice apiece:

| run | misses | disk hits | disk writes | values |
|---|---|---|---|---|
| cold, empty directory | 1, then 2 | 0 | 1, then 2 | `42`, `22` |
| warm, same directory | 0 | 1, then 2 | 0 | `42`, `22` |
| heal, one payload overwritten | 0, then 1 | 1, then 1 | 0, then 1 | `42`, `22` |

The second evaluation of each program changes no counter and adds no file: the
in-process map answers first, so the disk is not consulted again in that
process.

## What is not here

The GPU's SPIR-V / pipeline disk cache is a separate step, and so is any
eviction: the directory only grows by the number of distinct launch sets a
machine has assembled.

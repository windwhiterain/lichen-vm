# Plugin registration: a wrapper compiles on the import that names it

> Status: current — a native plugin package is registered as **pending**, and its
> wrapper is checked and frozen when a program first imports it.
> Points at: `crates/lichen-language/src/package.rs` (`PendingNative`,
> `register_native_deferred`, `materialize_pending_native`, `load_package`),
> `crates/lichen-compute/src/language.rs` (`register_compute`),
> `crates/lichen-compiler/src/cli.rs` (`staged_store`).

## Symptoms

`new_store` registered the compute sub-language's package on every store, and
registration **compiled the wrapper**: a check of `crates/lichen-compute/src/compute.lichen`
(29 lines, 1 587 bytes) that built **4 710 nodes**, then a freeze of the module into the
registry. Measured on this machine, release:

| stage | time | allocations | bytes |
|---|---|---|---|
| `register_compute` (the wrapper) | **5.4–5.6 ms** | 17 095 | 6 682 KiB |
| `prelude_import` (the core prelude) | 1.9–3.6 ms | 7 496 | 2 050 KiB |

For a 91-byte program that imports nothing (`array.lichen`) the store was **4.5 of its
6.9 ms**, and every run paid it whether or not the program could reach the wrapper: a
program that never writes `compute.` still bought the whole compute sub-language.

## Why it is not free to compile

The wrapper is small, but the checker's encoding is not: every definition is a
`[value, type, attributes]` slot, a function's type is the shared `FunctionType`
marker, and the
wrapper's fourteen type constructors (`KT`, `P`, `A`, `S`, `Buf`, `Read`, `Write`, …)
are dependent functions, so one line costs ~160 nodes. There is no artifact to reuse
either: a wrapper's values are a plugin's own runtime-only values, so the artifact
format cannot serialize it (its own module documentation says so).

## Fix: defer, and materialize at the import

`PackageStore` gained `pending_native`: a `(virtual_path, source, native_ops, bindings)`
entry that compiles nothing. `PackageStore::load_package` — the one path every import
resolves through — materializes the pending entry whose **file name** the import names,
before its existing native lookup, and reports a wrapper that fails to compile as a
preprocess diagnostic against the program that named it. `register_native` and
`register_linked` keep their eager behaviour for a host that wants the failure at
registration; `register_native_deferred` / `register_linked_deferred` are the lazy forms.

The compute sub-language and the compiler CLI use the deferred form, so:

- a program that imports no plugin compiles none, and
- one store still compiles each wrapper **once**, because materializing moves the entry
  out of `pending_native` into `native`.

`prelude_import` stays eager: every program carries the prelude as a seeded import, so
deferring it would only move the cost.

## Measurements

Alternating A/B, both trees built in their own worktree, release, `core_profile`:

| reading | before | after | |
|---|---|---|---|
| `array.lichen` (91 bytes, imports nothing) | 6.9 ms | **2.4 ms** | 2.9× |
| its allocations | 24 840 | **7 755** | 3.2× |
| its bytes allocated | 8 796 KiB | **2 119 KiB** | 4.2× |
| its **peak live** | 3 727 KiB | **1 017 KiB** | 3.7× |
| corpus (46 programs, one process) | 752 ms | **624 ms** | 1.2× |
| corpus allocations | 2 476 218 | **2 066 061** | 1.2× |
| corpus bytes allocated | 820 552 KiB | **660 168 KiB** | 1.2× |

The corpus gains only where a program does not import the sub-language: **22 of the 46**
examples import `compute.lichen` and still pay its wrapper, which is why the aggregate is
1.2× while the trivial program is 2.9×. The corpus peak is unchanged (17.7 MB) for the
same reason.

## What it does not change

- **The suite readings are unchanged**: `lichen-language` 265 integration + 68 lib (the
  same two pre-existing lib failures), `lichen-highlevel` 136, `lichen-compute` 220 with
  the same two failures, and the examples' outcome set is byte-identical — the same 8
  programs are red at both ends.
- **The error's timing moves, not its content**: a broken wrapper used to fail at store
  setup; it now fails at the import that names it, as a preprocess diagnostic. That is
  where the program that asked for it can be blamed.
- The lazy path is not visible to a host that calls `register_native` (the eager form):
  the language server's store, which registers its packages once per session, keeps its
  current behaviour.

## What is left

- `prelude_import` — 1.9–3.6 ms and 2 050 KiB on every store, because the prelude is
  compiled per store rather than shared. A store whose registry outlives it would pay it
  once; nothing in this change touches that.
- The evaluator's own hot path is [evaluation-performance](evaluation-performance.md)'s
  subject, and it is the larger half of a real program's run.

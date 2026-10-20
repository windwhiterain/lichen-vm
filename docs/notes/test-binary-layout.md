# One test binary per crate, and how to run one test

> Status: current
> Ruled by: `AGENTS.md`, "Tests". The fold that brought `lichen-serve` here is
> `tests: the serving suite is one binary, one module per area`.

## The rule

A crate carries **at most two** test binaries, one for each kind of test, and never
more:

| binary | what it holds | where it comes from |
| --- | --- | --- |
| the **lib** binary | inline `#[cfg(test)]` unit tests | `src/`, compiled into the library's own test binary |
| the **external** binary | every integration area | `tests/<name>.rs` and the modules under `tests/<name>/` |

An area is a **module** under the one external binary's directory, never a second
top-level file. Cargo links a test binary per `tests/*.rs` **and** per
`tests/<dir>/main.rs`, so a second area as a file is a second binary that statically
links the whole dependency stack again — for the compute and serving crates that is
the language front end, the graph IR, the kernel IR and the Vulkan driver.

The inline half cannot be folded into the external one, and that is why it is allowed
rather than merely tolerated: a unit test reaches the crate's private items
(`low_type`, `equality`, the checker's tables), which are not reachable from a `tests/`
module. One binary per kind is the floor that is actually reachable.

## The shape

The external binary is a root that names its directory with `#[path]`, because a
root's submodules resolve *beside* it rather than under its stem, which is why the
directory's `mod.rs` is named:

```rust
//! This crate's whole integration suite: one binary, one module per area.
//!
//! # Invariant
//! Cargo links a test binary per `tests/*.rs` and per `tests/<dir>/main.rs`, ...

#[path = "lichen_compute/mod.rs"]
mod lichen_compute;
```

`tests/lichen_compute/mod.rs` is then one `mod <area>;` line per file, sorted. A
helper several areas share is declared **once** there and reached with
`use super::common::{…}`, so it is compiled once rather than once per area.

No file under `tests/<name>/` may be named `main.rs`: that spelling is how Cargo
decides the directory is itself a binary.

## Running one test

The binary is one, and **libtest's own free filter argument** is how a single area or
a single test is selected. Nothing is added to the suite for this.

```
cargo test -p lichen-serve --test serve                      # the whole external binary
cargo test -p lichen-serve --test serve step_cost            # one module, by substring
cargo test -p lichen-serve --test serve -- --exact serve::step_cost::the_cost_of_one_step
cargo test -p lichen-serve --test serve -- --list            # every name, module-qualified
```

`--exact` matches the **whole** name the binary lists, so a bare test name filters to
**0 tests** and prints `ok`. An external binary's names carry no crate prefix:
`<module>::<test>`. A lib binary's do: `<crate>::<module>::<test>`.

The fold therefore costs nothing to run narrowly, which is what makes it free of
behaviour: what is selected by an argument was already selected by which file it sat
in.

## What else one binary changes

Linking is the reason for the rule, but it is not the only thing it moves.

**Concurrency.** `cargo test` runs the crate's test binaries **one after another**,
each with libtest's own pool. Folded, there is one pool over every test in the crate:
`lichen-serve` went from 54 processes of 1–12 tests to 77 tests at
`--test-threads = cpus`, and this machine has 20. On a suite that loads a model, that
is over-subscription rather than a speedup, and the suite should be run capped
(`-- --test-threads 4`). **This is the one place the fold needs a decision rather than
a default**, and it is a per-suite judgement: a suite of pure host arithmetic gains
nothing by capping.

**Process-wide state.** A test that relied on its own process — a device installed
once, a stub the crate installs globally, a port — loses that. `lichen-compute-gpu`
already carries a shared device slot for exactly this reason
([its note](lichen-compute-gpu.md)); folding is the moment to find out which suites
never needed one.

## What the fold is worth

Each folded suite reports its own executables relinked, because every binary pays the
full static link of the stack its crate depends on, and its own Vulkan/driver load at
process start.

| crate | before | after | tests before → after |
| --- | --- | --- | --- |
| `fusion_bridge`, `kernel_ir`, `language_server`, `registry`, `utils` (8514f2d1) | 20 binaries | 5 | 10, 22, 46, 22, 8 — unchanged |
| `lichen-serve` | 54 binaries | 1 | 77 → 77, all 54 areas listed by `--list` |

For `lichen-compute-gpu` the measured pair is the one the workspace rule quotes:
**relinked executables 9 → 2**, and the full run's median **9.96 s → 3.23 s**, because
nine process launches each paid the driver's DLL load and became one.

## Reading the census

`cargo metadata --no-deps` is the census, and it is the authority over the working
tree's shape — it is what Cargo will actually build:

```
cargo metadata --no-deps --format-version 1
```

Every package must report **one** target of kind `test` (the external binary) after
its lib. A crate with no integration area reports none, which is also allowed: zero
external and one inline is under the bound, not over it.

**The bound is a check, not a reading.** `test-layout-check` runs the census and
fails on any package above one external binary, naming the crate and the binaries
it carries:

```
test-layout-check --config test-layout.toml
```

The checker is a crate of its own — [test-layout](https://github.com/windwhiterain/test-layout) —
installed the way `comments-check` is and run by CI, so the rule is enforced
rather than stated. It reads the census rather than the working tree, so it
catches both ways Cargo makes a binary — a second `tests/*.rs`, and a
`tests/<dir>/main.rs` — because both appear as a target of kind `test`.

The policy file `test-layout.toml` states the bound and the exemptions. The
exemptions are **path patterns, not package names**: a nested checkout of the
same repository carries this project's crate names, so it is exempt by its place
and not by its name.


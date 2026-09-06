# Testing the package manager offline (venv-test)

> Status: current
> Points at: `scripts/venv-test.sh` (the harness), `crates/lichen-package`
> (the `lichen` package manager + its `plugin.rs`/`main.rs`), and
> `docs/notes/package-manager.md` / `docs/notes/plugin-taxonomy.md` /
> `docs/notes/artifact-cache.md` for the surrounding design.

Every developer should be able to smoke-test the package manager without
touching `~/.lichen` and without reaching GitHub. `scripts/venv-test.sh` does
this: it builds the three shipping binaries, stages them into a scratch `bin/`,
forks a fresh `$LICHEN_HOME`, and runs the package manager end-to-end in an
isolated sandbox under `target/venv-test` (gitignored via `target/`).

Run it from the repo root:

```bash
scripts/venv-test.sh          # run all legs, clean up at the end
scripts/venv-test.sh --keep   # leave the sandbox for inspection
scripts/venv-test.sh --no-build  # reuse existing bin staging
```

## What it verifies

**Leg 1 — shipping `run`/`build` + the device cache.** `lichen run` on `42` and
`1 + 2` prints `42: Int` / `3: Int`; a directory `run` prints each file; `lichen
build` prints `built …` **and** `type: Int`; a compiled artifact
(`artifacts/<sha256(file_id)>.module`) and a `registry` are written under the
fresh `$LICHEN_HOME`; `lichen clean` reclaims the base cache root and reports
`reclaimed 0 cached artifact(s)` (`gc` keeps every `.lichen`/`virtual:` slot).
This pins the shipping compiler's `run`/`build`/persist path (see
[`artifact-cache.md`](artifact-cache.md)).

**Leg 2 — a real `.lichen` git dependency.** A tiny `file://` git fixture repo
holds a `.lichen` library (`_.lichen` = `x => x + 1`); a project `depend`s it
and `import`s it. `lichen fetch` clones it into `sources/math`, then `lichen run`
resolves the import and prints `Function: Int -> Int`. Fully offline — this is
the package manager's dependency-fetch + import-resolution path (see
[`package-manager.md`](package-manager.md)).

**Leg 3 — a native plugin via a local git URL.** A project `plugin`-depends on
`lichen-std-native`; `lichen run --repo file://<local clone>` builds a composed
compiler into `$LICHEN_HOME/compilers/<plugin-set-key>/` and drives it, scoping
its artifact cache to that slot (see
[`plugin-taxonomy.md`](plugin-taxonomy.md)). The composed compiler registers each
plugin's native package (the plugin's `WRAPPER_SOURCE` compiled against its
private native-op registry) on the store it evaluates against, so `std.sort
[3, 1, 2]` fully runs and prints `[1, 2, 3]: Int<3>`. This leg needs **no
network** — see below.

## Accepting a local git URL

The package manager normally resolves a native plugin's **core** crates from
`DEFAULT_REPO` (GitHub), and a native plugin (e.g. `lichen-std-native`) declares
its own core crates as **git** deps to that same canonical repo. Building a
composed compiler therefore needs GitHub — unless the core repo is redirected to
a **local** source.

`lichen run` / `lichen build` / `lichen rebuild-plugin` accept `--repo <u>`, where
`<u>` may be a local `file://` URL (or a bare checkout path). When `--repo`
differs from `DEFAULT_REPO`, the generated compositor's `Cargo.toml` now emits a
`[patch."https://github.com/windwhiterain/lichen-vm"]` section (see
[`plugin.rs`](../../crates/lichen-package/src/plugin.rs) `core_patch`)
redirecting the core crates a plugin git-deps (`lichen-utils`, `lichen-lowlevel`,
`lichen-highlevel`) to that local source. So the whole composed compiler
resolves from the local clone — no GitHub fetch.

This is why leg 3 clones this repo to a `file://` URL and passes it as `--repo`.

## Native package registration at runtime

For a composed compiler to actually *run* a plugin's typed wrapper (not just
carry its vocabulary), the compiler must register each plugin's wrapper as a
**native package** on the store it evaluates against — the same store
`register_native` fills so the plugin's `$sort`-style native calls resolve (see
[`cli.rs`](../../crates/lichen-language/src/cli.rs)
`main_with_native_packages`). The generated compiler's `main` passes one
`("<alias>.lichen", <crate>::WRAPPER_SOURCE, <crate>::<crate>_ops!(LangProgram))`
per plugin; the shipping compiler passes `&[]`, so its store stays native-free.


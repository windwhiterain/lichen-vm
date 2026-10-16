# The `examples` binary panics: `invalid SlotMap key used`

> Status: **open — diagnosis, behaviour unchanged.** The `examples` test binary in
> `lichen-compute` panics at `crates/lichen-lowlevel/src/evaluation.rs:517` with
> `invalid SlotMap key used`, which takes the whole binary down and so masks the
> examples-level verdicts it would otherwise report. This note records what was
> established and how; it changes no behaviour.
>
> Measured, inferred and unknown are kept apart below, and each claim says which
> it is. Nothing here is a fix, and no `Invariant`/`Safety` sentence is added or
> changed.

## The entry point (measured by reading)

`NodeId` is a `slotmap::new_key_type!` key
(`crates/lichen-lowlevel/src/lib.rs:602`) indexing
`Module::nodes: SlotMap<NodeId, Node<P>>`
(`crates/lichen-lowlevel/src/lib.rs:849`). The panic is the `Index` impl
rejecting a key whose slot no longer holds that key's version — slotmap's
`invalid SlotMap key used`.

The site, in `Module::evaluate_node_deep_inner` (`evaluation.rs:501`), is the
first indexing of the walked node:

```rust
// A structural cycle is cut here: the node is being deep-evaluated by an outer frame and
// already holds its cached value.
if self.nodes[node].visiting
    && let Some(value) = self.nodes[node].value
```

## The call chain (measured by reading)

Each link is a real edge in the run path, named file:line:

1. `crates/lichen-compute/tests/examples.rs:42` — the single `#[test]` loops every
   example and calls `readme::program_output(&file, &source)`.
2. `crates/lichen-tools/src/readme.rs:150` — `program_output` builds a **fresh**
   `new_store(None)` per program and calls `evaluate_raw(source, Some(file), &mut store)`.
3. `crates/lichen-language/src/run.rs:223` — `evaluate_raw` delegates to `evaluate_raw_reporting`.
4. `crates/lichen-language/src/run.rs:242-258` — that preprocesses, calls
   `compile_with_imports_at` with `Some(store.registry())`, then `render_build(report.build.unwrap(), …)`.
5. `crates/lichen-language/src/run.rs:149` — `render_build` runs
   `module.evaluate_node_deep(build.root_val, None)` (and `root_ty` at :150).
6. `crates/lichen-lowlevel/src/evaluation.rs:448-455` — `evaluate_node_deep` creates a fresh
   `StaticModuleCache` and calls `evaluate_node_deep_inner`.
7. `crates/lichen-lowlevel/src/evaluation.rs:517` — the panic.

## Stale or foreign (inferred, not yet measured)

Two mechanisms can produce a key `self.nodes` does not hold, and reading has not
yet separated them:

- **Stale** — a node removed earlier in the same module's life. The two removal
  sites are `crates/lichen-lowlevel/src/gc.rs:265` (`drop_block`) and
  `crates/lichen-lowlevel/src/ancestors.rs:49`. `gc.rs:265` is reachable from the
  language's `@gc` marker, and the `gc` example is one of the two that panic.
- **Foreign** — a key belonging to a different `Module`'s map, for instance an
  imported package's node reaching the main module's table.

Note that `program_output` builds a fresh store per program
(`crates/lichen-tools/src/readme.rs:149`), so a key from an *earlier example in the
same process* is not carried by that store. That makes cross-example key sharing
unlikely by construction, but the store is not the whole story and the question is
measured below rather than settled here.

## Measured / inferred / unknown

- **Measured** — the entry point, its function, and the call chain above, all by
  reading; `NodeId` is a slotmap key over `Module::nodes`; `program_output` builds
  a fresh store per program.
- **Inferred** — that the offending key is stale (a removed slot) rather than
  foreign, because `drop_block`/`garbage_collect` remove slots and bump their
  versions, which is exactly the panic's condition.
- **Unknown** — which example reaches it first; whether the examples *family* is
  special; whether it is environmental or a defect on `dev`; the smallest
  reproduction; and what the runner reports once past the panic.

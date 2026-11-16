# Polymorphic flag, slice 1 (expand): implementation record

> Status: **slice 1 landed** on `feature/polymorphic-flag` (commits `a355f335`,
> `435036be`), behaviour-preserving. The flag is computed and asserted; **selection
> still reads the old class-scope rule**, so the flag is inert. Slice 2 (the selection
> swap) is not started. The rulings and hazards are [the contract note](polymorphic-flag.md)'s;
> the baselines and site map are [the recon](polymorphic-flag-recon.md)'s. This note
> records only the implementation choices slice 1 made and what it hands to slice 2.

## What landed

- `Node.polymorphic: Option<bool>` (tri-state) and its frozen mirror
  `StaticNode.polymorphic`; accessor `Module::node_polymorphic`, creation mark
  `Module::mark_polymorphic`, counter helper `Module::write_polymorphic`
  (`crates/lichen-lowlevel/src/{lib,module}.rs`).
- The creation mark on **every formal's** parameter/body cells in `check_lam`
  (`crates/lichen-highlevel/src/checker/lambda.rs`). On this surface every binder is
  primed (recon §2.4), so the mark is constant `true`; the mechanism stays general.
- Propagation `crates/lichen-lowlevel/src/polymorphic.rs`: `compute_polymorphic`
  (read-only fixpoint over the recursion set) and `propagate_polymorphic` (write-back).
- Entry `debug_assert!(flag.is_some())` beside a `# Invariant` stating the ordering at
  `node_apply`, `template_clone`, `static_node_apply`; each clone inherits its source's
  flag (`function.rs`, `template.rs`, `static_module/apply.rs`).
- The freeze computes the flag read-only over the frozen set
  (`static_module/freeze.rs`); artifact format bumped 32 → 33
  (`lichen-language/src/persist{.rs,/container.rs}`).

## Design choices, with the measurements that forced them

1. **Propagation rides the clone-walk entry, not every `evaluate_node_deep`.**
   `OperatorExt::run_deferred` calls `evaluate_node_deep` per operand
   (`lib.rs`), so a per-entry trigger is O(N²): the 100k-deep
   `compaction::deep_block_chain_evaluates_stack_safely` and the infinite-stream
   budget test both hung (>90 s). The clone entry runs once per instantiation.
2. **Global fixpoint + an O(1) skip.** `propagate_polymorphic` flags the whole module
   (every node, reachable from a walk's roots or not) and skips when
   `polymorphic_unflagged == 0`. A clone inherits a `Some` flag at birth and GC cancels
   a released node's debt, so an expansion adds no owed verdict and later applies skip
   in O(1). Measured: the budget test 10.7 s → 0.13 s; the basic suite 0.22 s.
3. **The recursion set walked** (SPEC §Recursion): operation operands of *every*
   operator, value references (array items — shallow included — table entries, a
   closure's whole template), class members routed through the representative, and
   **the owning function's template** of each reached node. The owning-function edge
   closed the capture gap: `highlevel::checker::an_open_struct_template_lets_its_field_be_read`
   reads a node owned by another function through a computed value (recon §4.2 rows 2–3).
4. **Freeze computes, not reads.** The freeze API takes `&Module` (callers, including
   tests, pass immutable refs), so `freeze_set` runs `compute_polymorphic` over the
   frozen set rather than reading a `&mut`-propagated source.

## Coverage gaps closed (recon §4.2)

Class members, non-`External` operands, closure bodies, shallow positions, scope-root
reads, struct identity items, and `External`'s operand's own operands are all on the
propagation walk; argument-side cells (joined by the parameter check's unify) are
covered by the global fixpoint. The entry assert holds across lowlevel basic (167),
highlevel (136) and the language pipeline.

## What slice 2 must take up

- **Upgrade staleness.** The O(1) skip guarantees *totality* (`Some` everywhere), not
  monotone *value* correctness: a mark appearing after the graph is fully `Some` does
  not re-propagate `false → true`. Slice 1's flag is inert so this is invisible; the
  selection swap needs upgrade propagation at class union (the contract note §3
  merge-staleness item, [apply-clone-ownership](apply-clone-ownership.md) §4's
  write-side maintenance).
- **The owning-function edge is conservative** (it pulls a whole template). Slice 2
  may refine it to the exact capture set once selection reads the flag.

## Verification (the tool's own lines)

- `cargo test -p lichen-lowlevel`: `test result: ok. 167 passed; 0 failed; 1 ignored`.
- `cargo test -p lichen-highlevel --test highlevel`: `test result: ok. 136 passed; 0 failed`.
- `cargo test -p lichen-language --test language`: `test result: FAILED. 277 passed; 12 failed`
  — the **same twelve** pre-existing names as the recon baseline (§1.3); no new failure.
- `recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget ... ok`.
- Five Ideal API forms (front end): `6: Int`, `2: Int`, `2: Int`, `0.5: Float`, `Int: Type`.
- `comments-check --config comments-test.toml . --max-violations 0`: no violations.
  `cargo fmt --all -- --check`: clean.

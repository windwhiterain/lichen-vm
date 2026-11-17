# Polymorphic flag: struct-template freshness (slice 2 continuation)

> Status: **both halves landed** on `feature/polymorphic-flag` — the struct-instance half
> (`08c94904`) and the function-generic half (`fad34c2b`, the apply clone freshens its output
> cell per call). The flag was never changed; the fix is in the clone/unify mechanism.
> The rulings are [the contract note](polymorphic-flag.md)'s; the selection swap is
> [slice 2](polymorphic-flag-slice2.md)'s. This note records the struct-field creation
> mark, the measurement that forced its shape, and the `ideal4` diagnosis.
>
> Points at: `crates/lichen-highlevel/src/checker/structs.rs` (`check_struct_value`,
> the open-field mark), `crates/lichen-lowlevel/src/polymorphic.rs`
> (`compute_polymorphic`, the referrer-direction propagation),
> `crates/lichen-highlevel/src/checker.rs` (the `Placeholder` branch's `fresh_cell`),
> `crates/lichen-language/src/resolve.rs` (`seed_imports`, the core-prelude divergence).

## 1. The rule, and where it lives

A struct template with an **open field** — a field whose expression is a `Formal`, a
`Placeholder` (`_`) or an `Annotation` — is per-call: each instantiation fills a fresh
record. `check_struct_value` marks each open field's **value and type slot cells**
polymorphic at creation; the rule is stated in the comment at that mark
(language-spec.md §Polymorphic). A decided constant struct (every field a literal) has
no open field and stays shared.

## 2. Why the mark is on the slot cells, not the record container

A first attempt marked the struct **record container** in `check_struct_value` and did
not fix `ideal4`/`any3`/`any6`. The cause is the propagation direction:
`compute_polymorphic` flows a mark **up to its referrers, never down to children**
(the reverse-edge worklist in `polymorphic.rs`). Marking the container therefore left
the field slot cells `Some(false)`; `node_apply` referenced them in place, and the first
instantiation pinned them (`expected 1, found 1.5`), the second collided. Marking the
leaf slot cells upgrades the field expr and the container through the referrer edges,
so the whole record clones per call.

The `_` hole is a `fresh_cell()` in the checker's `Placeholder` branch — **not**
`any()`/`mark_wildcard_fill` — and is marked only when the binding is primed. So an
unprimed struct template's field holes are shared unless this mark fires; the
`is_wildcard_fill` freshening in `node_apply` does not reach them.

Marking the **value half alone** fixed `any3` but moved `any6` to a type conflict
(`expected Int, found Float`): the field's type slot is a sibling cell, equally shared.
Both halves must be marked.

## 3. Readings at `08c94904` (the tool's own lines)

- `any3` → `(2, 2): Tuple`; `any6` → `((1, 2), (1.5, 2.5)): Tuple` (were cross-call collisions).
- All 22 [nested-apply-none](nested-apply-none.md) §27 reconstruction probes answer their
  recorded values; `ideal1`/`2`/`3`/`5`, `any7` hold.
- `cargo test -p lichen-lowlevel`: `167 passed; 0 failed; 1 ignored`.
- `cargo test -p lichen-highlevel --test highlevel`: `136 passed; 0 failed`.
- `cargo test -p lichen-language --lib`: `69 passed; 0 failed`.
- `cargo test -p lichen-language --test language`: `284 passed; 5 failed` — the same five
  pre-existing names as [slice 2](polymorphic-flag-slice2.md), zero new.
- `comments-check --config comments-test.toml . --max-violations 0`: no violations;
  `cargo fmt --all -- --check`: clean.

## 4. `ideal4`/`any4`: FIXED — the apply clone freshens its output cell per call (`fad34c2b`)

The bug: the per-call codomain bind (`static_function_apply` apply.rs:102 / dynamic
`function_apply` function.rs:265,338, `unify_ref(cell, codomain)`) wrote each call's result
type into the SHARED template result cell (operand[2], node 266), because the clone walk
referenced that non-polymorphic cell in place. The per-call apply clones (detached, distinct
class reps 44 vs 2) collided Int/Float on the one shared cell. The flag was CORRECT
throughout — 266 is rightly non-polymorphic; the defect was the clone/unify wiring, not the
flag. (The "result edge" and "born-consistent value入class" directions were retracted.)

The fix (within SPEC §Clone "clone more nodes should make no change to the language
behaviour"): mark each apply's output cell (operand slot 2) at creation — the checker's
`check_apply` / `check_instantiate` / indexing apply sites call `Module::mark_apply_output`
(`apply_output_cells`). `node_apply`'s `per_call` decision freshens an apply-output cell in
scope, order-independent of the clone walk (a remap pre-insertion at the apply failed because
the operand array can clone before the apply). The clone inherits the cell's flag unchanged —
no flag is touched; the output is detached per call, giving §27's codomain bind the per-call
result cell it assumes. A monomorphic apply's output is not reached by a cloning operand
walk, so it stays shared (breadth preserved; the array-body pin and in-place tests hold).

Acceptance (the tool's own lines): ideal4/any4 `(2, 0.5): Tuple` (both calls); any3 `(2, 2)`,
any6 `((1, 2), (1.5, 2.5))`; all 22 [nested-apply-none](nested-apply-none.md) §27 programs
answer their recorded values; ideal1/2/3/5, any7 hold. lowlevel `167 passed; 0 failed;
1 ignored` (budget test 0.05s, array-body pin ok); highlevel `136 passed; 0 failed`; language
lib `69 passed; 0 failed`; language external `284 passed; 5 failed` — the same five
pre-existing names, zero new. Examples unchanged. comments-check + fmt green.

Runner divergence resolved: native `/` (no core) has no codomain bind; with core `/` is the
frozen div and the bind now writes into the per-call output cell.
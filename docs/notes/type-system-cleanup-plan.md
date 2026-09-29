# Type-system cleanup and standardization plan

> Status: **Phase 0 complete** (B1–B8 all landed on `fix/type-system-phase0`);
> **D3 landed** (`fix/phase1-instantiate`: the checker is total and
> type-directed for `Instantiate`, with the nominal callee check);
> Phases 1+ pending.
> Decisions recorded: D1 = Option A (extract a `Program` unification hook;
> lowlevel becomes honestly untyped). D2 = document equi-recursive
> unification (no occurs check) as the designed semantics. D3 = syntactic
> recognition at parse time (already the reality); the checker is total +
> type-directed with a nominal callee check. D4 = open — see analysis below.
> D5 = compute JIT out
> of scope for now; the future JIT decoupling is to be built on the
> low-type layer — see [lowlevel-low-types](lowlevel-low-types.md).
> Basis: five-point survey of `lichen-lowlevel`, `lichen-highlevel` (checker,
> IR, program), `lichen-utils` / `lichen-compute` / `lichen-perspective`, and
> the docs/tests, performed 2025 — findings cited inline as `file:line`.

### D4 analysis (from the design discussion)

The superior's suspicion is confirmed by the survey: lichen's `Type : Type`
is a **PTS-with-Type:Type** (System U family) where the chain
`value → type → kind → K` has three discriminating levels and then a
self-referential fixed point `K = [Type, ↺]`. `Type` is not a supertype —
the system has no subtyping relation at all (`Int` is not `<: Type`); `Type`
is the type of the atomic type *markers* and the terminal of the chain. The
inert upper chain is a necessary consequence of the uniform
`[value, type]` pair encoding, not an implementation mistake. The genuine
defects to fix are representational, not theoretical: (a) the inert chain
still costs special cases in the unifier/checker (universe whitelist,
deep-prove of the universe); (b) the kind level is repurposed as a metadata
bus (`TypeStruct{id, names}` stores the nominal id and the field-name table
inside the kind marker). Phase 1 should absorb (a); Phase 4 should spec
`Type : Type` honestly as "the type chain closes in a cycle at `Type`".

## 1. Diagnosis: why the type system feels fragile

The fragility is not a pile of unrelated mistakes. It has **one root cause**
and **one boundary violation**:

**Root cause — the type representation is an untyped graph with positional
conventions.** A type is a `[shape, [marker, universe]]` node pattern; there
is no typed view of it. Every rule re-derives meaning from raw array offsets
(`container_ty[0][1]` vs `container_ty[1][0][1]` for the same name table,
`checker.rs:1927-1940` / `2018-2037`), from structural guesses ("marker is a
2-element array ⇒ struct", `checker.rs:1027-1029`), and from magic sentinels
(`usize::MAX`, `NodeId::default()`). There is nowhere to attach an invariant,
so each new feature (structs, named fields, raw reads, attributes, tables)
patched the encoding, the unifier, and the checker in an ad-hoc way. The
duplication epidemic (kind markers spelled in 6 places, the arrow encoding in
5, `TypeOperator::run` in 3, structural descent in the lowlevel ×4) is the
same root cause.

**Boundary violation — the lowlevel claims to be an untyped runtime but
hard-codes highlevel type theory.** `equality.rs` knows the `[Type, ↺]`
universe (`equality.rs:191-230`), the `[shape,[marker,universe]]` kind
encoding (`class_holds_type`, `equality.rs:601-624`), and carries three
bespoke deferral rules justified by highlevel concepts (annotation, dependent
type, field read — `equality.rs:295-345`). In the other direction, the
lowlevel `Module` stores source spans (`assert_spans`, `lib.rs:659-664`),
user-facing assert sets, and checker-shaped `ApplyError`s. Meanwhile the
compute JIT reverse-engineers the checker's encoding from raw nodes
(`compute.rs:1410-1630`), so any encoding change breaks it silently.

Secondary amplifiers: errors flow through global append-only vectors detected
by length-delta and *suppressed* by truncation (`check_unify_relaxed`,
`checker.rs:905-934`); panics are used as control flow
(`catch_unwind` for non-termination, `checker.rs:418-440`); and the docs
lag the code (spec still spells `Int<n>`, `type_of` and named instantiation
are implemented but undocumented, `docs/README.md` indexes 14 of 30 notes).

## 2. Confirmed acute defects (Phase 0 — approved and in progress)

| # | Defect | Status |
|---|---|---|
| B1 | **Compiler stack overflow**: the cycle-cut skeleton gate (`checker.rs:1085-1106`) omitted `Field`/`NamedField`/`RawNamedField`/`Record`; `a = a.x` etc. overflowed the stack | **fixed** (`ab03da4`): gate is now `block_roots` membership alone — exhaustive by construction; regression tests added |
| B2 | **Codec tag collision**: `ComputeValue::write_value` wrote tag `0` for both `TypeBuffer` and `TypeWrite` | **fixed** (`216555e`) + round-trip coverage |
| B3 | **Vacuous test**: `pipeline.rs:1626` `Int<_>` (now a `RawIndex`) tested nothing | **fixed** (`997dc88`): respelled `array<Int, _>`, asserts inferred length |
| B4 | **Release-mode deadlock**: self-freeze guarded only by a `debug_assert` | **fixed** (`cfe6f44`): hard `assert!` |
| B5 | **`Ctx::value_node` omitted `type_marker`** → non-canonical universe node | **fixed** (`ab03da4`) |
| B6 | **Freeze layout fragility**: double payload copy; write/read arena-base alignment mismatch | **fixed** (`cfe6f44`): shared `arena_align::<P>()`, single copy, invariant-checked lookup |
| B7 | **`TypeOperator` semantics duplicated ×3 and divergent** (`Eq` USize-only vs generalized) | **fixed** (`bd0d30b`): one program-generic blanket impl carrying the spec's generalized `==`; both copies deleted |
| B8 | **`LowValue::None` conflates "unbound" with "computed nothing"** (five meanings; predicates disagree; lazy named-read over an anonymous struct hit `unreachable!` misreported as `NonTerminating` — probe-confirmed) | **fixed** (`816b886`): new `LowValue::Void` for computed-nothing (additive codec tag 7; `None` keeps tag 3 as the unit value), `is_unbound` = `Parameterized`-only, defined arms for TableGet/Index/assert/printer/key_hash, `Doc::missing_value` → `Parameterized`, diagnostic dedup keyed by (kind, node) |

## 3. The keystone: name the encoding once (Phase 1)

One new module in `lichen-highlevel` (working name `shape.rs`) becomes the
**single authority** for the pair/type encoding:

- Typed accessors and predicates for: pair layout `[value, type, attrs…]`,
  kind shape `[shape, [marker, universe]]`, the universe, each kind marker,
  the struct marker `[id, names]`, both name-table paths, attribute slot
  arithmetic (`2 + tail index`). Every `is_*_any` family, every magic offset,
  and `tag_descent`'s structural guess (`checker.rs:3097-3142`) move here.
- The 8 kind markers are defined once (one macro or const table) and the
  `TypeValue` variants, `ValueType` methods, `Ctx` accessors, `Checker`
  fields, `Build` fields, and codec tags are all *derived* from it — adding a
  marker touches one place.
- Attribute slot assignment becomes manifest-driven: the composition macro
  assigns slot numbers and *statically rejects collisions*; `Perspective`'s
  `2` and `Doc`'s `3` stop being cross-crate magic numbers
  (perspective.rs:174, doc.rs:67). The three uncoordinated slot orders
  (`merge_slots` sort, frontend tail order, positional `annotation_attrs`)
  are reduced to one.
- Persisted codec tags are derived from declaration order with a round-trip
  property test, replacing the hand-maintained two-sided tables
  (program.rs:515-580, codec.rs:99-238).

This phase changes no semantics; it is the prerequisite that makes every
later phase reviewable.

## 4. Restore the layer boundary (Phase 2)

**Decision D1 (see §7)** chooses the direction:

- **Option A — pull type theory up.** The lowlevel unifier's highlevel
  knowledge (universe recognition, `class_holds_type`, the three deferral
  rules) moves behind a `Program`-level hook (e.g. a `UnifyPolicy` associated
  type). The lowlevel becomes honestly untyped; the deferral rules become
  documented highlevel policy. Cost: touches the hottest, subtlest code in
  the repo; the hook must not slow the unify loop.
- **Option B — push the contract down.** Formally declare the `[value, type]`
  pair grammar and the universe as part of the *lowlevel's* documented
  contract (it is de-facto already), rename the crate's self-description,
  and keep the rules where they are but consolidated and tested. Cost: the
  "untyped runtime" story is abandoned.

Either way, independent of D1:

- Remove `assert_spans` (source positions), `user_asserts`, and the
  checker-shaped `ApplyError`/`UnifyError` attribution fields from the
  lowlevel `Module`; diagnostics attribution becomes a highlevel-side table
  keyed by node id (the lowlevel already supports source-blind `Loc`).
- Replace length-delta error detection (`unify_errors.len()` before/after)
  and truncation-based suppression with a `Result`-returning unify API; keep
  the error vec only as a rendering buffer.
- Resolve the `LowValue::None` ambiguity (B8): `is_unbound` should match only
  `Parameterized`; a nullary-op result and an error yield need distinct
  representation.
- Consolidate the four parallel structural-descent implementations
  (`unify_inner`, `reconcile_*`, `key_eq`, `hash_inner`) around one walker
  with policy flags, or formally document why they must differ.

## 5. Checker structural cleanup (Phase 3)

- Split `checker.rs` (3254 lines) into submodules along its natural seams:
  shape predicates (→ Phase 1 module), lambda/apply, structs/fields,
  arrays/tables, annotations/attributes, diagnostics glue.
- Kill the fabricated-`UnifyError` diagnostic channel
  (`record_*_error` ×3, `checker.rs:2128-2669`): a first-class `Diag` channel
  where "a unify error exists" again means "a unification failed".
- `check_lam` registry surgery (`checker.rs:1362-1406`): build the function
  shell *before* the parameter nodes (a small lowlevel API addition) so no
  `retain`/overwrite dance and no temporarily-invalid `Function` record.
- Replace lexical-depth parent arithmetic (`checker.rs:1374-1378`) with an
  explicit parent link supplied by the frontend in the IR.
- Stop mutating the input IR (`set_schema` at `checker.rs:2257`): merged
  schemas go into a checker-owned side table; restore the documented
  "checker only reads the IR" contract.
- Deduplicate: struct-pair construction ×2, arrow encoding ×5,
  `TypeOperator::run` ×3 (fix B7 by making the generic impl cover
  `LangProgram`), `BinOp` vs `TypeOperator` (one enum + one mapping).
- Panic discipline: the ~49 `unwrap`/`expect` in checker.rs become either
  diagnostics (user-reachable) or `debug_assert` + graceful fallback
  (frontend-bug-only); non-termination stops using `catch_unwind` as control
  flow if the budget guards can report instead.
- Intern `USize(0)`/`USize(1)` and field-name nodes instead of re-allocating
  them ~20 times.
- Remove dead API (`type_expr_node`, `int_type_node`, `Schema::arity`,
  `LocKind`, `Loc::type_depth`, `NativePlugin` marker) and fix
  `recursive_func_nodes` (collects every lambda; rename or fix).

## 6. Semantic standardization decisions (Phase 4)

These change or bless semantics; each needs an explicit decision (§7):

- **D2 — occurs check.** None exists; cyclic types unify (equi-recursive
  behaviour) and the universe *requires* a cycle. Options: (a) document
  equi-recursive unification as the designed semantics and spec it; (b) add
  an occurs check with a universe whitelist (risks breaking
  `struct_recursion.lichen`-style programs).
- **D3 — struct instantiation recognition.** ~~Today a struct type
  arriving via a parameter falls through to plain application and *panics*~~
  **Stale premise**: the frontend parses every glued `X(args)` (except the
  single comma-free positional read) as `Instantiate` unconditionally —
  recognition is already syntactic, and the parameter case already works
  positionally via deferral.  The live defects were checker-side: a panic on
  a call-result callee, no nominality check (a tuple/function type could
  "instantiate"), false named-arg diagnostics for non-concrete callees, and
  a frontend alias-placeholder bug.  **Landed** (`fix/phase1-instantiate`):
  the checker is total and type-directed for `Instantiate` — a concretely
  non-struct callee is an `InstantiateCallee` diagnostic at the callee, an
  unbound callee is pinned to a struct kind (re-checked per apply), a
  call-result callee is force-evaluated at check time, and named arguments
  through a non-statically-known callee are an honest
  `InstantiateNamesNotStatic` diagnostic.  Known limitation (left for Phase
  2's unification-hook extraction, D1): the deferred field-list check of a
  param-dependent call-result callee relies on the lowlevel's
  pending-`Index` deferral (`class_holds_type`), which only accepts a
  2-element concrete other side — a ≠2-field struct shape there reports at
  check time instead of at the apply.
- **D4 — `Type : Type` wording.** Compound types are *not* typed by `Type`
  (they carry `[marker, Type]` kinds), contradicting the README/spec
  wording. Decide the honest statement and spec it.
- **D5 — scope of cleanup.** Whether `lichen-compute`'s JIT (raw graph
  reverse-engineering, process-global kernel/buffer registries, `Box::leak`)
  is in scope now or deferred behind a documented "checker encoding is
  unstable" note.

## 7. Decisions (resolved)

| # | Question | Decision |
|---|---|---|
| D1 | Lowlevel/highlevel boundary | **A**: extract a `Program` unification hook; lowlevel becomes honestly untyped |
| D2 | Occurs check | **Document** equi-recursive unification (no occurs check) as designed semantics |
| D3 | Struct instantiation recognition | **Syntactic at parse time (already the reality); the checker total + type-directed with a nominal callee check** — landed on `fix/phase1-instantiate` |
| D4 | `Type : Type` statement | **Open** — suspected not to be a real universe rule; `Type` is a terminal structure on the type chain, not a supertype. Analyze in Phase 4 |
| D5 | Compute JIT in scope | **Deferred** — mark the checker encoding as unstable for now |

## 8. Tests and documentation (Phase 5, continuous)

- Add the missing coverage identified by the survey: highlevel-level tests
  for `NamedField`/`RawIndex`/`RawNamedField`/`Find`/`Record`/`Static`/
  `NativeCall`, negative error-path tests for raw reads, the monomorphic
  sharing half of let-polymorphism, and a codec round-trip property test
  (would have caught B2).
- Fix the vacuous `Int<_>` test (B3) and the contradictory test docs
  (tests/checker.rs:167-184, 1066-1070).
- Sync the spec: `array<T, n>` spelling in §3, `type_of`, named
  instantiation arguments, the missing §4 compile-table rows
  (`table`, `Find`, `if`, `!`, shallow markers), the deferral rule, and the
  occurs-check statement.
- Rebuild `docs/README.md`'s index (14 → 30 notes), unify the status legend,
  and mark `frontend-syntax-separation.md` implemented.

## 9. Sequencing

1. **Phase 0** — B1–B8 as small independent commits (B1 is a crash fix; do
   first). No design risk.
2. **Phase 1** — the shape/kind/slot authority module. Pure refactor, guarded
   by the existing 80 checker tests + pipeline suite.
3. **Phase 2** — boundary, after D1 is decided.
4. **Phase 3** — checker structure, incremental per submodule.
5. **Phase 4** — semantics, after D2–D5 are decided.
6. **Phase 5** — continuous; docs synced with each landed phase.

Each phase ends with `cargo check` + the *affected* crate tests only
(per workspace policy), `cargo fmt`, and a docs sync.

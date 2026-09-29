# Type-system cleanup and standardization plan

> Status: **Phase 0 complete**; **Phase 1a/1b/1c complete** (shape authority
> module `lichen-highlevel/src/shape.rs`, kind-marker registry, single-sourced
> attribute slots — the canonical attribute order is the composition's `attrs`
> manifest order — and single-sourced codec tags); **D3 landed** (the checker is
> total and type-directed for `Instantiate`, with the nominal callee check);
> **Phase 2 landed** (D1: the unification deferral policy is a `Program` hook —
> `defer_pending` — and the lowlevel is untyped); **Phase 3a landed** (the
> `checker.rs` module split: the checking rules now live in five sibling
> modules beside the root). The assert metadata also moved: the lowlevel
> carries only the worklist and the errors, and a failure is attributed
> through the **template** condition a clone descends from, so a host table
> keyed by that template resolves the user-facing flag and the span (§4). The
> rest of Phase 3 — `check_lam` surgery, the remaining panic sites — is
> pending.
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
lowlevel `Module` stored source spans (`assert_spans`) and user-facing assert
sets (**both moved — see §4**), and still carries checker-shaped `ApplyError`s.
Meanwhile the
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
- Attribute slot assignment is manifest-driven: the composition's
  `attrs = [ … ]` list **is** the canonical attribute order.  The
  composition derives from it the order as data (`LANG_ATTR_ORDER`), each
  attribute's index (`AttrSet::order_index`, the pair slot being
  `shape::attr_slot(i)`), and a build-time assertion that every index is its
  position; the frontend sorts a spelled annotation's attributes into that
  order and the checker sorts a merged tail into it, so the three
  uncoordinated orders (`merge_slots` sort, frontend tail order, positional
  `annotation_attrs`) are one, and `AttrExt::slot()` (with `Perspective`'s
  `2` and `Doc`'s `3`) is gone.  **Landed** (Phase 1b).  The order is a
  compatibility contract like the codec tags, so it is pinned by a test; the
  residual hand step is the *syntax* (the frontend's `Expr::Annotation` has a
  fixed field per attribute kind), not the layout.  `Perspective`'s `2` /
  `Doc`'s `3` as cross-crate magic numbers (perspective.rs:174, doc.rs:67)
  are deleted with it.  See [attributes.md](attributes.md).
- Persisted codec tags are stored in the kind-marker registry entries (not
  derived from declaration order — the tag is the compatibility contract and
  must survive list reordering) with both codec sides generated from the one
  list, plus a round-trip property test iterating the registry-derived
  variant lists, replacing the hand-maintained two-sided tables
  (program.rs:515-580, codec.rs:99-238).

This phase changes no semantics; it is the prerequisite that makes every
later phase reviewable.

## 4. Restore the layer boundary (Phase 2)

**Decision D1 (see §7)** chooses the direction:

- **Option A — pull type theory up. LANDED** (`refactor/phase2-hook`): a
  `Program::defer_pending` hook whose default refuses, plus a `PendingSides`
  read-only view.  The highlevel states the policy in `shape.rs`, where
  "this class holds a type" is decided by the encoding authority; the
  lowlevel keeps only generic graph facts (a pending computation against an
  all-unbound skeleton, two pending `Index` reads) and gains one honest
  primitive, `Module::is_self_referential` — a cycle of length one, stated
  without claiming to know what the cycle means.  `class_holds_type` is gone
  from the lowlevel.  The wiring lives in the highlevel `ProgramImpl` *and*
  in `lang_compose_vocabulary!`, so every composed program inherits the
  policy; a program that states no policy keeps the lowlevel's honest
  default (conflict).
- **Option B — push the contract down.** Formally declare the `[value, type]`
  pair grammar and the universe as part of the *lowlevel's* documented
  contract (it is de-facto already), rename the crate's self-description,
  and keep the rules where they are but consolidated and tested. Cost: the
  "untyped runtime" story is abandoned.

Either way, independent of D1:

- ~~Remove `assert_spans` (source positions), `user_asserts`, and the
  checker-shaped `ApplyError`/`UnifyError` attribution fields from the
  lowlevel `Module`; diagnostics attribution becomes a highlevel-side table
  keyed by node id.~~ **Moving, via a secondary map** (Phase 2; the objection
  below was to the hook, not the goal — the superior ruled the move in).
  The lowlevel stops owning `user_asserts` and `assert_spans`: an assert
  worklist entry carries the **template** condition beside the live one (a
  clone's provenance — a fact of the clone machinery, not of types) and
  `AssertError` records that template, so rendering looks the highlevel-side
  table up by it.  The highlevel already kept its own `user_asserts` set; it
  becomes authoritative and gains the span map.  Two findings make this a net
  subtraction rather than a move plus a hook:
  - `assert_spans` had **no writer anywhere** — its only writes were the two
    clone-propagation paths reading an always-empty map, i.e. dead code;
  - rendering located an assert through `node_edges` keyed by the **live**
    condition, so a per-call clone's assert could never resolve a span; the
    template key fixes that as a side effect.

  `ApplyError`/`UnifyError` stay put, and so do `assert_errors`: they are the
  error contexts only the unify/apply site can build.  What D1 removes is the
  *encoding* knowledge (`[shape, [marker, universe]]`), and that is gone.
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

- ~~Split `checker.rs` (3254 lines) into submodules along its natural
  seams~~ (**done**, Phase 3a, branch `refactor/phase3-split`): the shape
  predicates went to the Phase 1 `shape` module, and the rules now sit in
  `checker/{lambda,structs,indexing,annotations,diagnostics}.rs` beside the
  `checker.rs` root, which keeps the state, the node construction every check
  shares, the per-kind dispatch, the `Build`/`Ctx` plumbing, and the passes.
  Root 1333 / structs 810 / indexing 404 / lambda 338 / annotations 286 /
  diagnostics 223 lines.  Pure code motion: the only edits are the
  `pub(super)` the boundary requires (the root's dispatch and the sibling
  modules call each other) and four doc links that had to become explicit
  paths.  The fabricated-error channel is the remaining part of this group.
- ~~Kill the fabricated-`UnifyError` diagnostic channel
  (`record_*_error` ×4, `checker/diagnostics.rs:100-204`)~~ (**done**, Phase
  3c, branch `refactor/phase3-diag`): "a unify error exists" again means "a
  unification failed".  Two commits, both verified against a before/after
  census of every diagnostic kind (no caller-visible change):
  - **A+B** — the lowlevel's `Module::try_unify` reports the range of
    `unify_errors` the call produced (empty on success), so no caller measures
    a length delta; `DiaryEntry` carries `errors: Range<usize>` (what the check
    owns — empty for a guard) and `seq` (when it was recorded), replacing
    `error_index`.  `check_unify` fills the range, `check_unify_relaxed`
    truncates exactly it, and `mismatch`'s owner lookup is range containment
    instead of "the last entry with `error_index <= i`".
  - **C+D** — the four fabrications are one `record_guard` that records the
    entry directly with an empty owned range; `Build::diagnostics` emits guard
    entries and owned unify errors interleaved by `seq` (unowned deep
    apply-time failures last, as before).  Because a guard no longer writes
    `unify_errors`, `Build::ok` and the two pass gates consult
    `Checker::check_failed` — a failed unification **or** any guard entry —
    rather than the vec's emptiness.
- `check_lam` registry surgery (`checker/lambda.rs:24-228`): build the function
  shell *before* the parameter nodes (a small lowlevel API addition) so no
  `retain`/overwrite dance and no temporarily-invalid `Function` record.
- Replace lexical-depth parent arithmetic (`checker/lambda.rs:81-86`) with an
  explicit parent link supplied by the frontend in the IR.
- Stop mutating the input IR (`set_schema` in `checker/annotations.rs`): merged
  schemas go into a checker-owned side table; restore the documented
  "checker only reads the IR" contract.
- Deduplicate: struct-pair construction ×2, arrow encoding ×5,
  `TypeOperator::run` ×3 (fix B7 by making the generic impl cover
  `LangProgram`), `BinOp` vs `TypeOperator` (one enum + one mapping).
- Panic discipline: the ~49 `unwrap`/`expect` in checker.rs become either
  diagnostics (user-reachable) or `debug_assert` + graceful fallback
  (frontend-bug-only); non-termination stops using `catch_unwind` as control
  flow if the budget guards can report instead.
  - **First case done: the `Index` target.** A read whose runtime target is not
    an array was `unreachable!`, so `f = s => s.x; f (1)` — and every
    parameter-borne named read whose argument is not a named-field struct —
    panicked the compiler, then reported a bogus "this binding never
    terminates" (the caught guard had inflated the depth counters).  It is now
    an `EvalError::IndexTarget` and a computed nothing, exactly like the
    out-of-bounds read beside it.
  - **A failed approach worth recording**: pinning the container's type to a
    struct kind in `check_named_field` when it is not concrete (the move D3
    uses for an instantiation callee) *breaks* `lichen-compute`.  The generic
    kernel wrapper's `.native`/`.sig` reads must stay lazy until an apply binds
    a concrete kernel struct; the pin unifies earlier, exposes targets that are
    not arrays, and every compute test fails.  A check-time pin is not a
    general answer here — the deferral is the answer, and the lowlevel simply
    must not panic on what it eventually finds.
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
  `InstantiateNamesNotStatic` diagnostic.  ~~A ≠2-field struct through a
  param-dependent call-result callee was reported to false-error at check
  time~~ — **not reproducible**: probed on the integrated tree before and
  after the D1 hook extraction, `f = g => (g (0))(1, 2, 3)` with a 3-field
  struct checks and evaluates (`(1, 2, 3): struct<Int, Int, Int>`), so the
  positional path was never arity-limited.  The real constraint on that path
  is the named one above, and it is a decision, not a gap: a lazy
  definition-order reorder is inexpressible (the lazy vocabulary has no
  scatter/gather, and name tables unify by handle, not by content).  **It
  cannot be fixed by being smarter about static analysis**: lichen binds
  names per apply, and whether the value arriving *is* a struct type is
  knowable only at that apply, never at the definition — "is this callee a
  struct" is a per-call-site fact by construction.  Any scheme that decided it
  earlier would have to add declarative constraints the language does not
  have.  The honest diagnostic is the end state, not a waypoint.
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
  sharing half of let-polymorphism. (The codec round-trip property test
  arrived with Phase 1c.)
- ~~Fix the vacuous `Int<_>` test (B3)~~ (done in Phase 0) and the
  contradictory test docs (tests/checker.rs:167-184, 1066-1070).
- ~~Sync the spec~~ (**done**, `docs/phase1-spec-sync`): `array<T, n>`
  spellings, `type_of`, named instantiation arguments, the missing §4
  compile-table rows, the equi-recursive statement (D2), the honest
  `Type : Type` wording (D4), `!` assert semantics.
- ~~Rebuild `docs/README.md`'s index~~ (**done**: all 32 notes indexed,
  statuses normalized; `frontend-syntax-separation.md` marked current;
  `lichen-compute-parallel.md` found superseded and marked historical).

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

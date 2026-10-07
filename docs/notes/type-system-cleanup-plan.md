# Type-system cleanup and standardization plan

> Status: **Phases 0–5 complete.** Phase 5 closed the last four open items
> (an explicit parent link from the frontend, one arrow constructor adopted by
> every crate including `lichen-compute`, the constant and name interning, and
> the two wrong spec rows) and **measured two more and refused them** with the
> numbers that settled it.  Two further user-reachable crashes were found
> outside the census's method — including one in a crate it never surveyed —
> and are recorded where the census lives.  Phase 4 closed the remaining
> structural items (the five dead declarations, `recursive_func_nodes` under its
> real name `lambda_value_nodes`, one struct-type builder, one `BinOp` →
> `TypeOperator` conversion), restored the checker's read-only-IR contract (a
> merged attribute tail is the checker's own, carried on `Build` for the
> renderer), and removed panics-as-control-flow: a budget guard now records
> which limit was exceeded and the non-termination diagnostic names it, so no
> `catch_unwind` remains and a genuine internal panic propagates with a real
> backtrace instead of being relabelled "this binding never terminates".
> Phase 0 (B1–B8); Phase 1 (the `shape`
> encoding authority, the kind-marker registry, single-sourced codec tags and
> attribute slots); D3 (the `Instantiate` check is total and type-directed,
> with a nominal callee check); Phase 2 (D1: the unification deferral policy
> is a `Program` hook and the lowlevel is untyped, with the assert metadata in
> a highlevel-side secondary map keyed by a clone's template condition); Phase
> 3a (the checker is five rule modules beside its root), 3b (the measured
> panic census — every user-reachable panic is a diagnostic, 90 sites
> classified), 3c (an explicit unify result and an honest diagnostic channel:
> no fabricated `UnifyError`), and the `check_lam` surgery (a function shell
> exists before its nodes). Two defects found while verifying were fixed
> rather than recorded: a table read whose key is a lambda parameter formed a
> cycle and crashed the evaluator, and an undecided key's content hashed to a
> constant that could match an entry it never compared against.
>
> The assert metadata also moved: the lowlevel carries only the worklist and
> the errors, and a failure is attributed through the **template** condition a
> clone descends from, so a host table keyed by that template resolves the
> user-facing flag and the span (§4).
>
> Decisions recorded: D1 = Option A (extract a `Program` unification hook;
> lowlevel becomes honestly untyped). D2 = document equi-recursive
> unification (no occurs check) as the designed semantics. D3 = syntactic
> recognition at parse time (already the reality); the checker is total +
> type-directed with a nominal callee check. D4 = resolved — `Type` is the
> terminal of the type chain, not a supertype: the chain closes in a cycle at
> `Type`, and that cycle is what admits recursive types (analysis below; the
> statement is now in the [spec](../language-spec.md) §3 and the
> [README](../../README.md)). D5 = compute JIT out of scope for now; the
> checker encoding is labelled unstable for external consumers — see
> [checker-encoding-instability](checker-encoding-instability.md) — and the
> JIT decoupling was to be built on the low-type layer, which is now
> [implemented](lowlevel-low-types.md); the label survives, narrowed to the
> body's graph walk.
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
bus (`[payload, TypeStruct]` stores the nominal id and the field-name table
inside the kind marker's payload). Phase 1 should absorb (a); Phase 4 should spec
`Type : Type` honestly as "the type chain closes in a cycle at `Type`".

## 1. Diagnosis: why the type system feels fragile

The fragility is not a pile of unrelated mistakes. It has **one root cause**
and **one boundary violation**:

**Root cause — the type representation is an untyped graph with positional
conventions.** A type is a `[shape, [marker, universe]]` node pattern; there
is no typed view of it. Every rule re-derives meaning from raw array offsets
(`container_ty[0][0][1]` vs `container_ty[1][0][0][1]` for the same name table,
`checker.rs`), from structural guesses (the struct marker's shape, now the
`TypeStruct` tag, `shape.rs`), and from magic sentinels
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
`checker.rs`); panics are used as control flow
(`catch_unwind` for non-termination, `checker.rs` — **removed**: the
budget guards now record `Module::budget_exhausted` instead of panicking, so
non-termination is no longer control-flow-by-unwind); and the docs
lag the code (spec still spells `Int<n>`, `type_of` and named instantiation
are implemented but undocumented, `docs/README.md` indexes 14 of 30 notes).

## 2. Confirmed acute defects (Phase 0 — approved and in progress)

| # | Defect | Status |
|---|---|---|
| B1 | **Compiler stack overflow**: the cycle-cut skeleton gate (`checker.rs`) omitted `Field`/`NamedField`/`RawNamedField`/`Record`; `a = a.x` etc. overflowed the stack | **fixed** (`ab03da4`): gate is now `block_roots` membership alone — exhaustive by construction; regression tests added |
| B2 | **Codec tag collision**: `ComputeValue::write_value` wrote tag `0` for both `TypeBuffer` and `TypeWrite` | **fixed** (`216555e`) + round-trip coverage |
| B3 | **Vacuous test**: `pipeline.rs:1626` `Int<_>` (now a `RawIndex`) tested nothing | **fixed** (`997dc88`): respelled `array<Int, _>`, asserts inferred length |
| B4 | **Release-mode deadlock**: self-freeze guarded only by a `debug_assert` | **fixed** (`cfe6f44`): hard `assert!` |
| B5 | **`Ctx::value_node` omitted `type_marker`** → non-canonical universe node | **fixed** (`ab03da4`) |
| B6 | **Freeze layout fragility**: double payload copy; write/read arena-base alignment mismatch | **fixed** (`cfe6f44`): shared `arena_align::<P>()`, single copy, invariant-checked lookup |
| B7 | **`TypeOperator` semantics duplicated ×3 and divergent** (`Eq` USize-only vs generalized) | **fixed** (`bd0d30b`): one program-generic blanket impl carrying the spec's generalized `==`; both copies deleted |
| B8 | **`LowValue::None` conflates "unbound" with "computed nothing"** (five meanings; predicates disagree; lazy named-read over an anonymous struct hit `unreachable!` misreported as `NonTerminating` — probe-confirmed) | **fixed** (`816b886`): new `LowValue::Void` (renamed `LowValue::Error` afterwards) for computed-nothing (additive codec tag 7; `None` keeps tag 3 as the unit value), `is_unbound` = `Parameterized`-only, defined arms for TableGet/Index/assert/printer/key_hash, `Doc::missing_value` → `Parameterized`, diagnostic dedup keyed by (kind, node) |

## 3. The keystone: name the encoding once (Phase 1)

One new module in `lichen-highlevel` (working name `shape.rs`) becomes the
**single authority** for the pair/type encoding:

- Typed accessors and predicates for: pair layout `[value, type, attrs…]`,
  kind shape `[shape, [marker, universe]]`, the universe, each kind marker,
  the struct marker `[payload, TypeStruct]` (payload `[id, names,
  names_in_order]`), both name-table paths, attribute slot
  arithmetic (`2 + tail index`). Every `is_*_any` family, every magic offset,
  and `tag_descent`'s structural guess live in `shape.rs`.
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
- ~~Replace length-delta error detection (`unify_errors.len()` before/after)
  and truncation-based suppression with a `Result`-returning unify API; keep
  the error vec only as a rendering buffer.~~ **Done** (Phase 3c, branch
  `refactor/phase3-diag`), but by a different mechanism: the measurement moved
  *inside* the lowlevel.  `Module::try_unify(a, b) -> (NodeId, Range<usize>)`
  returns the range of `unify_errors` the call itself produced, empty on
  success (`equality.rs:118-122`), so no caller measures a length delta and no
  `Result`-returning API was needed; `check_unify` fills the owned range and
  `check_unify_relaxed` truncates exactly it.  The vec remains the rendering
  buffer, as proposed.  (Detail under §5, "Kill the fabricated-`UnifyError`
  diagnostic channel".)
- ~~Resolve the `LowValue::None` ambiguity (B8): `is_unbound` should match only
  `Parameterized`; a nullary-op result and an error yield need distinct
  representation.~~ **Done in Phase 0, as B8** (`816b886`): `LowValue::Void` —
  renamed `LowValue::Error` afterwards, source-level only — is
  the computed-nothing value and `is_unbound` matches `Parameterized` only —
  see the B8 row in §2.
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
- ~~`check_lam` registry surgery~~ **done** (`68fa1e9`): `Module::begin_function` /
  `finish_function` make the shell exist before its nodes, so the parameter
  cells and pair are registered in the right template as they are allocated —
  the `retain` over the enclosing scope, the re-tagging, and the wholesale
  overwrite of `Function::nodes` are gone.  The record between the two calls
  is deliberately incomplete (its slots unset, and `finish_function` asserts
  the parameter is already a scope member), so a half-built function is not
  representable as a lying record.  The natural registration order equals the
  order the old code hand-assembled, so `Function::nodes` and the artifacts
  are byte-identical (all 25 examples verified).
- Replace lexical-depth parent arithmetic (`checker/lambda.rs:81-86`) with an
  explicit parent link supplied by the frontend in the IR.
- Stop mutating the input IR (`set_schema` in `checker/annotations.rs`): merged
  schemas go into a checker-owned side table; restore the documented
  "checker only reads the IR" contract.
- Deduplicate: struct-pair construction ×2, arrow encoding ×5,
  `TypeOperator::run` ×3 (fix B7 by making the generic impl cover
  `LangProgram`), `BinOp` vs `TypeOperator` (one enum + one mapping).
- Panic discipline (**census measured — see the inventory below**): every
  `unwrap`/`expect`/`unreachable!`/`panic!` in the checker and the lowlevel is
  classified as user-reachable (becomes a diagnostic / recorded failure),
  frontend-bug-only (a `debug_assert!` with the invariant stated), or
  not-provable (left alone); non-termination **no longer** uses
  `catch_unwind` as control flow — the three budget guards record
  `Module::budget_exhausted` (which budget, and its limit) and return a
  value, and the checker's three former catch sites are field reads.  A
  genuine internal panic now propagates with its real backtrace instead of
  being relabelled "this binding never terminates".

  ### Panic-discipline census (measured, branch `refactor/phase3-panic`)

  The plan's earlier "~49 in checker.rs" was a rough count of one file. The
  measured figures are **59 sites in `lichen-highlevel`** (checker.rs 18,
  checker/structs.rs 13, checker/indexing.rs 11, program.rs 5, checker/lambda.rs
  6, checker/annotations.rs 5, ir.rs 1) and **31 in `lichen-lowlevel`**. Each
  was probed with source programs through
  `cargo run -p lichen-language --bin lichen-compiler -- <file>`; every site not
  listed under "left" below is reachable only through a checker-invariant path.
  The per-site line numbers for `checker.rs` and `lowlevel/static_module.rs` are
  dropped from the tables below: those two files were later split into rule
  modules and their line numbers no longer resolve, so a site names its file
  and its classification only (`P5-14`).

  #### Changed (user-reachable — now a diagnostic or a recorded failure)

  | Site | Class | Evidence | New behaviour |
  |---|---|---|---|
  | `highlevel/src/checker.rs` | user-reachable, check time | a package whose last statement is a raw read (`[1, 2]<0>`), imported by another file | a recorded `ImportExport` guard at the import's location; the expression still compiles to a pair of fresh cells, so the descent stays total |
  | `lowlevel/src/evaluation.rs:162` | user-reachable, runtime | `a = [1,2,3]` / `i = "x"` / `a[i]`; also `a(k)`, raw `<…>`, and the same through a parameter (`f = x => a[x]`; `f "x"`) | `EvalError::IndexSubscript` + an empty value (`Error`), rendered as "this value is not an index" at the subscript's own span |
  | `lowlevel/src/evaluation.rs:154` (target not an array) | user-reachable, runtime | `f = s => s.x; f (1)` | `EvalError::IndexTarget` + `Error` — done at `62aad04`, kept as the model for the rows above |

  Every row above previously panicked the process and then reported a *bogus*
  "this binding never terminates" (the caught guard had inflated the depth
  counters). All three now report one honest diagnostic with a real `Loc`.

  #### Left (with the reasoning)

  | Site | Class | Reasoning |
  |---|---|---|
  | `checker.rs` | unreachable | the no-attribute-extension `attr_ext` is only installed for a program whose every schema carries no attribute, so it is never consulted; the language composition always supplies an extension |
  | `checker.rs` | frontend-only | every `check_term` arm sets `ty[root]`; the root is a compiled statement or a literal pair by construction |
  | `checker.rs` | frontend-only | `range_children`/`range_depths` are called only from the arm that matched the same `ExprKind`; `check_record`'s and `named_instantiate`'s `value` is always an `alloc_tuple` (frontend contract) |
  | `checker.rs` | frontend-only | `check_expr` immediately precedes both, and every arm sets `term` |
  | `checker.rs` | frontend-only | B1's `block_roots`-only skeleton gate is exhaustive by construction; all 26 self-referential root shapes (`a = a.x`, `a(0)`, `a[0]`, `a::x`, `a{0}`, `a<0>`, `a # 2`, `a ? "d"`, …) compile clean |
  | `checker.rs` | frontend-only | the `NativeCall` registry is validated against the frontend before the build; a name cannot reach here unregistered |
  | `checker.rs` | frontend-only | each follows a `check_expr` of its operand, which sets `ty`/`term` |
  | `checker.rs` | frontend-only | every `Parameter` use is compiled inside `check_lam` after the scope push (line 131), including `parameter_type`/`parameter_attribute` |
  | `checker/annotations.rs:153`, `155`, `162`, `216` | frontend-only | `check_expr` precedes each; every arm sets `term`/`ty` |
  | `checker/annotations.rs:205` | frontend-only | the frontend emits `attrs.len() == tail.len()` sorted by `order_index`, and `merge_slots` is a union, so the spelled-marker count equals the attribute-expression count; ~20 multi-attribute spellings probed clean |
  | `checker/indexing.rs:48`, `160`, `198`, `246`, `252`, `292`, `296`, `395` | frontend-only | each follows `check_expr` of the same expression |
  | `checker/indexing.rs:341`, `343`, `373` | frontend-only | `wrap_shallow` runs only after `check_expr(el)`, and the `levels`-empty path is the only `unwrap_or` arm |
  | `checker/lambda.rs:150`, `180`, `215`, `248`, `266`, `299` | frontend-only | each follows `check_expr` of the named sub-expression; `299` follows an `is_some()` test on the same slot |
  | `checker/structs.rs:46`, `120`, `213`, `368`, `407`, `423`, `527`, `590`, `613`, `712` | frontend-only | each follows `check_expr` |
  | `checker/structs.rs:717-718` | frontend-only | a `None` hole needs a named argument leaving lower positions unclaimed, but `def_len` is `Some` whenever the name table is readable, and the missing-field pass sets `valid = false` before the reorder runs; 13 named-instantiation shapes probed clean |
  | `checker/structs.rs:773` | frontend-only | dispatched from the matching `TypeStruct` arm |
  | `ir.rs:615` | frontend-only | called only on a matched `Annotation` kind |
  | `program.rs:609`, `648`, `659`, `685`, `693` | frontend-only | total matches inside one operator implementation; each arm is excluded above |
  | `lowlevel/evaluation.rs:82`, `85` | **user-reachable — NOT FIXED** | see below |
  | `lowlevel/evaluation.rs:90`, `188`, `212`, `255`, `260`, `330` | frontend-only | the operand bundle is built by the checker's own `array_node` helpers; every operator is handed the arity its arm destructures |
  | `lowlevel/evaluation.rs:195` | frontend-only | `evaluate_node_deep` sets `evaluated_deep` before returning |
  | `lowlevel/evaluation.rs:327` | frontend-only, **re-measured** | a `TableGet` target is a table or a computed nothing; both are armed.  The third case — an op built against a non-table — *was* reachable: the checker's forcing of a raw read's type walked the name table of a container the read's kind guard had just refused (an array's type, where the walk's last step is the array's own universe; `unreachable!("TableGet target must be a table")`).  The raw reads now leave the refused read **unbuilt** (`Checker::refused_pair`), so the checker never builds a `TableGet` against a non-table and the arm stays an invariant |
  | `lowlevel/evaluation.rs:399` | frontend-only | the `Static` arm above already returned |
  | `lowlevel/evaluation.rs:415` | **done** | the non-termination budget guard — no longer a panic: it records `Module::budget_exhausted` (the budget and its limit), returns the computed-nothing value, and the checker reads the field |
  | `lowlevel/evaluation.rs:520`, `gc.rs:170` | frontend-only | `garbage_collect` keeps the node it was given |
  | `lowlevel/equality.rs:192` | frontend-only | a `Module` always has a root block; both-sides-static cannot arise from the apply path |
  | `lowlevel/apply.rs:28`, `34` | **done** | the two apply budget guards — same conversion: they record `Module::budget_exhausted`, return the undecided marker, and never unwind |
  | `lowlevel/codec.rs:120`, `131`, `139` | frontend-only | a frozen module's payloads were relocated by the freeze layout pass |
  | `lowlevel/lib.rs:315`, `319`, `366`, `903` | frontend-only | total matches inside one impl; the operator arms are dispatched by the VM |
  | `lowlevel/static_module.rs`, `lowlevel/static_module/freeze.rs` | frontend-only | registration and the phase-2 layout precede every read |
  | `lowlevel/table.rs:128`, `163` | frontend-only | the deep pass resolves a key before it is hashed; the value variants are total |
  | `lowlevel/utils.rs:55` | frontend-only | a non-empty length and a non-zero alignment always form a valid `Layout` |

  #### The one user-reachable panic the census found — **since fixed**

  `lowlevel/src/evaluation.rs:82` — `unreachable!("cycle detected: …")`,
  minimal trigger:

  ```text
  t = table { 1 ==> 2 }
  f = x => t{x}
  f 1
  ```

  The census left it in place (a sharing/ordering defect, not a missing
  diagnostic); it was then fixed at root.  The mechanism was **not** the
  aliasing the census hypothesised: `TableGet` is a structural operator and
  evaluates its operands shallowly.  The forcing came from `key_hash`, which
  force-evaluated the key *before* asking whether it was decided, so the
  forced descent reached the very apply being evaluated.

  The real defect was a conflation, not a cycle: a read answered "cannot
  match" for both a key that is **not decided yet** and a key that is decided
  and **simply absent**.  Only the second is a miss.  `key_hash` now reports
  which of the three states it is in (`KeyState`), an undecided key leaves the
  read lazy, an `Error` key still misses, and a build still drops an undecidable
  entry.  The program evaluates to `2`.

  The same reasoning fixed a latent hazard found while narrowing it:
  `hash_inner` panicked on a key subtree holding a value-less node (asserting
  the deep pass must already have resolved it), and hashed a `Parameterized`
  element to a single `PARAM_TOKEN` — two different undecided keys collapsing
  onto one hash, so a lookup could hit an entry it never compared against.
  Undecided content at **any** depth is now undecided, and `PARAM_TOKEN` is
  gone.

  #### The same `unreachable!` survived the fix — a second-order re-entry

  The Phase 4 survey re-measured the census and found this site still
  user-reachable through a shape the census never tried.  Bisected to `8d0302d`
  — the fix that introduced `KeyState` introduced this:

  ```text
  t = table{ [1, 2] ==> 10 }
  g = x => x
  t{g [1, 2]}
  ```

  `build` and `run` both died with an uncaught `unreachable!` and exit 101 —
  **no `catch_unwind` covers those paths**; with a binding in between the panic
  *was* caught and reported as a false "this binding never terminates", the
  exact B8 failure mode in a new position.  Two defects stacked:

  1. the two early returns out of the read (`KeyState::Undecided` /
     `Unhashable`) bypassed the postlude, so the node's `visiting` mark was
     never cleared and the *next* pass hit the guard;
  2. fixing only that is **not sufficient** — because the undecided answer is
     not cached, it propagates into the enclosing array, whose descent forces
     the same `TableGet` that is still mid-computation.  The re-entrancy is
     real and independent of the stale mark.

  The fix is the invariant that was missing: **`Node::visiting` is owned by one
  evaluation attempt.**  `evaluate_node` delegates, the delegated body brackets
  the work, and the operation dispatch sits inside the bracket, so no early
  return and no panic can leak the mark.  The bracket is a `Drop` guard, which
  also made the budget work below safe.  Both programs now compile and
  evaluate to `parameterized: Int`.

  #### The census is not closed — two more, and one crate never surveyed

  Both of these were found *after* the census was written, neither by its own
  method, and the second one not in either crate it covered.

  1. **An unregistered native operator** — `Checker::check_native_call`
     resolved the `$name` against the module's private registry with an
     `expect`, on the stated invariant that "a native op name must be validated
     by the frontend against the module's registry".  **That invariant was
     never established**: `native_ops`' own field doc asserted it, the frontend
     has no access to the registry (it says so itself), and there is no
     validation site anywhere in `lichen-language`.  So `x = $nosuchop(1)` in
     an ordinary file exited 101.  The checker owns the registry, so the
     checker owns the check: it is now a `NativeOpUnresolved` guard at the `$`,
     and the three comments that claimed otherwise — the field, the empty
     default, and the panic message — state the truth.
  2. **`crates/lichen-language` was never surveyed at all** — 53 production
     panic sites, against the 90 the census classified across two other crates.
     Enumerated and probed: **none is user-reachable.**  The `run`/`build`
     `report.build` unwraps are guarded by the fact that `build` is `None` only
     when resolve produced no IR, and resolve always emits a diagnostic; the
     `path.file_name()` unwraps in the CLI are downstream of a successful
     package load, so a bare `.` or `..` argument is refused with a diagnostic
     first; `readme.rs`'s seven are in a module the shipped CLI never reaches
     (it has exactly `Run` and `Build`); `package.rs`'s registry panics are
     process-lifetime invariants whose message even says "restart the process".
     Ten adversarial programs (empty source, bare `.` as an argument and as a
     body, 400-entry table, 60-deep lambda chain, self-reference, bad metadata)
     all reported honestly.  The one defect the sweep did find was a comment
     still claiming a non-terminating program "panics at the VM's
     recursion-depth guard" — true until the budget work removed that panic.


  - **A failed approach worth recording**: pinning the container's type to a
    struct kind in `check_named_field` when it is not concrete (the move D3
    uses for an instantiation callee) *breaks* `lichen-compute`.  The generic
    kernel wrapper's `.native`/`.sig` reads must stay lazy until an apply binds
    a concrete kernel struct; the pin unifies earlier, exposes targets that are
    not arrays, and every compute test fails.  A check-time pin is not a
    general answer here — the deferral is the answer, and the lowlevel simply
    must not panic on what it eventually finds.
- **Landed** (`refactor/phase5-encoders`; was "deferred" in Phase 4): intern
  `USize(0)`/`USize(1)` and field-name nodes instead of re-allocating them at
  14 + 3 sites.  The Phase 4 objection was the right one to raise — a shared
  constant node is a *shared* node, where before each occurrence allocated its
  own, so this is a real clone-aliasing change to the apply walk, not a
  refactor — and it is now **discharged rather than taken on faith**: the two
  constants are allocated by `install_constants` *before any function exists*,
  so they carry no function tag and belong to no template, which means the
  clone walk's membership test already referenced such nodes in place.  Same
  node, same behaviour, fewer allocations.  The examples' compiled output is
  byte-identical (verified by hash), and the affected crates' suites are green.
  `lichen-compute`'s four own constant allocations were **not** ported — see
  the adoption note above.
- **Landed** (`refactor/phase4c-decls`): the dead API (`type_expr_node`,
  `int_type_node`, `Schema::arity`, `LocKind` with its only user `Loc::kind`,
  and `Loc::type_depth`) is removed — each had zero call sites in the
  workspace.  `recursive_func_nodes` is renamed `lambda_value_nodes`: it
  collects *every* lambda's value node, not only the recursive bindings', and
  the write stays unconditional.  **Deliberately kept**: `NativePlugin` — a
  published trait with out-of-tree implementors (`lichen-compute`,
  `lichen-std-native`), so deleting it is a public API break; it survives as a
  nominal marker, and its doc no longer claims the `impl` enforces a contract
  that nothing is generic-bound to check.

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
  through a non-statically-known callee were an honest
  `InstantiateNamesNotStatic` diagnostic.  ~~A ≠2-field struct through a
  param-dependent call-result callee was reported to false-error at check
  time~~ — **not reproducible**: probed on the integrated tree before and
  after the D1 hook extraction, `f = g => (g (0))(1, 2, 3)` with a 3-field
  struct checks and evaluates (`(1, 2, 3): struct<Int, Int, Int>`), so the
  positional path was never arity-limited.  The named half of that clause is
  **superseded** (`feature/deferred-instantiate`) and the argument below it
  was wrong about the encoding: the reorder needs no scatter/gather — the
  struct marker carries its field names **in definition order** beside the
  name→index table, so a definition position's *name* is a constant subscript
  away, and the argument supplying it is found by a lookup keyed by that name
  (and, when the types are decided, by the field's type, which is how the
  field-type check fires).  A named instantiation through an unresolved callee
  now checks and reorders when the type resolves;
  `DiagKind::InstantiateNamesNotStatic` is gone.  The decision that was taken
  as "the honest diagnostic is the end state" is therefore reversed, and the
  recorded reason — "a lazy definition-order reorder is inexpressible (the
  lazy vocabulary has no scatter/gather, and name tables unify by handle, not
  by content)" — did not hold: the reorder is positional reads into the
  call-order array, and the type comparison is the table read's own
  content equality, not a handle comparison.
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
| D4 | `Type : Type` statement | **Terminal, not supertype** — the chain closes in a cycle at `Type`, and that cycle is what admits recursive types; no subtyping, a compound type is typed by its kind. Landed in the [spec](../language-spec.md) §3 and the [README](../../README.md) |
| D5 | Compute JIT in scope | **Deferred** — mark the checker encoding as unstable for now: [checker-encoding-instability](checker-encoding-instability.md). The decoupling this deferred was has since been built, on the low-type layer ([lowlevel-low-types](lowlevel-low-types.md), Phases 3a–3c): the JIT's *domain* read no longer touches the encoding, and the label is narrowed to the body's graph walk |

## 8. Tests and documentation (Phase 5, continuous)

- **The nine survey test gaps were mostly stale, and the plan said so wrongly.**
  Re-measured in Phase 4: eight of the nine are already covered, but downstream
  in `lichen-language` and in `examples/`, not at the `lichen-highlevel` level
  the plan named — **not one** of `NamedField`/`RawIndex`/`RawNamedField`/
  `Find`/`Record`/`Static`/`NativeCall` is ever constructed in
  `crates/lichen-highlevel/tests/**`, because the behaviour is already pinned
  where the frontend is in play.  The ninth was real and pointed: the three
  diagnostics Phase 3's census created to replace panics
  (`RuntimeIndexTarget`, `RuntimeIndexSubscript`, `ImportExport`) shipped with
  **zero** test references anywhere.  **Done** (`test/phase5-raw-read-diags`):
  one test each, plus the uncovered "find on a concretely non-table container"
  guard.  One finding worth keeping: `RuntimeIndexTarget` carries **no source
  span** in any program, structurally — `check_raw_index` records an edge for
  the subscript only — so the test pins that absence rather than a caret.  The
  monomorphic sharing half of let-polymorphism was already pinned
  (`pipeline.rs` `a_bound_struct_type_is_reusable`, with
  `two_struct_type_occurrences_do_not_unify` as its negative control).
  (The codec round-trip property test arrived with Phase 1c.)

- **The suite's own guard was self-healing, and that made it absent — LANDED.**
  Every phase of this cleanup rests on one invariant: the 23 example programs
  produce byte-identical output.  **Nothing in the repository enforced it.**
  `every_example_runs` asserted only that each program compiled and ran, and
  `readme_embeds_the_current_example_programs` went further than not checking —
  it called `sync_output_comments`, which **rewrote** each file's
  `output = "..."` metadata to whatever the compiler now printed, and passed.
  So a refactor that silently changed any example's output would have been
  absorbed into a green suite and a dirty tree, rather than caught.  (No drift
  did occur: the manual hash comparison held on every phase, so the work was
  sound — but sound by diligence, not by construction.)

  The two things are now treated differently, on purpose: **the README's
  embedded blob is derived documentation and still self-heals**, while **a
  program's declared output is a claim about observable behaviour and is
  asserted**.  `every_example_runs` became
  `every_example_runs_and_prints_what_it_declares`, comparing
  `readme::declared_output` against `readme::program_output` — the *same*
  function the README's generator uses, so there is no second notion of "what
  this program prints" — and reporting every drifted file at once rather than
  stopping at the first.  A file with no `output =` declaration now fails too,
  since the declaration is what makes the example a specification.

  Verified by falsification, not by passing: deliberately corrupting one
  declaration made the suite fail with the declared and actual values side by
  side.  A passing assertion proves nothing on its own; this one was watched
  to go red.
- ~~Fix the vacuous `Int<_>` test (B3)~~ (done in Phase 0) and the
  contradictory test docs (tests/checker.rs:167-184, 1066-1070) — **the test
  docs are done** (Phase 4 documentation sync): the `is_int_type` helpers
  now state the marker-slot / type-slot contract, and the "call result
  annotations are lazy" section header is replaced by what the two tests below
  it pin.  The tests themselves were not changed.
- ~~Sync the spec~~ (**done**, `docs/phase1-spec-sync`): `array<T, n>`
  spellings, `type_of`, named instantiation arguments, the missing §4
  compile-table rows, the equi-recursive statement (D2), the honest
  `Type : Type` wording (D4), `!` assert semantics.  (The §4 table was finished
  in the Phase 4 documentation sync: the `Static` and `NativeCall` rows were
  added and the `RecordBlock` row was corrected to the `Record` variant.)
- ~~Rebuild `docs/README.md`'s index~~ (**done**: all 32 notes indexed,
  statuses normalized; `frontend-syntax-separation.md` marked current;
  `lichen-compute-parallel.md` found superseded and marked historical; the
  index carries 33 after the checker-encoding note arrived with the Phase 4
  documentation sync).

## 9. Sequencing

1. **Phase 0** — B1–B8 as small independent commits (B1 is a crash fix; do
   first). No design risk.
2. **Phase 1** — the shape/kind/slot authority module. Pure refactor, guarded
   by the existing 80 checker tests + pipeline suite.
3. **Phase 2** — boundary, after D1 is decided.
4. **Phase 3** — checker structure, incremental per submodule.
5. **Phase 4** — semantics, after D2–D5 are decided. **Done**: D2–D5 all
   resolved, the remaining structural items closed, the checker no longer
   mutates the IR, and the budget guards record instead of panicking.
6. **Phase 5** — continuous; docs synced with each landed phase.

Each phase ends with `cargo check` + the *affected* crate tests only
(per workspace policy), `cargo fmt`, and a docs sync.

### Phase 5: the remaining items, and what measuring them cost

Four of the six open items were closed.  Two were **measured and refused**,
and the measurements are the finding — they are recorded here so nobody repeats
the experiment.

- **The lambda's parent is now an explicit link — LANDED.**  `ExprKind::Function`
  carries `parent: Option<ExprId>`, the frontend reserves the function node's id
  *before* its body compiles (the same reserve-then-stamp discipline a
  block-wide binding already used), and the checker resolves it through a
  `function_of` map.  `depth` is gone from the IR, the frontend and the test
  helpers.  The rule it encoded — a same-depth sibling must hang under nothing,
  or mutual recursion re-applies a never-bound template — moved to the
  frontend, where the parent is now actually decided, and is written out there.
  Agreement with the old arithmetic was **proved by instrumentation**, not by
  reasoning: both parents were computed on every lambda and asserted equal
  across the whole suite and every example, with the instrument itself
  verified live by deliberately breaking the link.  That earned its keep —
  it caught a missed `Expr::TypeOf` push *and* a real latent bug: the
  block-wide transplant copied a value's kind into the pre-reserved
  placeholder without rewiring the ids inside it, so a nested closure's parent
  named a node that is never compiled and never gets a `FunctionId`.  The old
  depth arithmetic was **id-independent** and could not see that; an explicit
  link can, and does.  Fixed with a `repoint`.
- **The arrow / function-type encoding — LANDED, in every crate.**  One
  constructor, `Checker::arrow_parts` (with `Checker::arrow` / `Ctx::arrow`
  delegating), and the symmetric reader `shape::function_type_parts`.  All three
  highlevel build sites and all four `lichen-compute` build sites go through
  it; `JitOp::build` and `LaunchOp::build` no longer hand-build the
  `[marker, universe]` kind.  `Ctx::arrow` does not touch the `arrows` set, so
  no `compute` site started registering one, and all four allocate their three
  nodes in the same order as before — which is why the artifact is byte-identical
  rather than merely example-compatible.
- **Interning `USize(0)` / `USize(1)` / field names — LANDED for the highlevel.**
  `Checker::install_constants` now installs the two constants alongside the
  kind markers, and `Checker::name_node` interns the name nodes, replacing 14 +
  3 per-occurrence allocations.  This also removed an inconsistency: two sites
  called `module.add_node` directly, bypassing `alloc_node`, so a constant
  allocated inside a function body was neither tagged nor registered.  The
  constants are safe to share because they are installed **before any function
  exists**, so they belong to no template and the clone walk returns them in
  place (`function.rs`'s membership test) — the property the kind markers have
  always relied on.
- **Both spec rows — LANDED.**  The two `Annotation` rows now name the real
  payload (`attributes: ChildRange`, positionally aligned with the schema tail);
  `$name(args)` is documented in §2 as what it is — a lexed atom, parenthesized,
  no named-argument form — along with the `plug` directive that was a real
  keyword with no spec coverage at all.

#### Refused, with the number that settled it

- **The four lowlevel structural descents — measured, it does not pay.**  A
  full attempt behind a policy enum cost **+262 lines** before any of the four
  sites even converted; the best design (two of four) was still net positive.
  Four policy enums plus a struct is ~200 lines, each site shrinks by ~12, and
  the per-variant logic could not become a policy field at all — whether a
  `Table` compares by identity is a *variant* question, not a *shape* question,
  so every call site still needed its own `arms` method.  The finding is
  stronger than the line count: **`#[stacksafe]` is a per-entry guard**, so
  splitting one logical comparison frame across a `spine`/`descend`/`arms`
  trio makes each array element allocate another 2 MiB fiber instead of
  reusing the guarded stack.  The lowlevel test `cyclic_keys_hash_and_compare_equal`
  went from 0.84s to **266 seconds** and then died with `unable to allocate
  fiber`.  A correct version would have to funnel the recursion through exactly
  one guarded entry per frame, which puts the array descent back in each call
  site — i.e. un-shares the spine that was the point.  The four walks stay
  duplicated, and the duplication is now understood rather than merely noted.
- **The `lichen-compute` subscript constants — deliberately left per-occurrence.**
  They are allocated inside a native operator's `build`, which runs with a
  current function on the stack, so a shared node would be tagged into the
  operator's own template scope and **cloned per apply** instead of referenced
  in place — the opposite of what sharing buys.  That is a semantic change, not
  a port, and the two read sites also must keep their lazy `Index` chain
  (an unbound signature resolves at apply time), so `shape::function_type_parts`
  does not apply to them either.  Recording the reason beats guessing at it.

- **The checker's IR is now read-only by construction — LANDED.**  `Checker`
  holds `Arc<IR>` and `Build` hands the same handle back out, so the Phase 4
  contract is enforced rather than merely true: the rewrite that broke it
  (`self.ir.set_schema`) compiled before, and `self.ir.set_kind(…)` no longer
  does — measured, `E0596: cannot borrow data in an Arc as mutable`.  The four
  public entry points still take `IR` by value, so no caller changed, and every
  reader on `build.ir` is a field or index access that `Deref` passes through
  unchanged.  **One route is left open on purpose**: `Arc::get_mut` still
  compiles, because the reference count is 1 for the whole check.  Closing it
  means either keeping a second live handle for the duration or threading an
  `&IR` lifetime onto every `Build` consumer across three crates — a far larger
  change than the residual gap is worth, and the escape is two deliberate steps
  that name themselves in review, where the accidental form was one call that
  type-checked.

- **The attribute's `missing_slot` node — measured, there is nothing there.**
  The last open item was whether to intern the node an absent attribute reads
  (`AttrExt::missing_slot`, `attr.rs`), by analogy with the `USize` constants.
  A design review recommended **not** doing it, and improved on the original
  reasoning: the barrier is not that `Doc`'s missing value is "not a `USize`"
  but that it is `Parameterized` — **an unbound cell `unify_slots` binds by
  design** (`doc.rs`), so sharing one across occurrences would let the first
  bind poison every later read.  That is a correctness argument, and it holds
  (`unify_slots` calls `check_unify_relaxed`, which binds).  The separate
  objection also holds and transfers from the compute refusal: the node is
  allocated through `Ctx::value_node` → `alloc_node`, so inside a lambda body
  `alloc_node` tags it into that function's template and the clone walk copies
  it per apply — the opposite of what sharing buys.

  **Then the premise itself turned out to be wrong, and by measurement.**  The
  review's cost estimate was "attributes × expressions, low hundreds of nodes".
  Putting a `panic!` in `Doc::missing_slot` and confirming the string was in the
  binary before running anything: **it never fires** — not for any of the 23
  examples, not for `doc.lichen`, not for `perspective.lichen`, and not for
  four probes written specifically to reach it, including `c = [5, 6] # 4`
  where the value carries no doc at all (the `else` arm at
  `annotations.rs:268`, where `value_attr_node` returns `None` and the missing
  slot is demanded).  Substituting a distinctive `USize(999)` for the missing
  value changed no output either.  So the cost being debated is **zero** on
  everything observable, and the hazard the review describes is currently
  unreachable rather than merely rare.

  **The `else` arm does fire — for a constraint, never for a label.**  Putting
  the same `panic!` in `Perspective::missing_slot` and confirming the string
  was in the binary, `f = x => x` / `a = 3 # 4` / `r = f a` hits it dead on
  (`lambda.rs:267-275`: the argument carries a constraint slot and the
  function's parameter does not, so the declared side is the attribute's
  missing slot).  It is not dead code, and it is **covered — by three tests in
  `tests/perspective.rs`** (`a_missing_child_reads_zero`,
  `an_annotated_parameter_accepts_a_uniform_argument`,
  `an_identity_function_rejects_a_perspective_argument`) while being reached by
  **none of the 23 examples**.  A label cannot reach it, and the reason is
  structural rather than accidental:

  - `check_ann` iterates the **merged tail** — the union of what the value
    carries and what the annotation spells — not a padding out to
    `AttrSet::ORDER`.  An attribute that neither the value nor the annotation
    mentions therefore has **no slot in the pair at all**, so nothing ever
    asks for one.  (The pair's width tracks the tail's length, while
    `attr_slot(i)` is an *absolute* index from the marker's order, so a hole
    is skipped rather than shifting later slots.)
  - Where a slot *is* absent, the two callers either preserve the value's own
    slot (`value_attr_node`, the `else` arm at `annotations.rs:266-268`) or are
    gated on `self.attr[e]` being set — and `annotations.rs:269` only sets it
    `if !ext.is_label()`.  A label never populates the slot a constraint is
    found through, so `Doc` can satisfy neither condition.

  **The interning question has a split verdict, and the measurement is what
  showed it.**  For `Doc` there is nothing to intern: the site is unreachable.
  For `Perspective` the site is hot in the test suite, its missing value
  (`USize(0)`) is immutable, and interning is defensible on the top-level
  subset — but the compute refusal's objection still bites the in-lambda
  subset, where `alloc_node` tags the node into the enclosing template and the
  clone walk copies it per apply.  That leaves a top-level-only win of unknown
  size, and nothing has measured it as worth taking.  The honest disposition is
  therefore: **refused, and now for a reason that distinguishes the two
  attributes instead of lumping them together.**

- **The attribute owns its missing value — LANDED, as a mechanism rather than
  as a reason to stop.**  "The missing value is the attribute's to define" is
  the design, and the first version of this work used it as a *reason not to
  build anything* — which is not the same thing.  It is now the thing itself:
  [`AttrExt::share_missing_slot`] lets an **attribute** say whether its own
  missing slot may be one shared node for the whole build, and the checker only
  asks and caches the answer.  The contract is stated as the fact it is — *a
  shareable missing value must be **concrete*** — because reconciling two slots
  is a real unification that writes whichever side is unbound; a concrete node
  is only ever read, so one can serve every occurrence, while an unbound one
  would be written by whichever occurrence reconciled first.  That single fact
  is what separates the two attributes: a perspective's absent form is the
  constant `0` and opts in; a doc's is an unbound cell that a unify binds on
  purpose, and keeps the per-site form.  Default `false`, so an attribute that
  says nothing behaves exactly as before.

  The checker holds `Vec<Option<NodeId>>` indexed by `order_index()` and fills
  it **on first use**, not in `install_constants`.  That ordering is the
  measured part: an **eager** install costs one slot on *every* build, and
  since this site is cold it spent 30 builds across the 23 examples and 34
  across the perspective suite — a 6× regression on the suite and +24 on the
  examples, to save them only on the few programs that ask.  Lazy, the cost is
  zero on programs that never have an absent attribute.

  **The honest size of the win, measured: 6 missing-slot builds across the
  whole perspective suite become 5.**  One.  The mechanism is in place and the
  decision is where it belongs, but the site is cold, and a reader should not
  mistake this entry for a performance improvement.  What it buys is that the
  next attribute whose missing value *is* hot has one line to write.


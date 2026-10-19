# The scope perspective, and what MMA needs that the value lattice cannot say

This note is the decision record between the `Perspective` work that is landed and the
MMA item on the roadmap. It states what is enforced today, what the GPU path reads, why
the kernel-config annotation fails, and the options for closing it. Measurements are on
dev at `3231542a`.

## 1. What is enforced today

`Perspective` is a divisibility lattice on a value's lane uniformity: `# p` states "the
same across every `p`-aligned lane group", `0` is the absent top, `1` is lane-varying.
The acceptance table is [attributes](attributes.md) §"Behaviour"; the enforcements are:

- a parameter's declared `# p` is checked at each apply (`declared | provider`), and an
  absent parameter annotation states nothing (the isomorphic reading);
- a function's return certificate rides to the call site, including through a frozen
  (package) callee — the compute wrapper's `# 1` seeds on `range`/`row`/`col`/`read`
  make a false `# 64` claim a check-time refusal;
- a lane-group requirement is spellable in the shipping vocabulary:
  `f = (x : Int # compute.warp) => x` answers `expected 32, found 1` for a lane value,
  and accepts a uniform value or a `# 64` claim (`compute.warp = 32`, the backend's
  `SUBGROUP_WIDTH`).

## 2. What the GPU path reads

**Nothing.** The kernel IR carries no attribute field, the emitters read no perspective,
and `compile_parallel_fragment` hardcodes its lane width. [Axis
C](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for)
is the intended first consumer (hoist a uniform subexpression, and emit it as one where
the target has a vector width); it is a codegen feature, not a check. The checks above
are the checker's; they constrain programs, but no GPU code is generated *from* them yet.

## 3. The paper's requirement is not this lattice

The reference document types `wmma` with `requires(thread[32])`: the operation involves a
32-lane group, and the *fragments* are lane-varying inside it. `Perspective` says a value
is uniform across an aligned group. The two statements are different axes:

| statement | reads | example |
|---|---|---|
| value uniformity | the value's lanes are identical | `# 32` on a tile base |
| scope requirement | which lanes participate in one operation | `requires(thread[32])` on `wmma` |

A warp-level MMA operand needs the second. Encoding it as `# 32` on the operands would
be wrong — a fragment is lane-varying — which is why [attributes](attributes.md)
§"A lane-group requirement" calls the landed check the *uniform-load* shape.

## 4. The kernel-config annotation, measured

`even_raw = (k : ParA # 4) => …`, where `ParA = compute.P (compute.KT Float)(…)`:

- **without** the `#`, the program checks and runs (`[2.0, 4.0, 6.0, 8.0]`);
- **with** it, the build fails with a **runtime** diagnostic: the generic apply-time
  parameter-check rendering (`diagnostic.rs::mismatch`'s apply arm), located at the
  argument's expression with descent path `Attr(0)`, whose two sides are function types —
  `raw[?a, raw[?b, ?c]] -> raw[?d, ?e]` against `struct{…} -> ?j`. The kernel-type merge
  is what sees the annotation, not an attribute check.

What was ruled out by instrumentation: the annotation provider path, the element-pair and
type-attribute reconciliations, and the frozen-parameter tail check do not fire for it;
disabling the frozen return-attribute propagation does not change it; `check_unify` and
`check_unify_relaxed` never see the failing pair. A computed type with a perspective
works in the bare language (`A = T 5; f = (s : A # 4) => s.n`), so "computed type +
perspective" is not the trigger — the kernel path is.

## 5. Options

1. **Refuse by name at check time.** Recognize the kernel-config parameter (the
   `compute.P`-shaped type is plugin knowledge, or is recognized by the parameter's type
   value being a type-level function) and answer with a diagnostic instead of the runtime
   raw unify: "a perspective on a kernel config parameter is not supported: the config is
   uniform by construction and a scope perspective is not built." No semantic change; the
   author's pending question answered on the conservative side.
2. **Support as a scope declaration.** `(k : ParA # p)` declares the kernel's lane-group
   width: the launch asserts `config.n % p == 0` (the deferred-assert channel already
   re-checks per call), and a warp-level requirement references `p`. Needs an
   attribute-tolerant kernel-type merge (the measured failure), the scope semantics, and a
   deferral story under "the editor evaluates nothing outside check mode".
3. **Support as a value uniformity.** Accept the annotation and treat the config as
   `# p`-uniform (true by construction, since a launch parameter is uniform). Cheap, but
   the claim is vacuous and it still needs option 2's merge tolerance.

**Recommendation.** Option 1 now (a named message, no semantics); option 2 as the MMA
prerequisite, sequenced after or with Axis C — the same lane-group question
([shared-memory-and-barriers](shared-memory-and-barriers.md)).

## Open questions for the author

- Option 1 or 2 for the config annotation, and now or after the scope design?
- Axis C first (the roadmap's recommendation) or the scope perspective first (the MMA
  item's prerequisite)? They touch different machinery (codegen vs the kernel-type merge).

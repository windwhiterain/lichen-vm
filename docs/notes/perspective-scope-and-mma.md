# The register perspective MMA needs, and the scope question behind it

This note is the decision record between the `Perspective` work that is landed and the
MMA item on the roadmap. It states what is enforced today, what the GPU path reads, why
the kernel-config annotation fails (a separate diagnostic-quality finding, not the MMA
path), and the register-side shape the author has ruled is the right home. The verdicts
are recorded in §6. Measurements are on dev at `3231542a`.

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

### What "the emitter consumes it" would take

Two mechanics, both larger than they look:

- **A hoist.** The SPIR-V walk emits every expression per invocation. A value proven
  uniform could be computed once per *workgroup* and shared instead — either on the host
  (the config leaves already ride push constants, so the machinery exists for leaves) or
  by one lane writing a workgroup variable behind a barrier. The first needs a host-side
  evaluator for the body expression; the second needs shared memory and a barrier, which
  the backend does not have yet ([shared-memory-and-barriers](shared-memory-and-barriers.md)).
- **A vector form.** "Emitted as one where the target has a vector width" means the
  value is uniform over the vector's lane group and the operation is vectorizable; the
  emitter would have to build `vec4` values from scalars, which changes the whole value
  model of the shader (loads, stores, arithmetic and the buffer layout).

Neither changes the *answer*. A body that ignores the perspective computes the same
result, one element per lane — which is why the author's reading (§6) holds: the emitter
consumes a perspective only as a performance factor; MMA consumes it because the lane
layout *is* the semantics.

## 3. The reference's model, and what lichen covers

The reference document types an expression as `expr: type(DP) @ UP` over a *scope* form
`level(n)` (`grid`, `block`, `thread`, with `warp(n) ≔ thread(32n)`), and it lists four
CUDA contracts: uniform parameter, disjoint mutation, synchronization, and convergence (a
cooperative function such as `wmma` needs a contiguous run of threads). The pieces:

| piece | reads | lichen today |
|---|---|---|
| `UP`, uniform perspective | the value is identical within the group | **this** `Perspective` — divisibility; the paper's `divisor` is lichen's `gcd`, and `lower` (a coarser value serves a finer requirement) is `declared \| provider` |
| `DP`, disjoint perspective | two thread groups' mutable regions do not overlap | absent — `Buf` carries no disjointness; the ladder's silent scatter is the cost |
| scope and `@requires` | the launch aligns `grid`/`block`/`thread`; `lower` moves a value to an aligned group | implicit: the kernel is dispatched with a fixed lane width (64, with a 32-lane subgroup), so `thread[32]` exists for every body |
| convergence | a cooperative operation needs the contiguous group | the dispatch supplies it, as the author ruled (§6) |

So the *register* side the author named — "寄存器的 perspective" — is the `UP` column, and
the value lattice is exactly that: the paper's `int @ thread[32]` is lichen's `# 32`, and
the `lower` relation is the acceptance table's `declared | provider`. `DP` is a *memory*
axis carried by the type (the reference encodes it in `view`, `ptr`, `shared`), not a
value annotation; it is the check that would catch the ladder's silent write, and it is a
separate item.

**MMA's checks, then.** The tile descriptor must agree across the group — `# 32` on the
`UP` axis, landed; a fragment's lane layout is the value's own perspective — the default,
with no requirement form (see the test in
[perspective.rs](../../crates/lichen-language/tests/language/perspective.rs): a warp-level
operation keeps the fragment's finer width, and the result claim is what a checker
refuses); and the convergence is the dispatch's. Encoding the convergence as `# 32` on
the operands would be wrong — a fragment is lane-varying — which is why
[attributes](attributes.md) §"A lane-group requirement" calls the landed check the
*uniform-load* shape.

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

**Recommendation.** None of the three is needed for MMA (§6): the config annotation is a
diagnostic-quality question. If it is pursued, option 1 (a named message) is the only low
-cost one and does not change semantics; option 2 stays parked with the kernel-type
merge.

## 6. The author's verdicts

- **Registers, not the config.** "为什么mma需要 config 的 perspective，寄存器的
  perspective 就够了吧" — a fragment's lane layout is a value fact, and the group is the
  dispatch's; the config annotation is not on the MMA path. The register-side check the
  value lattice already states is the uniform-operand requirement (`# compute.warp`), and
  the lane-varying fragment is the default.
- **The op states its perspective, uncurried.** The vocabulary's `mma` takes one `Mma`
  struct argument — not four curried lambdas — with the three tiles required warp-uniform
  (`# warp` on the parameter) and the result certified `# 0`. A lane-varying operand is
  refused at the call (`expected 32, found 1`), which is the paper's descriptor
  requirement; the lane-varying *fragments* stay outside the host-buffer op, since a
  buffer is uniform over every lane.
- **Only MMA semantically consumes a perspective.** The emitter's hoist/vectorize (Axis
  C) is an *optimization* — [the roadmap](gpu-algorithm-roadmap.md) itself calls it "a
  constant factor, not a capability": a body that ignores the perspective still computes
  the right answer. The *semantic* consumers are the collectives (matrix multiply,
  shuffles, reductions), where the lane layout decides the result. So the emitter item
  should stay parked unless the performance factor is wanted, and the MMA item's next
  step is the *checking* shape, not the codegen.

## Open questions

- Is the config annotation's named refusal worth a check-side hook now, or does the
  runtime raw message wait until the kernel-type merge is visited for its own sake?
- Should Axis C stay parked as an optimization, or be scheduled as a performance item
  when a uniform-heavy shape exists to measure?

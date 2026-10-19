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

**Where the scope requirement actually lives.** It is not a program annotation. A kernel
is dispatched with a fixed lane width (this backend: 64-lane workgroups, a 32-lane
subgroup), so the 32-lane group exists by construction for every body — the paper's
`requires(thread[32])` is satisfied by the dispatch, not by the config. What the program
side can and does state is the *register* side: a value the whole group must agree on
(the tile base, a uniform load) is `# 32`; a fragment's own lane layout stays the value
lattice's business. That is the author's ruling in §6.

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

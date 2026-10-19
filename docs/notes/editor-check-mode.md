# The editor checks without running

> Status: current. Points at `crates/lichen-language-server/src/analysis.rs` (the
> `Mode::Check` build and the snapshot renderers) and `crates/lichen-highlevel/src/checker.rs`
> (`Mode`). Companions: [incremental-evaluation](incremental-evaluation.md) Q9,
> [apply-type-side-check](apply-type-side-check.md).

The author's ruling: **the editor evaluates nothing outside check mode.** `analyze` builds in
`Mode::Check`, and no editor request runs a value slot — not for diagnostics, not for hover, not
for the statement-values panel. The reason is the one the mode exists for: a body may diverge,
and a green editor must mean "the types hold", never "we happened to finish running it".

## 1. What a hover renders

A snapshot's `value : type` appears only when the **check build decided the value** — a literal,
a type constant, a field read of decided cells. Otherwise the hover renders the type alone:

```
`x` — `3 : Int`          -- the literal is decided at check time
`y` — `Int`              -- `y = x + 4` is a routed call: its value is never run
`.double` — `raw[?a, ?b] -> raw[?a, ?b]`   -- an open cell, rendered as the type names it
```

That is the whole contract: there is no on-demand evaluation to fall back on, so a value the
check build did not decide is reported as absent, not computed later.

## 2. The statement-values snapshot is index-time

`statement_values()`/`statement_at()` return what the cascade decided at index time, and their
invariant is unchanged ("a statement the cascade left lazy reports `value` as `None` and its type
only"). The `compute.jit` statements show the boundary from both sides: `k_double`'s literal
chain is decided (`(raw none, Int, Int)`), while `k_outer`'s `.O` cell — a kernel's return type,
pinned only by the value side — reads `raw none` and its type names the cell as `raw[?c, ?d]`.
The `raw none` for a kernel's `.native` is the check-time reading too: the kernel value is the
`compute.jit` call's result, undecided until the definition pass runs it.

## 3. What this costs, and what it does not

- **Costs.** A hover no longer prints the value of a routed operator or a kernel-returning call;
  the statement panel reports types only for `compute.jit`'s kernel results. Those are the
  readings `Mode::Check` does not decide.
- **Does not cost the diagnostics.** A `Check` build still reports a builtin's refinement failure
  (`apply-type-side-check.md` §6) and still reports a routed call's type conflict; the assert
  evaluation that carries them is bounded by the depth budget, never by a wall clock.
- **Does not force the program.** The lazy-binding test (`a_lazy_binding_reports_type_but_no_value`)
  completes in milliseconds: a deferred binding's value node is not recorded, so the snapshot
  reads `None` and nothing is evaluated.

## 4. One rendering dependency

`module_field_definitions_resolve` hovers a module file's `succ = x => x + 1` and expects an
arrow. A lambda's type is its self-referential pair `[Function(fid), ↺]`; `D -> C` is the
**rendering** of that pair by reading the function's parameter and return type cells — it is not
a value the editor should evaluate. Until the type printer renders the pair as an arrow, that
hover reads `Function : raw[Function, …]` and the test stays red. That is a rendering-line item,
recorded here so the editor's contract is not mistaken for the cause.

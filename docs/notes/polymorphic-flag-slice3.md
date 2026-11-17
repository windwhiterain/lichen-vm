# Polymorphic flag, slice 3: scope-relative clone selection (the mark-scope antichain)

> Status: **landed** on `feature/polymorphic-flag` (commits `f3d9507e`, `f2be6ee1`,
> `095fa71c`, `70dc2da1`), aligning the code to the committed scope-relative SPEC
> (`8c730ac6`). The rulings are [the contract note](polymorphic-flag.md)'s; slice 2's
> absolute-flag selection is [polymorphic-flag-slice2](polymorphic-flag-slice2.md)'s;
> the struct-field creation mark is
> [polymorphic-flag-struct-freshness](polymorphic-flag-struct-freshness.md)'s. This note
> records the antichain mechanism, the two measurements that forced its shape, the perf
> canary, and the apply-output workaround reassessment.
>
> Points at: `crates/lichen-lowlevel/src/polymorphic.rs` (`compute_polymorphic`, the
> antichain fixpoint; `antichain_insert`), `crates/lichen-lowlevel/src/function.rs`
> (`node_polymorphic_over_scope`, `node_apply`), `crates/lichen-lowlevel/src/template.rs`
> (`template_class_is_cloned`), `crates/lichen-lowlevel/src/lib.rs`
> (`Node.polymorphic_scope`).

## 1. The rule the code now implements

SPEC §Polymorphic/§Clone (committed `8c730ac6`): a node is polymorphic **over a function
scope** if it was created marked in that scope or recurses into a node polymorphic over
that scope; a clone walk clones a node iff it is **inside the applying scope or a
children-scope** *and* **polymorphic over the applying scope or an ancestor-scope**.

A node reaches marks in several scopes at once, so its verdict is the **antichain of
maximal mark scopes** it reaches (`Node.polymorphic_scope: Vec<Arc<Scope<FunctionId>>>`).
An ancestor mark subsumes a descendant for the applying-or-ancestor test, so
`antichain_insert` drops any scope whose ancestor is already present. The bool flag is
the antichain's erasure (non-empty); the frozen mirror derives it. Selection reads
`any mark scope.is_ancestor_or_self(applying_scope)` (`node_polymorphic_over_scope`,
`template_class_is_cloned`) conjoined with `node_inside_scope` (which already spans
children-scope).

## 2. Two measurements that forced the shape

1. **A single outermost mark scope is insufficient** (`f3d9507e` → `f2be6ee1`). The first
   attempt stored one outermost scope per node; it broke `v3`
   (`mk := 'x -> 'y -> x + y; f := 'n -> (mk n) 1; f 3` → `none`). A body cell reaches marks
   in **incomparable** scopes — its own function (param `y`), the capture's function (`mk`,
   via `x`), and the core `+` operator's scope. Collapsing to one scope picked the core
   operator's depth-0 scope, which is not an ancestor of the applying inner-closure scope,
   dropping the marks the clone condition needs. The antichain keeps the incomparable
   maxima; condition (2) is an *any*-test over them.
2. **A nested function value is seeded over its defining (parent) function** (`095fa71c`).
   The antichain alone under-cloned `outer_call_returns_a_nested_function_value`: the
   closure value reaches only its own function's marks (a *descendant* of the applying
   enclosing function). A nested function value is a per-call closure minted in its
   parent's scope (slice 2's ruling), so the propagation seeds it with
   `function_scope(parent)`. A top-level function value is still an identity, referenced in
   place.

## 3. Perf canary (the balloon check)

The antichain fixpoint respects `evaluate_depth_limit` and keeps the sets pruned. The
infinite-stream budget test
(`recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget`) stays fast:
`test result: ok. 1 passed; 0 failed; … finished in 0.05s` — unchanged from the absolute-flag
baseline, no D18 hang.

## 4. The apply-output workaround is still required (reassessed)

Slice 2's apply-output freshening (`fad34c2b`, in `c02b8b46`: `mark_apply_output` /
`is_apply_output`) was tested for subsumption by removing the `is_apply_output` branch from
`node_apply`. `ideal4`/`any4` then refuse `expected Int, found Float` again: the apply's
output cell (operand slot 2) is a fresh unmarked cell that does **not** reach a creation mark
over the recursion set (it is wired to the return, not a structural operand/item that
propagates marks), so the antichain leaves it non-polymorphic. The special per-call
freshening is therefore **kept**. The hypothesis that the definition pass's class union
would make it polymorphic via class-member recursion did not hold.

## 5. Acceptance (the tool's own lines)

- `ideal1` `6: Int`, `ideal2` `2: Int`, `ideal3` `2: Int`, `ideal4` `(2, 0.5): Tuple`,
  `ideal5` `Int: Type`; `any3` `(2, 2)`, `any4` `(2, 0.5)`, `any6` `((1, 2), (1.5, 2.5))`,
  `any7` `(43, 44)`.
- All 22 [nested-apply-none](nested-apply-none.md) §27 reconstruction probes answer their
  recorded values (`v3` `4: Int` again).
- `cargo test -p lichen-lowlevel`: `167 passed; 0 failed; 1 ignored`.
- `cargo test -p lichen-highlevel --test highlevel`: `136 passed; 0 failed`.
- `cargo test -p lichen-language --lib`: `69 passed; 0 failed`.
- `cargo test -p lichen-language --test language`: `284 passed; 5 failed` — the same five
  pre-existing names as slice 2, zero new.
- Examples unchanged vs `c02b8b46` (struct/struct_generic/struct_recursion/let_polymorphism/
  placeholder/closure/nested_function/operators/dependent_type/recursion/mutual_recursion/gcd).
- `comments-check --config comments-test.toml . --max-violations 0`: no violations;
  `cargo fmt --all -- --check`: clean.

## 6. The frozen mirror

`static_node_apply` / `FrozenClassRule.inside` keep reading the frozen **bool** (the
antichain's erasure): a frozen function is self-contained, so its marks' scope is the
applying scope and the erasure coincides with the scope-relative verdict. The freeze
derives the bool from the antichain (`freeze.rs`).
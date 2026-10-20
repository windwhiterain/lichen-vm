# Open issues after the slot isomorphism

> **Status:** current. The attribute slot is handled exactly like the type slot — every
> expression term is a full-width `[value, type, attributes]` slot, every reader takes a
> slot the same way, and the checker states its facts instead of reading values that
> the check has not decided. The evidence is the parity suite: **`lichen-highlevel`
> 136/136** (the suite built for exactly this question), `lichen-language` **274/275**,
> `lichen-lowlevel` **164** (one ignored), compute JIT **30/40**. This note lists what is
> still open, in one place, and points at the measurements rather than repeating them.

## 1. A recursive binding inside a block does not check

`a_recursive_binding_inside_a_block_recurses` (in `crates/lichen-language/tests/language/pipeline.rs`) is the only red language test:
`f = y => { g = z => if z <= 0 then 0 else g (z - 1); g 3 }; f 5` stops on the
checker's apply-depth budget instead of answering `0`.

The measured chain, in order:

| Round | Finding | Where |
|---|---|---|
| 21 | the failure was a stale-value conflict (`USize(3)` against `USize(2)`); its two nodes are clones of *different* templates whose classes a parameter check joined | [§15](shallow-depth-element-values.md) |
| 24 | the second call's clone was cloned *from the first call's clone*; the minted closure's template parameter was the very node the enclosing call binds | [§17](shallow-depth-element-values.md) |
| 25 | **fixed**: the mint owns its parameter and its own scope's clones (captures stay shared) — the stale-value conflict is gone; the recursion now unrolls past the depth budget because every recursive level passes the **same argument node** | [§18, §19](shallow-depth-element-values.md) |
| 26 | the argument pair is a *minted* template node whose value was carried from the enclosing call (`z - 1` at `z = 3` gives `2`), and the mint's `carried` rule copies it into later templates. Two clone-rule refinements landed (scoped-ness judged on the template root; a clone is proven only when its template is) — both measured unchanged | [§20](shallow-depth-element-values.md) |
| 27 | **negative**: suppressing the carry for a mint's own scope makes this test pass but loses the enclosing call's applied values for captures; the scope list holds the captures' decided copies too, so scope list, function tag, and block chain all fail to separate them | [§21](shallow-depth-element-values.md) |

**Next step:** the distinction must be a *value-level* one — a stale answer is a slot's
value produced by a previous *call*, a capture is a fact the enclosing call holds — not
a scope, tag, or block property. The places to look are
`crates/lichen-lowlevel/src/function.rs`'s `node_apply` (`carried`,
`proven_concrete`, `minted_scope`) and `instantiate_stamped`'s regrouping.

## 2. The compute graph block (deferred by the user's choice)

Ten `graph_jit` tests in `lichen-compute` are red (JIT **30/40**). The measured routes
are in [graph-recording-rounds](graph-recording-rounds.md): the rule-side identification
is **dead** (no dynamic `Write` op is reachable, the apply's argument value is an empty
cell, and the rule side holds no identity). The stated design is a **checker hand-off**:
the check records the written class of a `compute.write` call's `.value`, keyed by the
apply node, and the recording reads that class instead of deriving it from the value
graph. It is a bounded feature and it is *not* part of the slot isomorphism.

## 3. Nothing else

Every other suite is green; the four crates' counts above are the reading. Smaller
defects found along the way are recorded where they belong (the apply-time diagnostic
renders the compared *types* rather than the compared values — see
[the apply type-side check](apply-type-side-check.md) for the check that the type-side
merge is load-bearing for refinement reporting).

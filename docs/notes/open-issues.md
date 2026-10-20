# Open issues after the slot isomorphism

> **Status:** current. The attribute slot is handled exactly like the type slot — every
> expression term is a full-width `[value, type, attributes]` slot, every reader takes a
> slot the same way, and the checker states its facts instead of reading values that
> the check has not decided. The evidence is the parity suite: **`lichen-highlevel`
> 136/136** (the suite built for exactly this question), `lichen-language` **275/275**,
> `lichen-lowlevel` **164** (one ignored), compute JIT **30/40**. This note lists what is
> still open, in one place, and points at the measurements rather than repeating them.

## 1. The compute graph block is red — a regression of the slot encoding

Ten `graph_jit` tests in `lichen-compute` are red (JIT **30/40**). What matters is the
change of the failure, not the count:

| point | reading |
|---|---|
| `9ed0ee4b`, the attribute squash | the ten `graph_jit` tests **pass** — 10/10, measured in a detached worktree |
| `4645b439`, the pair-shaped slot | JIT **29/40**, "was 20/40": the drop happened in the compound-slot commits *after* the squash |
| now | JIT **30/40**; nine of the ten stop in the low type, one on its refusal message |

So the earlier diagnosis — [graph-recording-rounds](graph-recording-rounds.md), measured on
`e659607e`: no dynamic write, an empty argument cell, the rule side holding no identity —
describes the *pre-squash* failure and no longer matches the tree.

**The shape question is settled** (`a2fd2796`, the ruling in
[lowlevel-low-types](lowlevel-low-types.md) §8): a class states the variant of its node's
value, and the highlevel judges it from the node's type node. The ten tests no longer stop
on the "a class states one shape" assertion.

**What they stop on now — the lowering's own reading of the compound-slot encoding.** The
role walk is right (`roles.outputs = [[2, 0]]`: the `.out` field, its `.z` buffer), but the
write's `.to` no longer resolves to that path. Read from the code, not yet measured:
`param_path` pushes one step per `Index`, while the checking encoding reads a field's slot
and then its value position, so the extra dereference becomes a spurious field step. The same encoding shows up twice more in the ten failures: a read over
a pair reports "this value is not a container — it has no element to read", and a kernel's
domain reads undecided (so the polymorphic-template diagnostic fires on a monomorphic
kernel). The fix is therefore in `lichen-compute`'s readers (`param_path`,
`peeled_argument`, the argument/codomain walks), not the checker hand-off; the hand-off's
design still stands for after that.

## 2. Nothing else

Every other suite is green; the three counts above are the reading. Two smaller defects are
documented rather than left open:

- The apply-time diagnostic renders the compared *types*, so a value-level conflict recorded
  through the apply edge prints both sides alike ("expected Int, found Int"). Left as a
  documented limitation: no program reaches it today — the one that did was the stale-value
  conflict the mint fix closed
  ([shallow-depth-element-values](shallow-depth-element-values.md) §22) — and the smaller
  fix (fall back to the compared values when the two sides read alike) buys little.
- The apply's type-side value merge is load-bearing for refinement reporting
  ([apply-type-side-check](apply-type-side-check.md)).

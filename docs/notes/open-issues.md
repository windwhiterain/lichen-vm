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

**Three seams are fixed** (`6c07f147`): `param_path` counts field selections rather than
the encoding's value reads, `assemble_parameter` builds a structure of slots, and
`place_of_node` walks through type cells. Graph JIT is **1/10** (was 0/10); the whole
compute suite is **97 passed** (was 81 at `7ac3e639`'s predecessor measurement).

**Where the compute block broke — measured by bisect.** The whole suite was **250/253**
at `9ed0ee4b` (the attribute squash) and **81 passed** at `7ac3e639`. Two commits carry
the drop:

- `5146f747` — "the applied clone's return pair states the call's attribute" — **250 →
  247**: the frozen callee's return pair remains a hole at check time, so a call's
  attribute falls back to the argument's whole provided slot. Its own message names the
  follow-up ("the check-time materialization is the follow-up if the field-level reading
  is wanted back"), with the cost it was deferred for: a full frozen-body clone per call
  site.
- `dabf4e2c` — "a struct value's fields are complete slots (red checkpoint)" — the
  collapse. Its message records the plan: "Next: the renderer reads an element's
  value/type/attributes from the slot's own positions …, then the same slot schema for
  arrays and tuples, then the compute readers (`known_shape`/`flat_arity`/`class_at`)."
  The language half is done (275/275); the compute readers are the open half — the three
  seams above are its first three, not the last.

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

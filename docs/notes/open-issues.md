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

**The reference is definition-time shape data again** (`195600fa`): `5146f747` deleted
`return_field`/`return_marker` (their static mirrors and artifact fields), and the
applied-pair design cannot carry the reference for a frozen callee — the clone's return
pair does hold the reference slot, but its first item is a static ref that cannot be
rebound per call, so the certificate reads a hole. Measured, the frozen return pair's
attribute *is* the reference (`StaticFunctionId(28)`), which is why restoring the shape
works: the marker and the parameter field are recorded on the function (artifact format
20), the checker's `attribute_reference` analysis fills them, and the call site resolves
`argument_field` and takes that field's own provided attribute. `a_write_returns_the_
input_buffer_perspective` is green; compute is **99 passed** (was 97).

Also reverted: `640b2705`'s type-first provided slot. A **type** states the *declared*
requirement, not the provided value — the read test's lane-varying index proved it: the
whole-argument combine must keep reading the field expressions.

What stays open is the **whole-parameter reference** (`perspective_of x`, `read`'s
return): its call site falls back to `provided_slot(argument)`, whose combine walks an
instantiation's `[type, value]` children and then only the record's value expression, so
a struct's field perspectives are never met and a lane-varying index stays warp-uniform
(`a_read_takes_its_perspective_from_the_buffer_and_the_index`). The follow-up is the
combine's field walk — the same one `provided_slot` needs for any compound argument.

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

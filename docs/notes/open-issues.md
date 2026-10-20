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

**What they stop on now — measured.** The failing reads are the kernel bodies' own
(`k.out.z`, `k.in.b`, `k.out.w`), reported as "this value is not a container — it has no
element to read" during `compile_parallel_fragment`'s deep evaluation of the body. A
temporary print (removed) traces one chain: `k`'s pair → its value → the `.in` slot → the
field's value → the `.b` slot → the value; the last container's **class holds a bare
`USize(32)`**, with two members — one a template node of the kernel's function, one
untagged. The checker's chain expects a pair at that position, so the value the body is
evaluated against carries a bare scalar where the compound-slot encoding puts a pair: the
array the chain reaches is a **bare-element array**, its element a node holding `USize(32)`
(the prelude's `warp`), while the checker's chain reads that element as a pair. This is the
task `4645b439`'s own message names for the lowering-built values ("they must be built as
slot arrays too"); the next step is to find the builder of that field array (the `.in`/`.out`
value the body reads) and slot-encode it.

The role walk itself is right (`roles.outputs = [[2, 0]]`: the `.out` field, its `.z`
buffer). The `.to` resolution and the undecided-domain refusals follow from the same
broken reads: once a read yields the wrong value, the write's target names no output and
the kernel's domain reads undecided. So the fix is in the attribute dispatch the compute
prelude's `# warp` exercises, not in the checker hand-off; the hand-off's design still
stands for after that.

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

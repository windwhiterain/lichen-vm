# A colliding answer is a unify error

**The maintainer's ruling.** When two answers for one node disagree, the engine reports a unify error, and
that is the intended behaviour — *"答案相撞就 unify error 啊，有什么问题"*. Nothing in the engine is wrong
here, and no rule about class values or recording cells changes because of it.

## What the disagreement looks like, measured

Thirteen tests in `crates/lichen-compute` fail because their program **does not check**, and the diagnostic
is exactly this collision. The chain, each step measured on
`pack_placement::an_unmatched_pack_is_inert`:

- the location is the program's own line 13, `kernel = compute.parallel (…) "gpu"`, on the `ParLaunch` node
  `n2740`;
- one class holds **six members from six different origins** (`n2453`, `n2450`, `n2456`, `n2018`, `n2021`,
  `n2024`); one of them, `n2983`, already holds an array value, and `propagate_class_value` writes that
  value into all six;
- `n2740` then computes its own answer, and `write_node_answer` unifies the two — a fresh array against the
  propagated one. Their element 0 is a **recording placeholder** on each side, `GraphValue(1)` against
  `GraphValue(0)`, so the two arrays are not structurally equal;
- the unify reports the conflict: `UnifyError` built by `record_error`, whose roots are `(node, node)` **by
  construction** in `write_node_answer`, with the real pair in `a`/`b`.

So the two sides are two evaluations of one correspondence, each minting its own cell, and the engine
refuses their disagreement. That refusal is the ruling's point.

## What did change, and what did not

Three diagnostic defects were found on the way and fixed, all without touching the judgement:

- a conflict raised by an **engine-side** unify has no diary entry, and the generic branch of `mismatch` took
  its location only from that entry — so it printed `expected ?, found ?` with no span at all. It now falls
  back to the descent the error itself recorded (the deepest step's operands, then the trigger's root, each
  through `node_loc`), so those diagnostics carry `span: Some((13, 10))`;
- the same branch forced `value_a`/`value_b` to `None`, discarding the values the error carried; they are
  kept now, so the diagnostic names `ComputeValue(GraphValue(1))` against `ComputeValue(GraphValue(0))`;
- **the message now names the collision and, when it can, its two values.** With the values kept, the
  rendered sides were still two `?`s, which name nothing; the renderer now spells the class and — when
  neither value is a handle, whose `Debug` would print an address — the two values:
  `expected ?, found ? — two answers met on one node: ComputeValue(GraphValue(1)) against
  ComputeValue(GraphValue(0)) (docs/notes/answer-collision.md)`. Measured on a one-kernel graph program
  whose root is a `collect` of `graphrun` (CLI, no device): the message reads that sentence at the
  kernel's `compute.parallel` line. A handle-bearing value falls back to the class sentence alone, so
  nothing address-bearing is ever rendered (`docs/notes/raw-rendering-mark.md`).

The compute suite is unchanged by all three (203 passed / 15 failed / 2 ignored) — what changed is that
its failures explain themselves. Whether those thirteen programs *should* check is the compute side's
decision, not a lowlevel defect: the engine is reporting a real disagreement between two answers.

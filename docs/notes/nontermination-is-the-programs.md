# Non-termination is the program's own

> Status: current — the maintainer's ruling, and what was measured before it.
> Points at: `crates/lichen-highlevel/src/checker.rs` (the definition pass),
> `crates/lichen-lowlevel/src/apply.rs` (the budget counters).

**The maintainer's ruling.** An unbounded recursion is the program author's to avoid. The definition
pass does not promise to detect it, and no diagnostic is owed when it happens: `f = n => f n; f 3`
simply runs. The two tests that asserted a `NonTerminating` diagnostic for that shape are removed.

## What was measured before the ruling

The guard `checker.rs` runs after the definition pass reads `Module::budget_exhausted`, and that flag is
only ever set by two counters:

- `apply_total` (`apply.rs`), the cumulative application count — it stays **zero** for this program,
  because the recursion never reaches the VM's apply path: the time goes into the **definition pass**
  itself (`checker.rs`'s `evaluate_node_deep(ret)`, stuck on the first function);
- the nesting bound (`apply.rs`'s `depth_exhausted`), which reads the **applied node's own** `depth` —
  and for a self-application that node is the **shared template node**, whose `depth` stays `0`. Measured
  over 1500 deep-pass entries during a run: `own_depth=0` for 1468 of them, `own_depth=1` for 32.

So a depth bound cannot fire for a self-application, and the pass keeps minting nodes per reach: with a
"settled verdict is a hit" guard added to the deep pass, the hang became an allocation failure after about
five minutes instead of an unbounded wait. Both are the same fact — the work grows without bound and no
budget counts it — which is why the ruling is that the program owns this, rather than the compiler.

## What is still promised

`WorkBudget` still refuses a **terminating** expansion that outgrows its nesting bound
(`apply_depth_limit`, see `loop-conversion.md`, `heavy-value-cache.md` §8 and `code-audit.md`'s
`P1-40`), and the definition pass still reports the
`NonTerminating` diagnostic when that counter does trip. **There is no cumulative bound** — it was
removed by that decision, so only an expanded recursion is bounded and a converted loop is not. Only
the promise to catch a genuine infinite recursion is withdrawn.

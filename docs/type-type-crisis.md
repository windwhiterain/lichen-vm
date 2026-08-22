# The `Type : Type` paradox, constructed

*2026-08-22.  Probe: `crates/lichen-language/tests/type_type_crisis.rs`.
Soundness story: [`soundness.md`](soundness.md).*

This language implements `Type : Type` — the checker seeds a canonical
universe node `K = [Type, K]` whose type slot points at itself
(`Checker::install_constants`, `crates/lichen-highlevel/src/checker.rs`), so
the term `Type` has type `Type` and every type spine bottoms out at the
cycle.  The classic result is that such a universe is **inconsistent**
(Girard's paradox; Hurkens' 1995 simplification): the pure λ-calculus over it
contains a closed term of *every* type, and well-typed terms need not
terminate.  Here, where the runtime *is* the typechecker, that shows up as a
checker-certified program the checker's own definition pass cannot run to
completion.

## The construction

The term below is Geuvers' direct `λU` transcription of Hurkens' paradox
(*Inconsistency of classical logic in type theory*, §2 — the `Type : Type`
version), adapted to this checker's rules: unannotated lambdas (an unbound
parameter type defers the apply guard and kinding), the opaque `U = Type`
placeholder (the term never destructs `U`, so its λ-reduction is unaffected),
a call-result arrow domain (kinding defers on an unbound cell), and a lazy
`if 1 then 5 else …` around the one direct apply whose argument cannot
resolve while its parameter is unbound.  The final binding is a **closed
pure-λ term** — no `rec`, no constructors:

```
U = Type
D = (x => x) Type
sb = A => r => a => z => r (z A r) a
le = i => x => x (A => r => a => i (sb A r a))
induct = i => x => (le i x) -> (i x)
WF = z => if 1 then 5 else induct (z U le)
I = x => D -> Int
omega = i => y => y WF (x => y (sb U le x))
lemma = x => p => q => q I p (i => q (y => i (sb U le y)))
lemma2 = x => (x I lemma) (i => x (y => i (sb U le y)))
paradox = lemma2 omega
```

Observed:

- The checker **certifies `paradox : Int`** with no diagnostics — the
  annotated subterm `(paradox : Int)` checks cleanly when it sits in an
  unselected `if` branch: the annotation unify binds the call's result cell
  to `Int`, and nothing forces the call.
- Running it as the root (`paradox : Int`) makes the definition pass evaluate
  `lemma2 omega`; the paradox's self-referential structure recurses until a
  VM guard panics:

  ```
  PANIC: too many function applications (limit 2000) — non-terminating recursion?
  ```

So the typechecker — which runs the program to check it — **panics on a
program it certified**.  "Type-checks" no longer implies "has a value": that
is the exact worsening `Type : Type` buys, the loss of the normalization
guarantee.  It is a *non-totality* consequence, not a value-level unsoundness
— the checker remains sound on the values it actually produces (the
three-outcome argument in `soundness.md`).

## Reference

- A. J. Hurkens, *A simplification of Girard's paradox*, TLCA 1995.
- H. Geuvers, *Inconsistency of classical logic in type theory* (the `λU`
  transcription used here), §2: `www.cs.ru.nl/~herman/PUBS/newnote.pdf`.
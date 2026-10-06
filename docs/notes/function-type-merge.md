# A function's type is the function, and its signature is two cells

> Status: **landed, measured, and it did not fix what it was aimed at.** It
> supersedes
> [function-type-as-function](function-type-as-function.md) for the arrow half:
> a function type stops being a node of its own, and the arrow stops being a
> type. The `f : f` half of that note is untouched and still current.
> The wrapper defect in
> [compute-type-wrapper.md](compute-type-wrapper.md) **survives the merge
> unchanged**, so its cause is somewhere this change never reached — and it is
> now reproduced in two lines of pure language, with no JIT and no operator
> involved; see
> [the same defect in two lines of pure language](#the-same-defect-in-two-lines-of-pure-language-and-what-it-actually-takes).
> Base: `dev` at `dcf0cf3`; re-merged with `8f45cd9` (the operator seam returns
> `Option`) and `c21c88d` (`LowValue::Parameterized` deleted) before landing.
> Branch `feature/function-type-merge`.

## The two representations, and why they are the problem

A function's type is written today as `A -> B` and compiled to an **arrow
term** `[[dom, cod], [FunctionType, K]]`. A function's *own* type is a
different thing: the self-referential node `[Function(fid), ↺]`, whose
signature is not in it at all but in the template (`Function::parameter`'s
type slot, and `Function::return_type`). Unifying the two went through
`Program::unify_function_type` — a `Program` hook, so the lowlevel could ask
its host a type question — and that hook had to answer two questions at once:
*where is the signature* and *may it be written*. It answered them
differently for the two kinds of function, so a dynamic function's signature
bound and a frozen module's did not.

The cost was not the asymmetry itself. It was that the lowlevel, whose job is
positional unification, had to be told by a host what a function type is.

## The decision

**There is one representation: a function value, and the two cells it
already holds.** So:

- **`A -> B` compiles to a function, not to a type.** In a type position an
  arrow lowers to the term `_: A => _: B` — a real lambda whose parameter is
  annotated `A` and whose return is annotated `B`. The surface syntax does not
  change; only the lowering does. A function type is therefore a function, and
  a function's type is its own two cells — which is why `f : f` is a
  *consequence* and not a rule anybody maintains.
- **Two functions unify positionally, like two arrays.** One arm of the
  value match, recursing into `parameter` and the return side. No hook, no
  policy, no clone, and no place for a dynamic/static asymmetry to live.
- **The arm descends into the parameter's whole `[value, type, attrs…]`
  pair** — the position a signature occupies, not its type slot alone, so the
  attribute tail comes along. Unifying slot 0 as well is a claim about
  *values*, and the design here was narrower than the code: the tail-only
  version was tried, measured identical, and dropped as the strict subset it
  is. See
  [slot 0 was tried as an exclusion](#slot-0-was-tried-as-an-exclusion-and-made-no-difference).

`Function::parameter` stays the `[value, type, attrs…]` pair it is: its
*value* slot is the variable the body binds, and the apply clone walk
(`function_apply` / `node_apply`) clones it. So the arm names its two
positions rather than reusing the generic positional match — the shape is
array-like, the slot choice is function-specific.

## Why: attribute constraints on a signature, not only on a value

Today an arrow carries `dom` and `cod` and nothing else, so writing the same
cell in both positions gives *"input and output have the same **type**"* and
nothing more. `attributes.md` records the consequence: attribute flow through
a function is an explicit non-goal, because there is nowhere on an arrow to
hang an attribute.

After the merge, a function type's parameter and return are **terms**, so they
carry attributes, so a signature can constrain the values that flow through it:

```lichen
?a: Int => ?a: Int
```

The two `?a`s are one class, so unifying a function against this signature
makes the argument and the result the same cell — the constraint is *on the
values*, and it is stated in the *type*.

**This capability is argued, not measured.** No test in the workspace writes a
signature that constrains a value, so the honest claim is that the
representation now *can* carry the constraint; whether the checker lets a
source author spell it is the open question at the end of this note. Recording
that here rather than claiming the section's headline is verified: the
soundness argument below is the one that got tested, and it passed.

## The soundness question this forces, and how it is settled

Merging two functions merges their **parameter value cells**. For
`?a: Int => ?a: Int` against `x => x` that is the point. For two unrelated
functions of the same type — `f = x => x + 1`, `g = y => y + 1` — unify puts
`x` and `y` in one class.

A parameter cell is only ever read inside its own body, and both bodies are
checked, so *in principle* the merge leaks nothing. That is an argument, not a
measurement, and the `apply-clone-ownership` family is exactly the family where
"two things became one class and a read came out different" has bitten before
(`pipeline`'s `an_applied_struct_constructor_keeps_the_occurrence_identity`
and `a_raw_named_read_yields_the_field_type` are both parked red for it).

So the baseline is the judge, not the argument: the arm lands first, and the
fourteen parked reds are re-run before anything else. **They are all still
parked and none moved**, which is the evidence
[the soundness question](#the-soundness-question-this-forces-and-how-it-is-settled)
asked for and got: the `apply-clone-ownership` family did not move, so the
merge is sound as written.

## What the measurement said

Landed on `feature/function-type-merge` as `0c8cda9`, with every clause above
in force. The mechanical parts all hold. What the measurement refused was the
premise.

**The wrapper defect is untouched.** The frozen-module reproduction
(`docs/notes/compute-type-wrapper.md`) still prints

```
struct<.I raw[?a, ?b], .O raw[?c, ?d]>
```

before and after, and `examples/compute_jit.lichen` still types its `6` as
`raw[Int, raw[?a, ?b]]` rather than `Int`. So the two representations of a
function's type were never the cause. Whatever leaves `I` and `O` unbound
across the module boundary is downstream of the arrow, not the arrow itself, and
the earlier suspicion — `materialize_static_signature` reading a frozen
template's cells without binding them — was a guess that the merge did not
confirm.

**The cost shrank to zero distinct regressions.** The first landing broke nine
tests. Seven of them were expectations pinning the old representation, and each
was re-pinned with its reason in the commit that changed it: the two
`lichen-highlevel --test checker` cases that named the arrow's shape, the
render case that became `a_written_arrow_is_a_function`, the `pipeline` case
whose `5` is now typed `Int -> Int`, and `examples/gcd.lichen`, which
declared `raw 6: ?a` and now declares `6: Int`. `gcd` was `6: ?a` before this
change and is `6: Int` after, so the polymorphic rendering it used to
apologise for is gone.

**Two are still red, and they are one defect wearing two coats** — both are the
wrapper defect, reached without a JIT. They are parked with that reason rather
than re-pinned, because re-pinning `compute.jit`'s unapplied wrapper to render
as a collapsed signature would record the defect as intended behaviour. See
[what is open](#what-is-open).

## Three things the implementation had to say that the design above did not

**Both sides, or neither.** The arm fires only when *both* operands are
function types. When only one is, the other is a *degenerate* function type — a
`[Function(fid), t]` pair whose type slot names a **different** function rather
than itself — and the positional match is the right answer for it. That shape
is not invented here: `examples/closure.lichen` builds one, because a lambda whose
body returns a nested closure gives the outer function a return type that *is*
the inner function's type node, and the apply clone walk then pairs the two.
Reading any one-sided case as a conflict broke `closure.lichen` outright; the
design above assumed only the well-formed case exists, and that assumption was
wrong.

**The merge is the array's merge.** When the signatures agree the two
function-type nodes become one class, exactly as two arrays with equal elements
do. An earlier draft kept them apart on the argument that a self-cycle merged
with another hands readers whichever `Function` value the merge carried — which
is true, and was measured as *free* to remove: the red set over the workspace is
identical with the merge and without it, `closure.lichen` and `gcd` unchanged.
So the asymmetry bought nothing and cost the arm a second rule.

The hole it leaves is the sub-typing question, stated rather than patched: the
descent names the signature's two positions and never reads slot 0, so a merged
class of two function types keeps one carrier and answers with whichever
function the merge took. Whether two agreeing signatures are *the same type* or
one a subtype of the other is not decided here, and a class carrying two
identities is what answering that question positionally before asking it looks
like. A special case would not have answered it; it would have hidden it behind
an exception the next change has to unlearn.

**A function type and the universe never merge.** This is the one rule the
design above listed as *not decided* and the code had to decide, because
`(\x. x) : Type` is a program that must not check. A function type and the
universe are both self-cycles, so the obvious positional answer — descend,
unify the two cells, merge the classes — makes the universe a function.

The judgement is **a self-cycle test, not a name list**: when exactly one side
is a function type and the other side is a self-cycle that is *not* a function
type, the unification is refused. That covers the universe and also a
recursive struct type, without a second rule for each. A *degenerate* function
type is not a self-cycle — its type slot names a different function — so it
yields, which is exactly what `closure.lichen` needs. The test is the
invariant, not the roster; a roster would have to be extended by whoever
invents the next cyclic type, and would be wrong the day they forgot.

## Slot 0 was tried as an exclusion, and made no difference

The arm unifies the whole `[value, type, attrs…]` pair. The design's own wording
in [the decision](#the-decision) is narrower — the *tail* from the type slot on
— and slot 0 is the variable the body binds, the one the apply clone walk
clones per call, so unifying it merges two functions' **arguments** and is a
claim about values rather than about types. It looks like the polymorphism
collapse, and it is: unifying whole pairs is what makes `compute.jit`'s
parameter render as `raw[Function, <signature>]` instead of the open `?a` — a
wrapper that stopped being generic.

Tried: unify the tail only, slot 1 onwards, positionally. **Measured identical
on this workspace** — `compute::wrapper_functions_render_with_named_type_variables`
still prints `raw[Function, …]`, and nothing else moved. So the arm's own pair
unify is not what binds that cell; something else does, and it is not in this
change. The tail version is also strictly less than the pair version, which
already covers it. So the pair version shipped, and the open question is named
in [the pure-language reproduction](#the-same-defect-in-two-lines-of-pure-language-and-what-it-actually-takes)
rather than answered: **whatever binds a wrapper's parameter value cell lives
outside this arm.**

## The arm and the array arm are one shape and two codes

The decision above says the function arm is the array arm. That is true of the
*descent* and false of the *code*, and the difference is six things the array
arm does that the function arm does not:

| | array arm | function arm |
|---|---|---|
| arity | checked before descending | not checked |
| a self-cycle on either side | short-circuits, refuses | descends |
| `path` | shared with the descent | fresh |
| `steps` | pushed and popped around the descent | never pushed |
| early break | the moment a child disagrees | never |
| merges the class | only when *every* child agreed | whenever the recursion returns |

The function arm also decides by comparing `unify_errors.len()` across a fresh
`self.unify` — a second recursion with its own `AncestorPairs` — where the array
arm reuses the descent's own state. So the function arm is a **second
implementation of the same rule**, and it is the weaker one: it has no early
break, so it pays for the whole tree, and it has no shared `steps`, so a failure
deep in a signature records a diagnostic without the path that names where.

The obvious repair is to make them one loop: treat the signature as the
two-element sequence `[parameter, return_type]` and run the array arm's loop
over it, parameterising only the arity. That buys back the early break, the
shared path and the `steps` trail for free, and it deletes a rule rather than
adding one. **It is not done here** — it is a refactor of a hot path with two
red tests already attributed to this feature, and doing both at once would make
the reds unattributable. It is the first thing the next change should do.

## The same defect in two lines of pure language, and what it actually takes

The wrapper defect was long read as a compute problem, on the reasoning that
`compute.jit` is a JIT wrapper and the JIT is where the cells go missing. That
reasoning is wrong. Two files, no operator, no `.native`, no struct:

`b.lichen` — a **frozen** module, because it is imported:

```lichen
wrap = f => {I = _; f: I -> _; I}
```

`main.lichen`:

```lichen
---
w = import "b.lichen"
---
w.wrap
```

On this branch `w.wrap` types as

```
Function: raw[Function, raw[?a, ?b] -> raw[?c, ?d]] -> ?b
```

and on `dev` at `dcf0cf3` the same program types as

```
Function: raw[?a, raw[?b, ?c]] -> raw[?d, ?e]] -> ?b
```

— generic, which is what a wrapper is supposed to be. A written arrow in a
frozen module's signature collapses the parameter's cell onto the `Function`
marker and freezes the signature into it. That is the whole of
`compute.jit`'s symptom, and it is the same defect that leaves `compute_jit`'s
`6` typed `raw[Int, raw[?a, ?b]]`.

Each candidate ingredient was removed in turn, on this branch, and the collapse
tracks exactly one of them:

| variant | type | collapses |
|---|---|---|
| `wrap = f => f` | `?a -> ?a` | no |
| arrow annotation `f: I -> _`, no struct, no native operator | `raw[Function, <sig>] -> ?b` | **yes** |
| the same, but the module is **local** | `raw[?a, ?b] -> …` | no |
| a struct instantiation, but **no** arrow annotation | `?a -> struct<…>` | no |
| arrow + a two-field struct | `raw[Function, …>` | yes |
| arrow + a three-field struct carrying `.native` | `raw[Function, …>` | yes |

**A struct is not required. A native operator is not required. A JIT is not
required.** The necessary and sufficient ingredient is **a written arrow
annotation in a frozen module**. The compute wrapper is that, plus a struct and
an operator, which is why it looked like a compute problem.

**What is still unknown is the mechanism.** Nothing in this change binds that
cell, and [the slot-0 experiment](#slot-0-was-tried-as-an-exclusion-and-made-no-difference)
ruled out the function arm's own pair unify as the cause. So the next person on
this has a two-line reproduction and a narrowed search space, and that is a
better position than the one this note started from.

## What goes

- `Program::unify_function_type`, `FunctionTypeUnify` and its three answers,
  and the hook call in `equality.rs`.
- `Module::is_function_type_node` and the self-referential node it recognised.
- `shape`'s `is_arrow_type_any`, `is_function_type_node_any`,
  `is_function_type_any`, `is_function_type`, `function_type_parts_any`,
  `function_type_parts`, `unify_function_type`, `signature_pair`,
  `signature_cells`, `clone_signature_dynamic`.
- `Module::clone_signature` and `Program::materialize_static_signature`; the
  latter is rewritten, not deleted, and now returns the parameter pair and the
  return type.
- `low_type_of`'s function arm, which decoded the arrow's shape.
- The clone-on-unify that `function-type-as-function` introduced: the
  `gcd` doc paragraph that described it as a rule is rewritten, because the
  rule is gone by construction now.

## What stays behind, dead

The arrow **rendering** machinery is not removed and is now unreachable: the
`TypeFunction` marker and its registry entry, the `arrows` set, `Ctx::arrow`,
`arrow_parts`, and the kind marker. Nothing builds `[FunctionType, Type]` any
more, so these are kept only so the diff stays reviewable. They are the obvious
next deletion, and they are safe because the marker is what
`compute-runtime-scalars.md` §5 quotes in a historical error message — removing
it changes that quote, so it wants its own commit and its own note edit.

## What is open

- **What binds a wrapper's parameter value cell.** The two-line reproduction
  above, minus the function arm. This is the blocker for the compute wrapper
  work, and it is not in this change.
- **Whether the function arm should be the array arm.** See
  [one shape and two codes](#the-arm-and-the-array-arm-are-one-shape-and-two-codes).
  The repair is known and is a deletion, not a design question.
- **Two reds, parked.**
  `compute::wrapper_functions_render_with_named_type_variables` — the *unapplied*
  `compute.jit` no longer renders as a generic wrapper, because its parameter
  collapsed to `raw[Function, <signature>]`. The test's own comment says "the
  wrapper itself stays generic", so re-pinning it would assert the defect.
  `compute::a_float_domain_is_permitted_at_every_position_the_walk_reaches` —
  a domain containing `Int -> Float` is now refused earlier than the test
  expects, and the message it produces says "annotate the parameter" about a
  parameter that is already annotated, which is a second bug in its own right.
  Both un-park by deleting one `#[ignore]` each once the mechanism above is
  found; neither should be re-pinned before then.
- **`?a: Int => ?a: Int` as source syntax.** Whether the type language spells a
  signature with attributes, and how a written `?a` in the two positions is
  made one cell, is the next question the merge opens — and the reason the
  merge is worth doing first.
- **The sub-typing question** in
  [the merge is the array's merge](#three-things-the-implementation-had-to-say-that-the-design-above-did-not):
  whether two agreeing signatures are the same type. Deliberately left to the
  next change rather than answered with a special case.

## Order of work, as it went

1. lowlevel: the `Function` arm; delete the hook, the enum, the call site and
   `is_function_type_node`. — `0c8cda9`
2. Measure before anything else. The special case that kept two function types
   apart is *free to remove*: the red set is byte-identical either way. — `7b4afee`
3. highlevel: delete the recognizers and the policy; `lambda.rs`'s
   function-ness gate recurses into the two cells directly; a type-position `->`
   compiles a lambda. — `0c8cda9`
4. Re-pin the five expectations that named the old representation, each with its
   reason. — `b8d7daf`, `3220fb0`, `7819381`
5. Merge `dev`, then fix the single-sided self-cycle hole the merge exposed
   (`(\x. x) : Type` must fail). — `cf21c76`, `b8d7daf`
6. Merge `dev` twice more, under `c21c88d` which deleted
   `LowValue::Parameterized`.  The two changes touched the same four files for
   the same reason — each was one way of writing *this node is not decided
   yet* — so the conflicts were import lists and one test that asserts the
   opposite thing about the same cell.  That test is the interesting one: `dev`
   says a frozen template's parameter type **must not be guessed** and this
   branch says the annotation **does** reach it.  Both cannot hold.  This
   branch's claim is the one kept, because the note's whole argument is that
   a signature is a pair the unifier descends into, and a template whose
   parameter type nothing can reach is the failure mode `materialize_static_
   signature` exists to fix.
6. **Verify, in this order — the first is the whole point, and it did not
   happen:**
   - the frozen-module reproduction goes from
     `struct<.I raw[?a, ?b], .O raw[?c, ?d]>` to `struct<.I Type, .O Type>` —
     **it did not; it printed the same before and after.** The reduced form of
     the same question is
     [two lines of pure language](#the-same-defect-in-two-lines-of-pure-language-and-what-it-actually-takes),
     which is where the next attempt should start.
   - `examples/compute_jit.lichen`: `raw 6: raw[Int, raw[?a, ?b]]` → `6: Int`,
     then drop it from `tests/examples.rs`'s `WORK_IN_PROGRESS` — **still
     `raw[Int, raw[?a, ?b]]`; the name stays in the list.**
   - the fourteen parked reds, against the same list — **none moved.**
   - `gcd` stays `6: Int` — **it is `6: Int`; it was `6: ?a` before this
     change.**

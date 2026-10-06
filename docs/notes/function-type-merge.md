# A function's type is the function, and its signature is two cells

> Status: **landed, measured, and it does not fix what it was aimed at.** This
> supersedes
> [function-type-as-function](function-type-as-function.md): a function type
> stops being a node of its own, and the arrow stops being a type. See
> [What the measurement said](#what-the-measurement-said) — the wrapper defect
> in [compute-type-wrapper.md](compute-type-wrapper.md) survives the merge
> unchanged, so its cause is somewhere this change never reached.
> Base: `dev` at `6525147`.

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
- **The arm unifies the parameter's whole `[type, attrs…]` tail**, not only
  its type slot. That is what makes the next section work.

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
values*, and it is stated in the *type*. This is the capability the merge buys
and the reason it is worth more than fixing the static branch.

For it not to be vacuous, the arm must unify the parameter's attribute tail as
well as its type slot. That is the whole reason the arm is written the way it
is.

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
twelve parked reds are re-run before anything else. If that family does not
move, the merge is sound as written. If it moves, the merge is wrong and the
arm needs a limit — for instance merging the parameter's value cells only when
one side is a type-position signature.

## What the measurement said

Landed on `feature/function-type-merge` as `0c8cda9`, with every clause above
in force. The capability the merge was for is real — the parameter pair is what
a signature descends into, so `?a: Int => ?a: Int` is now expressible over
values — and the mechanical parts all hold. What the measurement refused was
the premise.

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
confirm. `examples/gcd.lichen` still reads `6: Int`, so nothing that used to
work regressed.

**Nine tests that were green are not**, and they are the honest cost of the
merge rather than of a mistake in it: the three `lichen-highlevel --test
checker` tests that pin the arrow's shape (`partial_inference_in_an_arrow_type`
also pins clone-on-unify, which this change removes by construction),
`a_compound_type_value_renders_in_type_syntax`,
`a_float_domain_is_permitted_at_the_every_position_the_walk_reaches`,
`wrapper_functions_render_with_named_type_variables`, the `examples` suite, and
two more. None triaged. The twelve parked reds are all still parked, which is
the soundness evidence the [soundness question](#the-soundness-question-this-forces-and-how-it-is-settled)
asked for and got: the `apply-clone-ownership` family did not move.

## Two things the implementation had to say that the design above did not

**Both sides, or neither.** The arm fires only when *both* operands are
function types. When only one is, the other is a *degenerate* function type — a
`[Function(fid), t]` pair whose type slot names a **different** function rather
than itself — and the positional match is the right answer for it. That shape is
not invented here: `examples/closure.lichen` builds one, because a lambda whose
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

## What goes

- `Program::unify_function_type`, `FunctionTypeUnify` and its three answers,
  and the hook call in `equality.rs`.
- `Module::is_function_type_node` and the self-referential node it recognised.
- `shape`'s `is_arrow_type_any`, `is_function_type_node_any`,
  `is_function_type_any`, `is_function_type`, `function_type_parts_any`,
  `function_type_parts`, `unify_function_type`, `signature_pair`,
  `signature_cells`.
- `Ctx::arrow` / `arrow_parts` and the trait method — not rewritten, **not
  needed**: "make an arrow type" is not a thing any more, because what a
  type-position arrow makes is a function, and `check_lam` already does that.
- The `TypeFunction` kind marker, with its registry entry, `ValueType`
  method and codec tag.
- The renderer's two function arms collapse into one that reads the two cells.
- `low_type_of`'s function arm, which decoded the arrow's shape.

## Order of work

1. lowlevel: the `Function` arm; delete the hook, the enum, the call site and
   `is_function_type_node`.
2. highlevel: delete the recognizers and the policy; `lambda.rs`'s
   function-ness gate recurses into the two cells directly.
3. checker/frontend: a type-position `->` compiles a lambda.
4. renderer: one arm.
5. `low_type_of`: the function arm follows.
6. Verify, in this order — the first is the whole point:
   - the frozen-module reproduction: a wrapper in an **imported** module
     storing a type variable in a struct field goes from
     `struct<.I raw[?a, ?b], .O raw[?c, ?d]>` to `struct<.I Type, .O Type>`;
   - `examples/compute_jit.lichen`: `raw 6: raw[Int, raw[?a, ?b]]` → `6: Int`,
     then drop it from `tests/examples.rs`'s `WORK_IN_PROGRESS`;
   - the twelve parked reds, against the same list;
   - `gcd` stays `6: Int` and `partial_inference_in_an_arrow_type` keeps
     pinning the direct-unify rule.

## Not in this change

- Whether the two roots merge. Keeping them distinct is a deliberate
  asymmetry too: a function value and its own type must stay tellable apart,
  and the universe's `[Type, ↺]` is distinguished from a function by the same
  self-cycle. Not decided here, and the design does not need it decided —
  the arm recurses structurally, so the roots are only a question for readers.
- `?a: Int => ?a: Int` as *source syntax*. Whether the type language spells a
  signature with attributes, and how a written `?a` in the two positions is
  made one cell, is the next question the merge opens — and the reason the
  merge is worth doing first.

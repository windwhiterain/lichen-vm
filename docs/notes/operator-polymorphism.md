# Operator polymorphism: a refinement contract over a lichen dispatch

> Status: **proposed** — nothing here is implemented. The architecture was
> settled in discussion before any code: **an operator's contract and its
> implementation are separate artifacts, and the contract may be stricter than
> the implementation** (§2). The contract is a **refinement** (§3): one
> predicate function on the operand's *value* that must evaluate to `1`, carried
> in an ordinary **attribute** and enforced by the ordinary **assert** channel.
> The implementation is an ordinary lichen function that selects a per-class
> leaf and applies it (§4). The panic arm is dead by construction (§5).
>
> **Rejected on the way here: the class-set *type*.** A tenth kind marker whose
> members are type values, with a narrowing rule in `unify_inner`, cannot work,
> and each half of why is measured:
>
> - A set in *type* position has to commit a member at every use, and the unify
>   that does it is the **lowlevel's** (`apply_parameter_check`), so the rule
>   must be a `Program` hook consulted from `unify_inner` — which, being a
>   `Merge`, folds a per-occurrence domain node into the *canonical* class of
>   the member. `class_committed_value` then scans past the set, the deferral
>   path reconciles against it, and the domain node's own slot can be written
>   over. Every kernel test that reads a domain goes wrong, in ways that are
>   order-dependent and so differ between a fresh and an incremental build.
> - More fundamentally, a set that *is* a type makes **unification** the
>   enforcement mechanism. The contract is a fact about **values** — "this
>   operand's value is one of the numeric classes" — and a fact about a value is
>   checked by *evaluating* it, not by reconciling types. That is a refinement.
>
> A refinement needs no narrowing and no new type: the operand's cell stays an
> ordinary unbound `?a`, narrowed by the plain let-polymorphic cloning that
> already makes `x => x` polymorphic.
>
> Points at: `crates/lichen-highlevel/src/checker/operators.rs` (`check_binop`,
> the pin this removes), `crates/lichen-highlevel/src/attr.rs` (`AttrExt`, the
> carrier §3 takes `Perspective`'s shape from), `crates/lichen-highlevel/src/
> checker/asserts.rs` (`register_assert`, the enforcement `assert.rs` already
> provides), `crates/lichen-lowlevel/src/assert.rs` (`check_asserts`, the
> force-and-require-`USize(1)` discipline), `crates/lichen-highlevel/src/
> checker/indexing.rs` (`check_index`, whose bounds constraint is already a
> refinement in all but name), `crates/lichen-language/src/compile.rs` (the
> homogeneous branch-array desugar §4 replaces), `crates/lichen-compute` (the
> defaulting point), and `lichen-std/_.lichen` (the operators' end-state home).
>
> Companions: [operators](operators.md) (the operator set this edits),
> [floating-point](floating-point.md) §4.2 (the no-conversion rule this must
> not weaken), [type-of-in-std](type-of-in-std.md) (the precedent for moving a
> language form into the library),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferred
> unification the dispatch relies on).

## 1. What is missing

`check_binop` decides the class of an arithmetic operation once, at check
time: a concretely `Float` operand selects `Float`, and **every other operand —
an `Int`, an undecided parameter — pins the whole operation to `Int`**. The
pin is what kills polymorphism:

```lichen
add = x => y => x + y   -- today: Int -> Int -> Int
add 1.5 2.5             -- today: expected Int, found Float
```

The language's let-polymorphism is structural — every application clones the
parameter cells fresh — so `x => x` is polymorphic precisely because nothing
ever binds its type cell. The operator pin binds it early. The goal: `+ - * /`
are polymorphic over the two scalar classes in a type-system-standard way,
**every instantiated use is class-concrete**, and a wrong-class use is a check
error, not a runtime accident.

## 2. The separation principle

The design's load-bearing idea, stated plainly because everything else follows
from it:

> **An operator's contract and its implementation are separate artifacts, and
> the contract may be stricter than the implementation.**

- The **contract** is a refinement: `+ : {a | a ∈ {Int, Float}} → …`. It is what
  rejects `add "a" "b"`, and it leaves the operand's *type* open, which is what
  makes the operator polymorphic at all.
- The **implementation** is a dispatch: an ordinary lichen function that asks
  the value's class which machine leaf to run. It is written for *every* class,
  including the ones the contract excludes — its last arm is a panic the
  contract proves unreachable.

This is the standard shape of a bounded-polymorphic builtin: Haskell's
`Num a => a -> a -> a` is a contract over a dictionary the class system
supplies; here the contract is a refinement whose predicate consults a **domain
value** (§3) and the dictionary is the operand's own type value read by the
dispatch (§4). The separation is what lets each half be simple: the contract
never sees the dispatch, the dispatch never restates the contract, and the
"impossible" arm needs no clever type — it needs a proof it is never taken,
which the contract *is*.

Two measured facts bound what the implementation can look like:

- **The runtime dispatch already exists.** `TypeOperator::run` picks integer
  or IEEE arithmetic from the operand *values*. A selection over
  `type_of x` is the same dispatch written in the language, one level up —
  which is the point: the semantics of `+` becomes library code, the same
  trajectory `type_of` took ([type-of-in-std](type-of-in-std.md)).
- **A homogeneous container cannot hold the dispatch.** `if` desugars to a
  branch array whose element type is one cell (`compile.rs`: "the branch
  array is homogeneous"), and a table literal unifies every entry's value
  into one cell (`check_table_term`). In `if t == Int then $iadd x y else
  $fadd x y` the two branches' results are an `Int` and a `Float`, and they
  meet in that cell. §4 removes the cell rather than loosening it.

## 3. The contract: a refinement on the operand's value

The constraint is not a type. It is a **refinement** — classical
`{v | p v}`, with the base type left as the ordinary inference cell and the
predicate `p` required to evaluate to `1`.

The attribute's slot holds **exactly one function**, an ordinary lichen value.
The raw lowering of an annotated expression is the pair's third element:

```text
Int{x > 3}   ≡   [Int, Type, x => x > 3]
                 └────┬────────┘ └──┬──┘
              the pair's value,     the refinement slot
              type, and tail
```

One function is enough because a conjunction is one function written with `*`
(`0 * 1 = 0`, so `*` is conjunction over this language's `0`/`1` scalars):
`Int{x > 3}{x < 10}` is `x => (x > 3) * (x < 10)`. "No refinement" is the
attribute's **missing value**, one concrete shared constant (`Void`), so an
unrefined expression costs a shared node and nothing else.

It is carried as an **attribute** (`AttrExt`), which is the one mechanism here
that already carries a non-equality constraint on an expression across every
journey the graph takes. `Perspective` is the proof it works end to end, and the
refinement is its shape with the propagation removed:

| | [`Perspective`](../../crates/lichen-perspective/src/perspective.rs) | refinement |
|---|---|---|
| slot holds | a lattice value (a thread count) | **one predicate function** |
| `combine` | `Gcd` — the meet, propagated from the children | **none: no propagation** |
| `missing_value` | `0`, the `gcd` identity | **`Void`**, "no refinement" |
| reconcile | `check_unify_relaxed` + `is_subtype` | **a plain unify** |

**Why nothing propagates, and this is the load-bearing negative.**  The tempting
reading is Perspective's: derive an expression's refinement from its children's,
so that `x + y` carries `p x * q y` and a subtree accumulates one condition at
its root.  It does not hold here.  The frontend **folds** computations — an
`a == b` on known operands is folded away, a `let` is substituted, arithmetic on
constants is evaluated — so by the time a parent is built, the children that
produced its value may no longer exist as expressions at all.  A refinement that
survived that would have to be *inferred* rather than propagated, and inferring
a predicate for a computed value is a solver's job (the value is `3`; which
predicates does it satisfy?).  So propagation is dropped, and with it every
solver question: **a refinement is only ever where it was written**, and an
expression with no written refinement has none.

**Enforcement is one lowlevel `assert` per refined expression**, inserted by the
highlevel as it lowers that expression.  The condition is the refinement applied
to the expression's own value — `f value`, an ordinary lichen apply — and the
insertion is [`register_assert`](../../crates/lichen-highlevel/src/checker/asserts.rs) as it stands, so the
whole of the enforcement is already written and already has the right semantics:

- `check_asserts` force-evaluates the condition *ignoring laziness* and requires
  `USize(1)`; a condition that stays lazy is **pending**, not failed, so a
  refinement on an open parameter waits for a value.
- The entry is registered on the enclosing function, so the **apply clone
  re-checks the instantiated condition per call** — the refinement follows the
  argument without the checker.
- `AssertError`'s `{condition, template, value}` carries the provenance a
  diagnostic needs, and the condition node's runtime edge carries the span.
- It freezes with the function, so a persisted callee's refinement survives
  without the checker.

This is why the lowlevel needs **no new mechanism**, and why the earlier
"refinement record plus a `Program` hook" is not in this plan: the association
the lowlevel was missing ("which value does this constraint refine?") *is* the
condition node, because the highlevel has already applied the predicate to the
value and can attribute the span.  `check_index`'s bounds constraint
(`i < length`, registered on the parameter and re-checked per call) is already a
refinement in all but name; this generalises that path instead of opening a
parallel one.

**Reconciliation is a plain unify.**  Requiring the *same* refinement rather than
a weaker one is over-strict, and deliberately so: weakening needs implication
between predicates (`fact ⊨ requirement`), which is the subtyping this language
does not have.  A unify never wrongly *accepts*, so the strictness costs
expressiveness and buys soundness.

**Nothing unifies against a refinement.**  It is not in a type slot, no rule is
added to `unify_inner`, and the type cell stays open — that is the polymorphism.
The lowlevel apply does unify the parameter pair positionally, so the
parameter's live refinement slot must stay *unbound*, the same trick `check_lam`
already plays for a perspective: binding it would let the deep pass bake it and
make the apply enforce the declared refinement by equality.

The contract has **two halves**, and §2's separation principle is what keeps
them apart:

The contract that an *operator* carries has **two halves**, and §2's separation
principle is what keeps them apart:

- The **check** is the refinement: one predicate function in the attribute slot,
  enforced by the inserted assert. This is §3's subject.
- The **domain** is a *value*: a **class set** — a tagged list of member type
  values, `[TypeSet, [members]]`. Kept, but demoted out of the type system: the
  tag is a plain value constant (spelled beside `TypeId`, which is likewise
  **not** a kind marker), and the encoding is deliberately *not* a kinded type
  expression `[shape, [marker, K]]`. That is what makes "not a type" a property
  of the representation rather than a convention: `low_type_of` reads slot 0 as
  a shape, finds a tag that is no marker, then reads slot 1 as a kind, finds a
  one-element list where a `[marker, K]` belongs, and answers `Unknown`. It is
  what a reader that needs the **candidates** consumes — the kernel, which must
  answer an open domain with a class, and the diagnostics.

The domain is *data*, and the predicate is the *function that consults it*: the
membership test reads the set. Nothing unifies against either, `+ : ?a -> ?a ->
?a` stays exactly that, and no rule is added to `unify_inner`.

```lichen
-- lichen-std, end state
Num  = {Int, Float}                     -- the domain VALUE (a class set)
in_num = v => type_of v ∈ Num           -- one predicate, consulting the domain
is_int = v => type_of v == Int
iadd = …                                -- the machine leaves, today's TypeOperator
fadd = …                                --   split per class (see §6)
add  = x => y => { x : ?a{in_num}; y : ?a{in_num}      -- the contract
                   (fadd, iadd)(is_int (type_of x)) x y }
```

`x : ?a{in_num}` puts the predicate in the parameter's attribute slot and leaves
its *type* cell open — that is the polymorphism. `add`'s printed type is
`?a -> ?a -> ?a` with the refinement shown beside it (spelling in §8.1).

## 4. The implementation: a dependent if that already exists

**Measured, not proposed: the dependent if needs no new checking rule.**  The
positional slot read `a(k)` already types as

```text
ty = Index(Index(type_of a, 0), k)          -- `slot_read`, checker/structs.rs
```

so indexing a **tuple** with a dynamic key *is* `if c then type_of b else
type_of a`:

```lichen
(1, "1")(x)      -- Index([Int, string], x)
```

The first draft of this section proposed a rule for the same type slot while
`if` kept desugaring to an array.  The rule is unnecessary; the desugar is what
has to change, and the two load-bearing consequences fall out of encodings that
already exist:

- **Branch types may differ.**  `check_tuple_term` gives every element its own
  type slot, unlike the array literal today's `if` desugars to
  (`check_array_term` unifies every element into one cell).  So the fix is to
  desugar `if c then t else e` to the *tuple* read `(e, t)(c)` instead of the
  array read `[e, t][c]` — one line in `crates/lichen-language/src/compile.rs`.
- **The type slot is the lazy `Index`.**  Already what `slot_read` builds; the
  `Index` resolves per instantiation, when `c` is concrete.
- **An untaken arm's constraints do not fire — because the arm is an apply.**
  `check_app` performs **no** argument/parameter unify: it builds the `Apply`
  node and leaves the unify to the lowlevel `apply_parameter_check`, which runs
  per call site on the parameter clone.  A function body is a template and is
  never evaluated at definition, so an apply sitting in an unselected branch is
  never forced.  Both spellings therefore defer:

  ```lichen
  (fadd x y, iadd x y)(cond)      -- the `if` desugar
  (fadd, iadd)(cond) x y          -- select the leaf, then apply
  ```

  This removes the *branch-pending* deferral cause the first draft proposed
  (and with it the deferral-budget question): nothing new is needed, because the
  one unify that used to fire eagerly was the **array literal's** shared element
  cell, and the tuple desugar abandons that cell rather than loosening it.  The
  claimed laziness is the first thing Phase 2 measures.

And the exhaustiveness arm: `panic` (the language's recorded-refusal channel,
the same one `operator.divide_by_zero` uses) has a free type cell that unifies
with anything, so a chain always has a last arm. Under the §3 contract that arm
is dead — see §5.

What the dependent reading does **not** do: it does not constrain `?a` to
numerics.  `(1, "1")(c)` on a `string` operand still *checks* — the contract's
job is §3's, and this is the separation principle again: the index rule makes
the dispatch expressible, the refinement makes it safe.

## 5. The worked example

```lichen
add = x => y => { x : ?a{in_num}; y : ?a{in_num}
                  (fadd, iadd)(is_int (type_of x)) x y }
```

Measured on `dev` before any of this, and again with the first §6 change landed
(the operand tie instead of the pin), taken with `lichen-compiler`:

| program | today | after the tie |
|---|---|---|
| `add 1 2` | `3: Int` | `3: Int` |
| `add 1.5 2.5` | **fails**: `expected Int, found Float` at the `1.5` | **`4.0: Float`** |
| `add 1 1.5` | fails: `expected Int, found Float` at the `1.5` | **identical**, same text and position |
| `add "a" "b"` | fails: `expected Int, found string` | **`parameterized: string`** — not refused |
| `(add, add 1.5 2.5)` | — | `<?a -> ?a -> ?a, Float>` |

Two mechanism facts fell out of that measurement, and both are load-bearing:

- **The apply clone preserves equality classes.**  Tying the two operands into
  one class *in the template* therefore makes the arguments of a single
  application share one class, which is why `add 1 1.5` is still refused with
  today's exact message: the first argument commits the shared class and the
  second meets it.  So §8.4 is not a problem — "the operands are one class" is
  free, not a condition that has to be built and asserted.
- **An unknown class is not an error, it is an undecided value.**
  `TypeOperator::run` answers `Parameterized` for a pair it cannot compute, so
  `add "a" "b"` *runs* and yields an unresolved value instead of being refused.
  That is the hole the refinement closes, and it says what the refinement has to
  be: a condition that turns "undecided" into "refused", registered as an
  assert on the operand.

- **Definition.** `x : ?a{in_num}` and `y : ?a{in_num}` put the predicate in the
  parameters' attribute slots; the *type* cells stay open, so the signature is
  `?a -> ?a` until a leaf's contract closes it (a leaf typed `?a -> ?a -> ?a`
  leaves it as it is). Each parameter's refinement is applied to that parameter's
  value and registered as an assert, which stays pending while the cell is open.
- **`add 1 2`.** Apply clones; `1` binds the clone's type to `Int`;
  `is_int (type_of x)` is `1`, so the `Index` takes `iadd`; `iadd 1 2` is
  `2: Int`, and the refinement `in_num` holds.
- **`add 1.5 2.5`.** The clone binds `Float`, the `Index` takes `fadd`, and the
  result is `3.0: Float`. **This is the feature**: today the pin refuses it.
- **`add "a" "b"`.** The refinement's condition resolves to `0` — a refusal
  naming the domain (`does not satisfy {Int, Float}`), which is §2's "the type is
  stricter than the implementation" made concrete. The dispatch never matters.
- **`add 1 1.5`.** The one case the refinement does **not** reproduce on its own,
  and it is worth stating because it is the price of dropping the set's type
  role. Today's refusal comes from the *pin*: `x + y` puts both cells in the
  canonical `Int` class, whose committed value every clone replicates — so the
  `y` clone starts at `Int` and `1.5` conflicts at the argument. A refinement
  binds nothing, and each application instantiates the parameter cell
  independently — that *is* let-polymorphism — so both clones satisfy `in_num`.
  Under the end state the refusal is the **selected leaf's own contract**:
  `iadd`'s parameter is `Int`, so `iadd 1 1.5` is refused at `iadd`. That is an
  apply-time (runtime-channel) refusal rather than a check error at the argument,
  and §8.4 records the choice.
- **`compute.jit (y => y + y)`.** The parameter's type cell is open, so the
  domain is undecided — but the refinement names the **set**, whose candidates
  are data. The kernel reads them and takes the default class, `Int`. Two things
  make that sound rather than merely test-preserving, and both are load-bearing:

  - The compiled class must be **committed into the signature**, not just used
    for lowering: `.sig` is `type_of f`, and `launch` gates the argument against
    the signature's domain lazily. A signature left open would admit a `Float`
    argument to an `Int` kernel — the default would be a guess the launch could
    not defend. Committed, the launch refuses it, which is §5's `add 1 1.5`
    refusal arriving through the other door.
  - Therefore `compute.jit` must write the defaulted class into the parameter
    cell (or build `.sig` from the classes it actually compiled), which is the
    "kernel default" this phase owes — and it is why the existing expectations
    (`.sig Int -> Int`) are the *correct* answer here rather than a spelling
    that happens to match.

  With the class concrete the `Index` selects one leaf and it lowers to
  `KernelBin::Add` — byte-identical kernels, no new optimizer.

## 6. The leaves

The dispatch selects a per-class machine **leaf** and applies it. Today's
`TypeOperator::Add` is *already* class-polymorphic at run time (it reads the
operand values), so two shapes work:

- **Keep the unified leaf**: `iadd` and `fadd` are both the existing operator;
  the selection is then the *specification* of the dispatch (and the place a
  future class plugs in), while `run` keeps doing what it does. Zero lowlevel
  change.
- **Split the leaf per class**: each leaf names its machine op, the `run` arm for
  a wrong-class leaf records a refusal, and the dispatch is load-bearing all the
  way down. Cleaner failure isolation, one more `TypeOperator` variant per
  operator per class (codec tags append-only, as ever).

Either satisfies the design; **the split is the honest end state** because it
gives each leaf a *concrete* parameter type (`Int -> Int -> Int`,
`Float -> Float -> Float`). That is what replaces the pin as the thing that
refuses a cross-class use — §5's `add 1 1.5` — and it is what makes the selected
leaf's contract enforceable by the ordinary apply, with no new rule.

The **`Int`-only** operators (`%`, the bitwise trio) are not polymorphic at all:
their domain is a single class, so they keep today's pin
(`unify(operand, int_type)`). A one-member domain is a pin, not a refinement —
no uniform mechanism is bought by pretending otherwise, and pinning is what makes
their *result* a machine scalar rather than an open cell. `==`/`!=` stay
unconstrained (the generalized equality).

## 7. Routing a surface operator to the function

How `a + b` reaches the std function, unchanged from the earlier analysis and
still phased: **R1** a prelude desugar (principled end state; needs the
language's first implicit import), **R2** an intrinsic registry (the checker
resolves `ir::BinOp` to the registered function value and checks an ordinary
apply; the emitter recognises "apply of an intrinsic whose dispatch selects one
leaf" so kernels see `KernelBin` as today), **R3** the checker's special case
stays but *reads the contract from the std binding* instead of restating
`{Int, Float}` in Rust (waypoint). The refinement and the domain value are
routing-agnostic: phases 0–2 below land the semantics under R3, and R2/R1 are the
"operators are library functions" end state whenever the prelude question is
answered.

## 8. Open questions

1. **The printer's spelling** of a refined cell. The raw form is
   `[Int, Type, x => x > 3]`, so `Int{x > 3}` is the literal reading, and
   `AttrExt::render` is where it lands. For a *contract* the readable spelling is
   the named predicate, not the lambda's text.
2. **`Num`'s home**: std binding (the `type_of` precedent) vs keyword.
3. **The panic arm's spelling**: the recorded-refusal channel needs a
   value-level form a library function can write; today only builtins record.
   A `$panic` leaf with a free type cell is the minimal answer.
4. **Which channel refuses `add 1 1.5`.** §5 shows the refinement does not, by
   itself. The end state's answer — the selected leaf's concrete parameter type,
   refused by the ordinary apply — is an *apply-time* refusal
   (`DiagKind::Runtime`), where today it is a check error at the argument. That
   is the same channel `f = x => x + 1; f Type` already uses, so it is the
   language's existing answer to a per-call mismatch, but it is a visible change
   of diagnostic kind and position.
5. **The refinement's diagnostic flavour.** A refinement's assert must not read
   as `assert failed`; it is a contract failure and should name the domain. The
   lowlevel's `AssertError` carries `{condition, template, value}` and the
   checker already keeps `user_asserts` to tell an explicit `assert` from a
   generated guard — so a refinement is a third flavour on that same
   discrimination, and the wording lands in the language layer.
6. **The kernel's committed default.** §5 makes it load-bearing that
   `compute.jit` writes the defaulted class into the parameter cell (or builds
   `.sig` from what it compiled).  **Measured: it is not cosmetic and not only
   about lowering.**  With the domain left open, three test targets go red, and
   two of them are the signature being *unreadable* rather than misprinted:

   - `a_kernel_value_and_type_render_by_name` — `.sig Int -> Int` renders as
     `.sig ?c -> ?c`, and the artifact itself comes out `none`.
   - `runtime_only_package` — `launch`'s signature gate reads the domain
     *lazily* out of `.sig` (`Index(sig.ty, 0)`); an open domain leaves that read
     unresolvable, so the gate refuses an `Int` argument with `expected ?a, found
     Int`.  A domain that is decided unblocks the gate as well as typing it.
   - `examples` — `math.lichen`'s and `geometry.lichen`'s declared signatures
     change from `Int -> Int` to `?a -> ?a`: **the feature showing up in public
     example output**, which the repo's own harness says to update in the same
     commit when intended.

   So the default must be **authorised**, not blanket: `jit (y => y)` has an open
   domain and no candidates, and must keep refusing (today's `UNDECIDED_DOMAIN`,
   "annotate it so its domain is known").  What authorises it is the domain the
   refinement declares — which is the next thing to build, and the reason the
   refinement and this default are one step rather than two.

(Closed since the first draft: the narrowing rule and its `Program` hook, and the
refinement *record* plus its `Program` hook — §3 rejects the set's type role, and
applying the predicate at lowering reduces enforcement to the assert channel
that already ships. The attribute's slot layout — one function, no propagation.
The branch-pending **deferral budget** — §4 removed the deferral cause rather
than budgeting for it.)

## 9. Phases

- **Phase 0 — the mechanism.** The domain condition the refinement is built
  from, and its insertion as a lowlevel `assert` on the operand
  (`register_assert`), with a refinement flavour so the diagnostic reads as a
  contract failure rather than `assert failed`. **The condition cannot be spelt
  with `==`**: `TypeOperator::Eq` compares through `ValueExt::value_eq`, which
  for an array is *handle* identity, so a structurally identical class node out
  of another module would compare unequal and the refinement would fail
  spuriously. The membership test must decode structurally, like
  `shape::low_type_of_slot` already does — whether as a new `TypeOperator`
  variant or as a checker-built structure over it is open.
- **Phase 1 — the contract on the builtin operators.** `check_binop`'s operand
  tie is already landed (§5); what remains is the domain condition for the
  both-operands-open case, the `T{…}` surface spelling and its lowering, and the
  kernel's authorised default (§8.6). `add` is polymorphic; a wrong-class use is
  refused. *This is the user-visible feature.*
- **Phase 2 — the dependent if.** `if` desugars to the tuple read `(e, t)(c)`
  instead of the array read `[e, t][c]`, and the claimed laziness of an
  unselected arm is measured. Unlocks user-written generic numeric functions.
- **Phase 3 — the implementation moves to std.** The leaf-selection dispatch, the
  split leaves, `Num` and the operator bindings in `lichen-std`; routing R3 →
  R2/R1.

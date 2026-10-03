# Operator polymorphism: a strict type contract over a lichen dispatch

> Status: **proposed** — nothing here is implemented. The architecture was
> settled in discussion before any code: **the type and the implementation of
> an operator are separate, and the type is allowed to be stricter than the
> implementation** (§2). The contract is a **refinement** (§3): a predicate on
> the operand's *value* that must evaluate to `1`, carried as an ordinary
> **attribute** and enforced by the lowlevel. The implementation is an ordinary
> lichen function that selects a per-class leaf and applies it (§4). The panic
> arm is dead by construction (§5).
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

> **An operator's type and its implementation are separate artifacts, and the
> type may be stricter than the implementation.**

- The **type** is a contract: `+ : ∀a ∈ {Int, Float}. a → a → a`. It is what
  rejects `add "a" "b"` at check time, what every instantiated use commits to
  a concrete class with, and what a kernel reads to pick a machine op.
- The **implementation** is a dispatch: an ordinary lichen function that asks
  the type value which machine op to run. It is written for *every* type
  value, including the ones the contract excludes — its last arm is a panic
  the contract proves unreachable.

This is the standard shape of a bounded-polymorphic builtin: Haskell's
`Num a => a -> a -> a` is a contract over a dictionary the class system
supplies; here the contract is a set type (§3) and the dictionary is the
operand's own type value read by an `if` chain (§4). The separation is what
lets each half be simple: the type layer never sees the dispatch, the dispatch
never restates the contract, and the "impossible" arm needs no clever type —
it needs a proof it is never taken, which the contract *is*.

Two measured facts bound what the implementation can look like:

- **The runtime dispatch already exists.** `TypeOperator::run` picks integer
  or IEEE arithmetic from the operand *values*. An `if` chain over
  `type_of x` is the same dispatch written in the language, one level up —
  which is the point: the semantics of `+` becomes library code, the same
  trajectory `type_of` took ([type-of-in-std](type-of-in-std.md)).
- **A homogeneous container cannot hold the dispatch.** `if` desugars to a
  branch array whose element type is one cell (`compile.rs`: "the branch
  array is homogeneous"), and a table literal unifies every entry's value
  into one cell (`check_table_term`). `if t == Int then $iadd x y else
  $fadd x y` puts an `Int` and a `Float` in that cell — and worse, the two
  branches share `x`/`y`, so even the operands cannot satisfy both branches
  in one definition. §4 is the rule that loosens exactly this.

## 3. The contract: a refinement on the operand's value

The constraint is not a type. It is a **refinement** — classical
`{v | p v}`, with the base type left as the ordinary inference cell and the
predicate `p` required to evaluate to `1`:

> A refinement is **one predicate**, a function of the refined value; a
> conjunction of refinements is one predicate written with `*` (`0 * 1 = 0`,
> so `*` is the conjunction over this language's `0`/`1` scalars), and `1` —
> the `*` identity — is "no constraint".

It attaches to a value as an **attribute** (`AttrExt`), which is the one
mechanism in this codebase that already carries a non-equality constraint on an
expression across every journey the graph takes. `Perspective` is the proof it
works end to end, and the refinement is its shape with three parameters
changed:

| | [`Perspective`](../../crates/lichen-perspective/src/perspective.rs) | refinement |
|---|---|---|
| slot holds | a lattice value (a thread count) | **a `0`/`1` condition** |
| `combine` | `Gcd` — the meet | **`*` — conjunction** |
| `missing_value` | `0`, the `gcd` identity | **`1`, the `*` identity** |
| reconcile | `check_unify_relaxed` + `is_subtype` | **a plain unify** |

What each row buys:

- **The carrier is free.** The slot is element `2 + i` of the term's
  `[value, type, attrs…]` pair (`shape.rs`'s `PAIR_ATTR_BASE`/`attr_slot`). The
  pair is an array value, so it is cloned by the apply walk, serialized into the
  artifact arena, and kept alive by the block that owns it — with **no new
  cloning, codec, or GC obligation**, which is exactly what a `TypeSet` marker
  would have needed.
- **Propagation is free, and wanted.** `combine` derives an expression's slot
  from its direct sub-expressions', so the refinements of a subtree accumulate
  into one condition at its root: for `x + y` that is `p x * q y`. This is a
  *fact* lattice and it is monotone in the safe direction — a refinement that
  holds of the parts is required of the whole.
- **Reconciliation is a plain unify.** Requiring the *same* refinement rather
  than a weaker one is over-strict, and deliberately so: weakening would need
  implication between predicates (`fact ⊨ requirement`), which is the subtyping
  this language does not have. Unify never wrongly *accepts*, so the strictness
  costs expressiveness and buys soundness.
- **Enforcement is the lowlevel's, and mostly already written.** The check is
  "force the condition, require `USize(1)`; a condition that stays lazy is
  pending, and the apply clone re-checks the instantiated condition per call" —
  [`Module::check_asserts`](../../crates/lichen-lowlevel/src/assert.rs), with
  `AssertError`'s `{condition, template, value}` provenance already carrying
  what a diagnostic needs to attribute a span. What the lowlevel does **not**
  have is the *association*: "this constraint refines that value". That record,
  and the `Program` hook that tells the lowlevel where a node's refinement is
  (so a frozen callee's is recovered from the graph rather than duplicated
  beside it), is the whole of the new mechanism.
- **Rendering is free.** `AttrExt::render` is what spells `# 4` today; §8.1's
  spelling question lands there.

Two consequences follow, both accepted:

- `check_index`'s bounds constraint (`i < length`, registered as an assert on
  the parameter and re-checked per call) is **already a refinement** in all but
  name. This mechanism generalises that path rather than opening a parallel one,
  which is why the lowlevel half is small.
- The lowlevel apply unifies the parameter pair **positionally**, so the
  parameter's live refinement slot must stay *unbound* — the same trick
  `check_lam` already plays for a perspective, and for the same reason: binding
  it would let the deep pass bake it and make the apply enforce the declared
  refinement by equality. Kept unbound, the slot binds what the call carries,
  and the declared refinement is enforced where the value is: at the parameter.

The contract has **two halves**, and §2's separation principle is what keeps
them apart:

- The **domain** is a *value*: a **class set** — a tagged list of member type
  values, `[TypeSet, [members]]`. Kept, but demoted out of the type system: the
  tag is a plain value constant (spelled beside `TypeId`, which is likewise
  **not** a kind marker), and the encoding is deliberately *not* a kinded type
  expression `[shape, [marker, K]]`. That is what makes "not a type" a property
  of the representation rather than a convention: `low_type_of` reads slot 0 as
  a shape, finds a tag that is no marker, then reads slot 1 as a kind, finds a
  one-element list where a `[marker, K]` belongs, and answers `Unknown`. The set
  is what a declaration *names* and what a reader that needs the **candidates**
  consumes — the kernel (below) and the diagnostics.
- The **check** is a refinement: the predicate of the refined value, which must
  evaluate to `1`. This is the half that is enforced, and it is §3's subject.

A set is never a runtime value's type and never a member of a type spine. Nothing
unifies against it: `+ : ?a -> ?a -> ?a` stays exactly that, and no rule is added
to `unify_inner`.

The contract attaches to a parameter as an ordinary annotation:

```lichen
-- lichen-std, end state
Num  = {Int, Float}             -- the domain VALUE (a class set)
in_num = v => type_of v ∈ Num   -- the refinement (the check)
is_int = v => type_of v == Int
iadd = …                        -- the machine leaves, today's TypeOperator
fadd = …                        --   split per class (see §6)
add  = x => y => { x: Num; y: Num      -- the contract
                   (fadd, iadd)(is_int (type_of x)) x y }
```

`x: Num` is an attribute on the parameter, so `x`'s *type* cell stays open —
that is the polymorphism. `add`'s printed type is `?a -> ?a -> ?a` with the
refinement shown beside it (spelling in §8.1).

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
add = x => y => { x: Num; y: Num
                  (fadd, iadd)(is_int (type_of x)) x y }
```

Measured on `dev` before any of this — the behaviours this has to preserve or
fix, taken with `lichen-compiler`:

| program | today |
|---|---|
| `add 1 2` | `3: Int` |
| `add 1.5 2.5` | **fails**: `expected Int, found Float` at the `1.5` |
| `add "a" "b"` | fails: `expected Int, found string` |
| `add 1 1.5` | fails: `expected Int, found Float` at the `1.5` |

- **Definition.** `x: Num` and `y: Num` attach the domain as an attribute; the
  *type* cells stay open, so the signature is `?a -> ?b -> ?r` until a leaf's
  contract closes it (`?a -> ?a -> ?a` for a leaf `?a -> ?a -> ?a`). The
  refinement conditions build, stay lazy, and register for enforcement.
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
  independently — that *is* let-polymorphism — so both clones satisfy `Num`.
  Under the end state the refusal is the **selected leaf's own contract**:
  `iadd`'s parameter is `Int`, so `iadd 1 1.5` is refused at `iadd`. That is an
  apply-time (runtime-channel) refusal rather than a check error at the argument,
  and §8.5 records the choice.
- **`compute.jit (y => y + y)`.** The parameter's type cell is open, so the
  domain is undecided — but its refinement names the **set**, whose candidates
  are data. The kernel reads them and takes the default class, `Int`: today's
  default, now authorised by the declared domain instead of by a pin that had to
  be erased. With the class concrete the `Index` selects one leaf and it lowers
  to `KernelBin::Add` — byte-identical kernels, no new optimizer.

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

1. **The printer's spelling** of a refined cell: `?a -> ?a -> ?a where ?a ∈ Num`
   vs `Num -> Num -> Num` once `Num` is importable. `AttrExt::render` is where it
   lands.
2. **`Num`'s home**: std binding (the `type_of` precedent) vs keyword.
3. **The panic arm's spelling**: the recorded-refusal channel needs a
   value-level form a library function can write; today only builtins record.
   A `$panic` leaf with a free type cell is the minimal answer.
4. **What the attribute's slot holds.** §3 settles the *shape* — one `0`/`1`
   condition, combined by `*`, missing value `1`, reconciled by a plain unify —
   but the **domain set** still has to be reachable from it for the kernel and
   the diagnostics (§5's last bullet). Two workable layouts: the slot holds a
   pair `[condition, domain]` (`combine` multiplies the first half and
   intersects the second — the intersection logic the rejected design had in
   `unify_inner`, moved to where it is pure value computation); or the slot holds
   only the condition and the domain rides on the declared side, which then has
   to reach a frozen callee from the graph rather than from the checker.
5. **Which channel refuses `add 1 1.5`.** §5 shows the refinement does not, by
   itself. The end state's answer — the selected leaf's concrete parameter type,
   refused by the ordinary apply — is an *apply-time* refusal
   (`DiagKind::Runtime`), where today it is a check error at the argument. That
   is the same channel `f = x => x + 1; f Type` already uses, so it is the
   language's existing answer to a per-call mismatch, but it is a visible change
   of diagnostic kind and position.

(Closed since the first draft: the narrowing rule and its `Program` hook — §3
rejects the set's type role, so nothing unifies against a domain. The
branch-pending **deferral budget** — §4 removed the deferral cause rather than
budgeting for it.)

## 9. Phases

- **Phase 0 — the mechanism.** The refinement record and the `Program` hook that
  tells the lowlevel where a node's refinement is; the force-and-require-`1`
  pass (generalising `check_asserts`) with its own error record and provenance;
  the `AttrExt` refinement with `combine = *`, `missing_value = 1`, a plain-unify
  reconciliation, and a `render`. The domain value kept as a *value*: tag,
  encoding, codec tag, printer. Nothing user-visible.
- **Phase 1 — the contract on the builtin operators.** `check_binop` gives
  `+ - * /` and the order comparisons the domain refinement instead of the pin
  (R3); the kernel reads the domain value for its default. `add` is polymorphic;
  a wrong-class use is refused. *This is the user-visible feature.*
- **Phase 2 — the dependent if.** `if` desugars to the tuple read `(e, t)(c)`
  instead of the array read `[e, t][c]`, and the claimed laziness of an
  unselected arm is measured. Unlocks user-written generic numeric functions.
- **Phase 3 — the implementation moves to std.** The leaf-selection dispatch, the
  split leaves, `Num` and the operator bindings in `lichen-std`; routing R3 →
  R2/R1.

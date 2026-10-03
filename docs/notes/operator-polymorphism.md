# Operator polymorphism: a strict type contract over a lichen dispatch

> Status: **proposed** — nothing here is implemented. The architecture was
> settled in discussion before any code: **the type and the implementation of
> an operator are separate, and the type is allowed to be stricter than the
> implementation** (§2). The type contract is a first-class class-set value
> (§3); the implementation is an ordinary lichen function that dispatches with
> `if t == Int then … else if t == Float then … else panic`, checked under a
> new dependent-if rule (§4); the panic arm is dead by construction (§5).
>
> Points at: `crates/lichen-highlevel/src/checker/operators.rs` (`check_binop`,
> the pin this removes), `crates/lichen-language/src/compile.rs` (the
> homogeneous branch-array desugar §4 replaces), `crates/lichen-highlevel/src/
> checker/indexing.rs` (`check_index`, the lazy `Index` the dependent if
> reuses), `crates/lichen-lowlevel/src/equality.rs` (`unify_inner`, where the
> narrowing rule lives), `crates/lichen-highlevel/src/shape.rs`
> (`for_each_kind_marker!`, the set value's encoding), `crates/lichen-compute`
> (the defaulting point), and `lichen-std/_.lichen` (the operators' end-state
> home).
>
> Companions: [operators](operators.md) (the operator set this edits),
> [floating-point](floating-point.md) §4.2 (the no-conversion rule this must
> not weaken), [type-of-in-std](type-of-in-std.md) (the precedent for moving a
> language form into the library),
> [defer-pending-type-forms](defer-pending-type-forms.md) (the deferred
> unification machinery the dependent if builds on).

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

## 3. The type contract: a class-set value

The constraint `a ∈ {Int, Float}` must survive the three journeys the graph
takes without the checker — apply-time cloning, persistence, incremental
retention — so it lives **in the graph, as a value**: a *class-set type* whose
members are ordinary type values, encoded as a tenth kind marker
`[members, [TypeSet, K]]` so the printer, codec, and checker dispatch derive
from `for_each_kind_marker!` (`floating-point.md` §3.2 is the field guide;
`TypeId` holds codec tag `8`, so the next marker is `10`). A set is never a
runtime value — no runtime value is "an Int or a Float, not yet decided" — it
only ever sits in *type* position, which the kind-marker encoding states in
the representation.

One additive rule in `unify_inner`, matched on the `TypeSet` marker:

| unify | rule |
|---|---|
| set ∩ concrete member | membership check; commit the concrete type |
| set ∩ concrete non-member | the ordinary unify error — `add "a" "b"` fails at check |
| set ∩ set | intersect; empty is the error, singleton commits |
| set ∩ unbound cell | the cell commits to the set |

No existing arm changes; no program today contains a set node, so every old
path runs verbatim. Narrowing is **not** subtyping: a set cell that narrows
commits to the member, there is no widening and no value conversion —
`1 + 1.5` still fails exactly as `floating-point.md` §4.2 demands.

The contract attaches to the implementation as an ordinary annotation, because
a type is a value and an annotation is a unify:

```lichen
-- lichen-std, end state
Num  = …                        -- the {Int, Float} set value
iadd = …                        -- the machine leaves, today's TypeOperator
fadd = …                        --   split per class (see §6)
add  = x => y => {
  x: Num                                    -- the contract
  t = type_of x
  if t == Int then iadd x y else if t == Float then fadd x y else panic
}
```

`x: Num` constrains the parameter's cell through the existing annotation
machinery; `y` and the result share that cell through the dispatch's applies
and the signature. `add`'s printed type is the contract, not the dispatch:
`?a -> ?a -> ?a where ?a ∈ Num` (spelling bikeshed in §8).

## 4. The implementation: a dependent if

The one new checking rule. Today `if c then a else b` is `[b, a][c]` with a
homogeneous element cell. The dependent reading keeps the **value** exactly as
it is — the lazy `Index` already evaluates only the taken branch — and changes
only the **type slot**: the branch array's element *type* is itself the lazy
read

```text
elem_ty = Index([type_of b, type_of a], c)
```

i.e. the type of `if c then a else b` is `if c then type_of a else type_of b`
— the same `Index`, one level up. This is the dependent-types answer to a
conditional (Bool elimination with the motive computed from the scrutinee),
and it is cheap here precisely because types are values computed by the same
lazy machinery the values use.

Its two consequences, each load-bearing:

- **Branch types may differ.** The `Int` branch and the `Float` branch no
  longer meet in one cell; per instantiation `c` is concrete, the `Index`
  resolves, and the result's type is the taken branch's.
- **An untaken branch's constraints do not fire.** Both branches are still
  *built* (the checker walks them), but a unify that only the untaken branch
  demands — `$fadd x y` with `x` an `Int` — must defer, and fire only if its
  branch is ever taken. This is the deferred-unification machinery of
  [defer-pending-type-forms](defer-pending-type-forms.md) given one more
  deferral cause: *branch-pending*. The existing apply-clone re-check then
  gives the right behaviour per call site: for `add 1.5 2.5` the `Int` arm's
  constraints never fire; for `add 1 1.5` the taken arm's operand unify fires
  and reports `expected Int, found Float` — the same refusal as today, at the
  same concreteness.

And the exhaustiveness arm: `panic` (the language's recorded-refusal channel,
the same one `operator.divide_by_zero` uses) has a free type cell that unifies
with anything, so the chain always has a last arm. Under the §3 contract that
arm is dead — see §5.

What the dependent if does **not** do: it does not constrain `?a` to numerics.
`if t == Int then … else …` on a `string` operand still *checks* — the type
layer's job is §3's, and this is the separation principle again: the if rule
makes the dispatch expressible, the set value makes it safe.

## 5. The worked example

```lichen
add = x => y => { x: Num; t = type_of x
                  if t == Int then iadd x y else if t == Float then fadd x y else panic }
```

- **Definition.** The contract commits `x`'s cell to the set; the signature is
  `?a -> ?a -> ?a where ?a ∈ Num`. Both real arms build; their branch-pending
  unifies wait. The panic arm's free cell closes the chain.
- **`add 1 2`.** Apply clones; `1` narrows the clone to `Int`; `t` evaluates to
  the `Int` type constant; the `Index` takes arm 1; `iadd 1 2` is `2: Int`.
  The float arm's unifies never fire.
- **`add 1.5 2.5`.** The clone narrows to `Float`; arm 2 runs; `3.0: Float`.
- **`add "a" "b"`.** The argument's type meets the contract's set: non-member —
  **a check error naming the domain** (`expected a numeric class (Int or
  Float), found string`). The dispatch never runs; the panic arm is
  unreachable for any use the contract admits. *This is "the type is stricter
  than the implementation" made concrete.*
- **`add 1 1.5`.** Contract admits both operands one at a time, but they share
  one cell: `1` commits it to `Int`, `1.5` is then a non-member — the same
  refusal as today.
- **`compute.jit (y => y + y)`.** At lowering the body's class is the set, and
  the kernel answers an undecided-or-set domain with `Int` — today's default,
  arrived at honestly. With the class concrete the condition `t == Int` is a
  constant, the if folds to arm 1, and `iadd` lowers to `KernelBin::Add`:
  byte-identical kernels, no new optimizer — the fold is the class read the
  lowering already does ([compute-jit-low-types](compute-jit-low-types.md)),
  applied to a constant condition.

## 6. The leaves

The dispatch's arms call per-class machine leaves. Today's `TypeOperator::Add`
is *already* class-polymorphic at run time (it reads the operand values), so
two shapes work:

- **Keep the unified leaf**: `iadd` and `fadd` are both the existing operator;
  the `if` chain is then the *specification* of the dispatch (and the place a
  future class plugs in), while `run` keeps doing what it does. Zero lowlevel
  change.
- **Split the leaf per class**: each arm names its machine op, the `run` arm
  for a wrong-class leaf records a refusal, and the dispatch is load-bearing
  all the way down. Cleaner failure isolation, one more `TypeOperator`
  variant per operator per class (codec tags append-only, as ever).

Either satisfies the design; the split is the honest end state because it
makes the arms' types (`Int -> Int -> Int`, `Float -> Float -> Float`)
concrete and independent, which is what lets an untaken arm's unify be
*wrong* and deferred rather than accidentally right. `%` and the bitwise trio
keep the singleton contract `{Int}` — behaviour identical to today's pin, one
uniform mechanism. `==`/`!=` stay unconstrained (the generalized equality).

## 7. Routing a surface operator to the function

How `a + b` reaches the std function, unchanged from the earlier analysis and
still phased: **R1** a prelude desugar (principled end state; needs the
language's first implicit import), **R2** an intrinsic registry (the checker
resolves `ir::BinOp` to the registered function value and checks an ordinary
apply; the emitter recognises "apply of an intrinsic whose dispatch folds to
one leaf" so kernels see `KernelBin` as today), **R3** the checker's special
case stays but *reads the contract from the std binding's type* instead of
restating `{Int, Float}` in Rust (waypoint). The dependent if and the set
value are routing-agnostic: phases 0–2 below land the semantics under R3, and
R2/R1 are the "operators are library functions" end state whenever the prelude
question is answered.

## 8. Open questions

1. **The printer's spelling** of a constrained cell: `?a ∈ Num` vs
   `Num -> Num -> Num` once `Num` is importable.
2. **`Num`'s home**: std binding (the `type_of` precedent) vs keyword.
3. **The panic arm's spelling**: the recorded-refusal channel needs a
   value-level form a library function can write; today only builtins record.
   A `$panic` leaf with a free type cell is the minimal answer.
4. **Deferral budget.** Branch-pending unifies defer work the checker today
   does eagerly; a deeply nested `if` chain (three or more classes, one day)
   composes deferrals, and the error a user sees must still name the arm that
   fired.

## 9. Phases

- **Phase 0 — the set value.** The `TypeSet` marker, the narrowing rule, shape
  decode treating a set as undecided, printer spelling, persist round-trip.
  Nothing user-visible.
- **Phase 1 — the contract on the builtin operators.** `check_binop` builds
  the §3/§6 domains (R3); the kernel default. `add` is polymorphic; wrong
  classes are check errors. *This is the user-visible feature.*
- **Phase 2 — the dependent if.** The type-slot `Index` and branch-pending
  deferral. Unlocks user-written generic numeric functions.
- **Phase 3 — the implementation moves to std.** The `if`-chain dispatch, the
  split leaves, `Num` and the operator bindings in `lichen-std`; routing R3 →
  R2/R1.

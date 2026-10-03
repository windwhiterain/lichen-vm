# Operator polymorphism: `+ - * /` over a finite class set

> Status: **proposed** — nothing here is implemented. The design was settled in
> discussion before any code: the constraint is a **first-class set type value**
> (§2), narrowing is not subtyping (§3), the only defaulting point is kernel
> lowering (§4), and a surface operator is *routed* to a library function rather
> than hardwired to an IR node — but only in phases, because the routing crosses
> the kernel lowering (§6).
>
> Points at: `crates/lichen-highlevel/src/checker/operators.rs` (`check_binop`,
> the pin this removes), `crates/lichen-highlevel/src/program.rs`
> (`TypeOperator::run`, the dispatch that is already polymorphic),
> `crates/lichen-lowlevel/src/equality.rs` (`unify_inner`, where the narrowing
> rule would live), `crates/lichen-highlevel/src/shape.rs`
> (`for_each_kind_marker!`, the likely encoding), `crates/lichen-compute` (the
> defaulting point), and `lichen-std/_.lichen` (where the operators' signatures
> would live at the end state).
>
> Companions: [operators](operators.md) (the operator set this edits),
> [floating-point](floating-point.md) §4.2 (the no-conversion rule this must not
> weaken), [type-of-in-std](type-of-in-std.md) (the precedent for moving a
> language form into the library).

## 1. What is missing

`check_binop` decides the class of an arithmetic operation once, at check time:
a concretely `Float` operand selects `Float`, and **every other operand — an
`Int`, an undecided parameter — pins the whole operation to `Int`**
(`checker/operators.rs`: the `names_float_class` default). The pin is what kills
polymorphism:

```lichen
add = x => y => x + y   -- today: Int -> Int -> Int
add 1.5 2.5             -- today: expected Int, found Float
```

The language's let-polymorphism is structural — every application clones the
parameter cells fresh (spec: "every lambda is automatically let-polymorphic") —
so `x => x` is polymorphic precisely because nothing ever binds its type cell.
The operator pin binds it early. The fix is not to make the operator
"multi-typed"; it is to let the cell stay **free but constrained**: free enough
to clone, constrained enough that `add "a" "b"` is still a check error.

In type-system terms the target is a textbook one — bounded quantification over
a **finite, closed domain**, `+ : ∀a ∈ {Int, Float}. a → a → a`. This is SML's
equality-type-variable trick (`''a`) generalised from "equality" to "numeric",
or equivalently Haskell's `Num` with the class, the instances, and the
dictionary all built in and closed. It is deliberately **not** user-extensible
type classes (§7 names what that would cost).

## 2. The constraint is a value: the class-set node

The one architectural decision everything else follows from. A constraint on a
type cell has to survive three journeys the graph takes without the checker:

- **apply-time cloning** — the lowlevel apply machinery clones parameter cells
  per call site; a checker side-table keyed by node id does not come along, and
  an unconstrained clone is `add "a" "b"` passing again;
- **persistence** — a frozen artifact round-trips values, not checker state;
- **incremental retention** — the resolver keys expressions by content, not by
  checker tables.

So the constraint lives **in the graph, as a value**: a *class-set type* whose
members are ordinary type values. This is the same move the language has already
made three times — the universe is a value (`K = [Type, ↺]`), a recursive struct
is a cyclic value, a type is a value — and it is what "a type is just a value"
means when the type you need is "one of these two".

**Encoding: a tenth kind marker, not a new `LowValue` variant.** The set is a
compound type value `[members, [TypeSet, K]]` whose members slot holds the
element type values. `for_each_kind_marker!` exists precisely so that a new type
form derives its `TypeValue` variant, `Ctx` accessor, checker dispatch, printer
arm and artifact codec from one line (`floating-point.md` §3.2 is the field
guide, including the codec-tag trap: `TypeId` holds `8`, so the next marker is
`10`). A `LowValue` variant would instead buy the three-tag-space dance
(`floating-point.md` §3.6) for a form that is never a *runtime* value — no
runtime value is "an Int or a Float, not yet decided"; the set is a static
over-approximation that only ever sits in *type* position. The kind-marker
encoding states that in the representation.

**The unify rule is additive and lives in the lowlevel.** One new case in
`unify_inner`, beside the existing structural arms:

| unify | rule |
|---|---|
| set ∩ concrete member type | membership check; commit the concrete type |
| set ∩ concrete non-member | the ordinary unify error — `add "a" "b"` fails at check |
| set ∩ set | intersect; empty is the error, singleton commits |
| set ∩ unbound cell | the cell commits to the set |

No existing arm changes: no program today contains a set node, so every old
unify path runs verbatim. The delicacy of touching `unify_inner` is on record
([universe-containment](universe-containment.md) §3) — the mitigation is that
this rule only *adds* a branch matched on the `TypeSet` marker, it never edits
one.

## 3. Narrowing is not subtyping, and nothing converts

Two lines the design must not cross, both already on the books:

- **No subtyping.** A set cell that narrows to `Float` *commits* to `Float` —
  the cell's value becomes the member type, exactly as if it had unified with a
  literal's type all along. There is no "is-a" relation, no widening, and no
  value whose runtime representation the set describes. `1 : {Int, Float}`
  leaves the pair's type slot `Int`; the set only ever constrains cells that
  are not yet concrete.
- **No conversion.** `floating-point.md` §4.2 is untouched: `1 + 1.5` still
  fails, now as "set ∩ concrete non-member" — `1` commits the shared cell to
  `Int`, `1.5` is not a member of `{Int}`, the diagnostic is the same
  `expected Int, found Float` a user sees today. The two class crossings remain
  the only doors between the classes.

## 4. Which operators, which domain, and the one defaulting point

| operators | domain | result |
|---|---|---|
| `+ - * /` | `{Int, Float}` | the shared cell |
| `< > <= >=` | `{Int, Float}` | `Int` (the `0`/`1` scalar, as today) |
| `% & \| ^` | `{Int}` | `Int` — behaviour identical to today's direct pin; the mechanism is now uniform |
| `== !=` | unconstrained | unchanged — the generalized equality already unifies any two same-typed values |

`{Int}` deserves one sentence: giving the `Int`-only operators the singleton
domain is not a nicety, it is what keeps *one* rule in `check_binop` — "build
the domain set, share one cell across operands, unify each operand against it"
— with no second code path that can drift.

**The interpreter never defaults.** `TypeOperator::run` already dispatches on
the operand *values* (`program.rs`: two `USize`s go to wrapping arithmetic, two
`Float`s to IEEE), and `low_type` already transfers `Float` if any argument is a
float, `USize` otherwise. A polymorphic `add` applied at floats needs nothing
new at run time — this is the measured fact the whole design stands on.

**The kernel is the only defaulting point.** A JIT'd kernel must commit a class
per fragment. Today's behaviour is the default already: `compute.jit (y => y +
y)` checks its body as `Int` and lowers `I64` arithmetic. Under the set design
the body's cell is `{Int, Float}` when lowering reads it, and the lowering
answers an undecided-or-set domain with `Int` — the same answer, arrived at
honestly (the low-type channel already seeds this; see
[compute-jit-low-types](compute-jit-low-types.md)). An author who wants the
float kernel writes the annotation, exactly as today. **No program that runs
today changes its kernel.**

**Printing needs no default either.** A still-constrained cell prints as what it
is: `add` renders `?a -> ?a -> ?a where ?a ∈ {Int, Float}` (spelling bikeshed in
§8, but the constraint is *shown*, not silently defaulted — the Haskell wart
this avoids is defaulting that changes meaning at an invisible boundary; here
the only boundary that defaults is the one that must commit machine code).

## 5. The worked example

```lichen
add = x => y => x + y
```

Checking: `x` and `y` share one fresh cell through the operator; the cell
commits to the set value `[<Int, Float>, [TypeSet, K]]`. The lambda's type is
`?a -> ?a -> ?a` with `?a`'s representative the set.

- `add 1 2` — apply clones the cells; `1` narrows the clone to `Int`; the
  result pair is `2: Int`.
- `add 1.5 2.5` — the clone narrows to `Float`; `run` dispatches on the float
  values. `3.0: Float`.
- `add "a" "b"` — set ∩ `string` is the unify error, at check time, naming the
  domain: `expected a numeric class (Int or Float), found string`.
- `compute.jit (y => y + y)` — body cell is the set at lowering; the lowering
  defaults `Int`; the wasm/SPIR-V output is byte-identical to today's.

## 6. Routing: a surface operator is a library function

The end state, and the question that shapes the phases: **how does `a + b`
reach a function defined in lichen-std?** The demand is the same one that moved
`type_of` into the library ([type-of-in-std](type-of-in-std.md)): *a form the
language can express has no business in the language definition*. Two facts
bound what "express" can mean here.

**The dispatch itself does not need re-expressing.** A natural first sketch is
a type-keyed table in std:

```lichen
-- does NOT check, and the reason is measured, not stylistic
add = x => y => table{ Int ==> $iadd, Float ==> $fadd }{ type_of x } x y
```

`check_table_term` unifies every entry's value into **one** value-type cell
(`checker/indexing.rs`): `Int -> Int -> Int` and `Float -> Float -> Float`
conflict, and making them not conflict needs a *dependent* table type — the
entry's type as a function of its key — which the table type
`[[key, value], [TypeTable, K]]` has no slot for. The same wall stops the
`[$iadd, $fadd][type_of x == Int]` spelling (arrays are homogeneous too). But
the wall does not matter, because the dispatch it would express **already
exists**: `TypeOperator::run` is the class dispatch, keyed by the operand
values, one layer down. The primitive leaf is the machine op; the library
function's job is the *signature*, not the dispatch:

```lichen
-- lichen-std, end state
Num = …                        -- the {Int, Float} set value
add = x => y => {x: Num; $add x y}   -- $add is today's TypeOperator::Add
```

The annotation `x: Num` attaches the set through the ordinary annotation
machinery; `$add`'s `run` picks the machine arithmetic from the values, as it
does today. The lichen function is thin on purpose: all the semantics live in
the set's unify rule (one place) and the leaf's `run` (one place).

**What remains genuinely hard is the routing**, and it has a real blocker on
each route:

- **R1 — desugar to a prelude name.** `a + b` parses as an application of a
  well-known binding. Blocks on two things the language deliberately does not
  have: an *implicit prelude* (today every program imports std explicitly; a
  program with no imports would have no `+`, which is unacceptable, so the
  prelude must be implicit — a new language concept), and *kernel recognition*:
  `compute.jit (y => y + y)` must still lower to one `KernelBin::Add`, so the
  lowering must inline the prelude function and re-recognise the leaf — a
  bounded, special-cased inline, but an optimizer where none exists.
- **R2 — intrinsic registry.** Keep `ir::BinOp`; the checker resolves the
  operator to a *function value* from an intrinsic module compiled at startup
  (the way `compute.lichen` is embedded with its private native registry), and
  checks the expression as an ordinary apply of that function — cloning,
  narrowing and polymorphism all inherited from the apply machinery. The
  emitter still special-cases "apply of intrinsic whose body is one native op"
  to the op node, so kernels see exactly what they see today. No prelude, one
  bounded recognition rule; the cost is a new compiler concept (the intrinsic
  module) rather than a new language one.
- **R3 — signature in std, special case stays.** `check_binop` keeps emitting
  the op node but *reads the constraint from the intrinsic function's type*
  instead of restating `{Int, Float}` in Rust. Minimal machinery, zero kernel
  work; the cost is the checker still knowing which operators exist (the
  five-layer story of [operators](operators.md) §2 is untouched).

The honest statement: R1 is the principled end state and the largest change;
R2 gets the semantic benefits (one source of truth for operator signatures,
written in lichen) without the prelude; R3 is a waypoint that is strictly
better than today and on the way to either.

## 7. What is deliberately not here

- **User-defined instances / type classes.** A class a user can extend wants
  instance resolution, dictionaries (or monomorphisation), and a coherence
  rule — a language feature, not an operator feature. The set domain is closed
  and built in. If user classes ever arrive, the set node is *not* wasted work:
  it is the finite case of "a variable with a constraint", which is the
  machinery any class system also needs.
- **Mixed-class arithmetic.** §3. `1 + 1.5` is an error by design, forever.
- **Literal polymorphism.** A literal's type is its own class
  (`floating-point.md` §4.2); `1` is an `Int` even where a `Float` would fit.
  Haskell's overloaded literals were considered and declined for the same
  reason the classes do not convert.
- **Surface syntax for set types.** The set value is built by the compiler (or
  bound in std as `Num`); whether a user can *write* `{Int, Float}` as a type
  expression is a separate question with its own grammar cost, deferred until a
  second consumer exists.

## 8. Open questions

1. **The printer's spelling.** `?a -> ?a -> ?a where ?a ∈ {Int, Float}` vs
   `{Int, Float} -> {Int, Float} -> {Int, Float}` vs `Num -> Num -> Num` when
   the set is the std binding. The first is the most honest at the REPL; the
   third reads best once `Num` is a name the user can import.
2. **`Num`'s home and name.** A std binding (`std.Num`) keeps the language
   small; a keyword makes the error messages shorter. Leaning std binding, per
   the `type_of` precedent.
3. **A set-typed value at an observation.** A parameter that is only ever
   added and never applied prints with the constraint (fine); a `cache` cell or
   an artifact whose persisted type is a set needs the codec to round-trip it
   (free under the kind-marker encoding, but needs a test).
4. **The `==` future.** The set mechanism is the same one an `Eq` constraint
   over type values would use if `==` ever grows a static domain; not in scope,
   recorded so the generalisation is not designed away.

## 9. Phases

Each phase is independently landable and independently useful:

- **Phase 0 — the set node.** The `TypeSet` kind marker, the lowlevel narrowing
  rule, `low_type_of_slot`/shape decode treating a set as undecided, the
  printer spelling, the persist round-trip. No operator changes; nothing
  user-visible.
- **Phase 1 — the operators.** `check_binop` builds domains per §4 (R3: the
  constraint stated in the checker); the kernel default at lowering; the
  diagnostics. This is the whole user-visible feature: `add` is polymorphic.
- **Phase 2 — the signature moves to std.** `Num` and the operator signatures
  become lichen-std bindings; the checker reads the constraint from the
  intrinsic type (R3 complete, or R2 if the intrinsic module lands here).
- **Phase 3 — routing.** R2's intrinsic registry or R1's prelude, with kernel
  recognition. Only this phase deletes `ir::BinOp`, and only if the kernel
  story stays byte-identical.

Phase 1 is the brainstorm's promise; phases 2–3 are the "operators are library
functions" end state and can wait for a second consumer of the machinery.

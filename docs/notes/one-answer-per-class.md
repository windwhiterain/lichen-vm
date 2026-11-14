# One answer per class

> Status: **current** — the maintainer's verdict that a "second answer" is not a concept (a
> class holds one answer, so the engine has no path that names one), the measured defect that
> used to produce `expected ?, found ?` on the examples, and the SPEC rule the defect is to be
> repaired by.
>
> Points at: `crates/lichen-highlevel/src/checker/structs.rs` (`check_instantiate`),
> `crates/lichen-highlevel/src/checker/templates.rs` (`instantiate_requirement_value_with`),
> `crates/lichen-lowlevel/src/equality.rs` (`write_node_answer`).

**The verdict.** A class holds one answer (`equality.rs` fills a class once and never
replaces it), and a node has one value. There is no second answer for the engine to
reconcile, so there is no "two answers on one node" concept in the implementation, no
diagnostic sentence that names one, and no code path that exists to report one. Where a
write disagrees with the class's value the engine reports an **ordinary unify failure**
(`DiagKind::Runtime`, rendered `expected X, found Y`), and the only correct response to one
is to remove the code that produced the second value.

## The defect, measured

A struct instance's field is the template's field node. `check_instantiate` copies the
template through `instantiate_requirement_value`, and `instantiate_requirement_value_with`
returns every frozen-typed requirement in place ("a frozen type half names a value outside
every applying function"). `CIn`'s `.x` field is declared `compute.Buf (…)`, a frozen `Buf`
type value with an open `.native` cell, so that early return hands the template's own node to
the use; the first instance's buffer is written into it and the next instance's buffer meets
it.

Measured with `examples/convolution2d.lichen`'s own program, each run through
`lichen-compiler`:

| program | reading |
|---|---|
| one kernel call | checks |
| three calls of one kernel, different buffers | checks |
| two kernels, the same `CIn` argument values | checks |
| two kernels, different buffers | `expected ?, found ?` |
| the second kernel given its own `CPar2` but sharing `CIn` | `expected ?, found ?` |
| the second kernel given `CPar2` and `CIn2` | checks, declared answer |

An attempt that made the frozen copy conditional (an instance copies a frozen-typed field
only when it holds an open cell, other requirements still reference it) passed this table and
the suites, but it **invented a rule that the SPEC does not have**: the SPEC has no frozen /
closed-cell distinction. It was reverted.

## The SPEC rule the repair follows

`docs/language-spec.md` §Struct gives the instance construction:

```sytax
`s{.n2 e3, .n1 e4}` :=
`{
  let _{.n1 e1, .n2 e2} = s;
  v := @raw[name_table, nominal_id, @raw None, @raw None]
  e3 = @raw v[name_table["n2"]+2]
  e4 = @raw v[name_table["n1"]+2]
  ...
}`
```

An instance's field is a **fresh `None` cell** of its own `v`; the template's field (`e1`,
`e2`) is only the requirement. So `check_instantiate` must build `v` with fresh field cells
and unify the arguments into those cells; the template's field node must never be the node a
use's value lands in. The frozen reference stays where it belongs — in a value handed over as
an argument (`5b016643`'s graph placeholder), not in a struct instance's construction.

**A straight reading of the snippet is blocked in the current representation.** The checker's
struct value is `[name table, nominal id, field *slot*…]`, where a field slot's value carries
the declared field's value (`compute.Buf (…)` for `.x`) and its type carries the declared
type (`Int` for `@any : Int`); `check_instantiate` unifies the argument slot with the
declared field slot, which is how a type-valued declaration (`{.x (compute.Buf …)}`) becomes a
type check. Building `v`'s field slot as `[fresh, declared value]` fails as
`expected Int, found Type` at the `BufParam` declarations (`examples/convolution2d.lichen:51`),
because that position is the declared *type*, not the declared value. The repair therefore
has to split the declared field into its requirement and the instance's value cell in this
representation — the next measurement is which slot a type-valued field's requirement belongs
in.

## Landed: the struct instance uses the module's clone

`check_instantiate` now instantiates its template with `Module::clone_template` — the lowlevel
walk that implements SPEC §Clone (§`Undecided`/`Any` included): an open cell becomes this
use's, a decided one is referenced. The checker's own `instantiate_requirement_value` is no
longer on this path.

Measured: `convolution2d.lichen` checks and prints its declared values; language **289**
(+69), highlevel **136**, compute **283** / 1 ignored, lowlevel **167** / 1 ignored,
compute-gpu **94**.

The array instance (`indexing.rs`) and the annotation requirement (`check_unify_expr`) still
call the checker's `instantiate_requirement*`. Moving them measured two failures, so they
stay until those are understood: the array instance's dependent length stopped being checked
(`('n -> {A := [@any : Int, n]; A[1, 2, 3]}) 5` no longer fails), and the annotation path broke
the compute package load (`expected raw[?a, ?b], found 0` at `compute = import "compute.lichen"`).

## What the diagnostic is now

`write_node_answer` still writes the operator's answer into the node's class; a class that
holds nothing takes it, and a value a unification wrote is not the operator's answer, so the
operator runs (the run stamp is the other axis). Where a different value does reach one
class, the renderer prints `expected X, found Y`, spelling the two values when neither side is
a handle. The earlier note that named a second answer, the sentence that named one, and the
`value_a`/`value_b` special case for two open sides are removed.

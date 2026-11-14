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

## The apply clone is not an instance rule (reverted)

`check_instantiate` was moved to `Module::clone_template` to make an instance's cells fresh.
It fixed `convolution2d.lichen` and kept every suite green (language **289** + 69, highlevel
**136**, compute **283** / 1 ignored, lowlevel **167** / 1 ignored, compute-gpu **94**), but
it is **not the SPEC's rule**: clone exists only at a function application, so an instance
must not use it. The change is reverted; the checker's `instantiate_requirement_value` stands
on that path until the strict construction lands.

The strict version (WIP branch `spec/instance-v`, `f48883c4`, not merged) removes the copies
at the struct instance, the array instance and the annotation: `convolution2d.lichen` then
**checks** (the collide is gone) but its result is `(raw none, raw none)`, because the kernel
body's field reads resolve through the declared type template, whose open cells the copy used
to fill with the argument.

## The SPEC's constructions are three, not one

The two measured failures are not a defect of the SPEC. The SPEC gives a **different rule for
each construction**, and the implementation applies one routine to all of them:

| construction | SPEC rule | consequence |
|---|---|---|
| struct instance `s{…}` | `v := @raw[name_table, nominal_id, @raw None, …]`; `e3 = v[…]`, `e1 = v[…]` | fresh field cells of this use's own `v` |
| array instance `e0[e1, e2]` | `let [template, length] := e0; length = 2; e1 = template; e2 = template` | **unified against the template's cells**, length included |
| table / set instance | `let table{kt, vt} = t; k1 = kt; …` / `let set{t} = t; e1 = t` | unified against the template's cells |
| function apply `f a` | `@clone`: undecided node in the applying scope → clone; decided → reference (`### Any`: an `Any` cell becomes `None` per use) | the application clone |

The array instance's `length = 2` is a **unify with the template's length cell**, so a
parameter-dependent length (`A := [@any : Int, n]`) stays the parameter's node and the later
call conflicts with the literal count. Cloning the template at the array instance detaches
that cell, and the check disappears — measured: the array instance on `clone_template` fails
`a_dependent_array_length_rejects_other_lengths`, while on its own routine it passes, and the
probe `('n -> {A := [@any : Int, n]; A[1, 2, 3]}) 5` fails at the call as the rule says. The
annotation path is `a : T`, a unify of the annotated attribute with the expected expression
(`## Annotation`), not an application: applying the application clone there breaks the compute
package load.

**Verdict.** The SPEC is coherent; the implementation is what is wrong — it has one
instantiation routine serving three constructions, and any single rule substituted for all
three contradicts one of them. The repair is per construction: struct → `v`'s fresh cells;
array/table/set → unify against the template (only `Any` cells are per use); annotation →
unify, no instantiate; application → `@clone`.

**Measured on the strict version (WIP branch `spec/instance-v`, `f48883c4`, not merged).**
With the copies removed — the struct instance built from the argument terms, the array
instance unified against the template, the annotation not instantiated — `convolution2d.lichen`
**checks** (the collide is gone) but its result is `(raw none, raw none)`. The reason is a
second implementation defect, not the SPEC: the kernel body's field reads
(`Module::parameter_annotations` / `declared_field_value` in `checker/structs.rs` and
`resolve_kernel_reads` in `checker.rs`) resolve through the **declared type template**, whose
open cells the old copy used to fill with the argument. Reading through the instance (`v`)
instead is the next piece. The copy in `check_unify_expr` is also what carried the type
template's clone (origin 185 in the measured class) into the instance's class, i.e. the
collision's source on the annotation path.

The examples survey confirms the strict version is a different failure set, not a smaller one:
dev reads **33 clean / 8 panicked / 7 drifted** (`convolution2d` and `filter` drift, `generate`
panics), the strict branch reads **32 clean / 9 panicked / 7 drifted** (`convolution2d` and
`filter` now panic, `generate` and `struct_recursion` drift). `gemv.lichen` runs and answers
`[14.0, 32.0, 50.0, 68.0]` on both; the loss is in what the launch's argument and the declared
template each carry, not in every kernel. So the read-path change is a prerequisite for
removing the copies, not a follow-up: with the copies gone the collide disappears, but the
value a launch reads has no declaration-side carrier.

**Traced one step further (strict branch `33d92bab`).** With `DBG_PL` on the launch, the strict
version prints **no** `PLRUN` line: the kernel launch never runs, so the result is `raw none`.
The partial strict state (the `check_unify_expr` copy kept) does print the launch's input paths
(`[3, 0]`, `[3, 1]`) and resolves the two `CIn` slots, and then collides — so the input
resolution works; what changes is whether the launch happens at all. The parallel op's run
requires a **materialized** function (`function_materialized`, and the fragment compile reads
the codomain declaration), and the checker's check-time copy in `check_unify_expr` was what decided the
cells the gate reads. Per the SPEC the declaration's open cells are not values, so the fragment cannot
be compiled from them: the compile must happen at the launch with the applied argument, or be
independent of the declaration's open cells. That is the next piece, and it is larger than the
instance construction itself.

**Follow-up probe on the strict branch.** The parallel gate is not what stays open:
`function_materialized` passes (the parameter's type is a decided array, `layout: 1`), and
`compile_parallel_fragment` answers `Ok` for all five parallels (the refusal arm never
prints), so a `ParKernel` is produced. Yet `PLRUN` still never prints: the **`ParLaunch` op
never runs**, so the launch's operand — the `convolve`/`replicate` binding — is undecided when
the launch is evaluated. The next probe is that operand at the launch's evaluation, not the
fragment compile.

**Where the SPEC puts the specialization.** §Function: *"Polymorphism at every application:
@clone function's expressions, unify the argument with the cloned parameter."* The source has
**no separate parameter annotation**: the parameter is one expression (`'k = CPar`), where
`'k` declares the parameter and `CPar` is an outer-scope reference in it. So the apply's clone
covers that whole expression — the cloned parameter's cells are what the argument unifies
with — and the checker's check-time copy in `check_unify_expr` is not that clone. The
remaining work is at the apply; the launch's operand stays undecided until the clone covers
the parameter expression. That is the SPEC's sentence, not a new mechanism.

**A negative result on the SPEC's `Any` form.** Rewriting the compute structs as
`{.x @any : (compute.Buf …)}` (the SPEC's polymorphic field form) is refused by the compute
read path: `expected Struct, found @any` at `k.in.x`, because the kernel's read needs the
**Buf value** in the declaration's field, not the wildcard. So the examples' type-valued field
(`{.x (compute.Buf …)}`) is what that path requires, and the instance rule must handle it: the
declaration keeps the type value, the instance's `v` keeps the buffer, and the read's value
comes from the launch's cfg.

**The launch's op is attempted once, before its operand is decided.** On the strict branch the
`ParLaunch` op node is built (the checker prints its id) and the deep pass **does** reach it
(`DEEP op` prints for it), but the launch's run never prints: the op's one attempt happened
while an operand was still undecided, so `evaluate_node_operation` recorded "ran undecided"
(the run stamp), and the deep pass now returns that empty answer instead of retrying. The
former copy's side-effect evaluation used to decide the operand before that first attempt.
So the repair is about **when** the first attempt happens — an attempt with undecided operands
must not spend the one run (`for effeciancy, one operation only run once`, SPEC §Operation) —
not about the instance construction.

**The gate that actually stops the launch is in `run_deferred`.** The compute operator's
`run_deferred` evaluates the operand deep, then walks every operand item and returns `None`
when `module.node_value(item)` is empty — before `run` is entered. On the strict branch the
kernel item of the launch's operand has no node value there, even though the parallel's own
`run` printed its gate and produced a `ParKernel`: so either the kernel's value lands in a
class the binding's node is not in, or the gate reads a node that is not the one the parallel
wrote. A version that stopped spending the run stamp on an open-operand attempt (a SPEC
§Operation reading) was built and measured: the launch still does not run — the gate, not the
stamp, is the blocker. That is the next probe.

## What the diagnostic is now

`write_node_answer` still writes the operator's answer into the node's class; a class that
holds nothing takes it, and a value a unification wrote is not the operator's answer, so the
operator runs (the run stamp is the other axis). Where a different value does reach one
class, the renderer prints `expected X, found Y`, spelling the two values when neither side is
a handle. The earlier note that named a second answer, the sentence that named one, and the
`value_a`/`value_b` special case for two open sides are removed.

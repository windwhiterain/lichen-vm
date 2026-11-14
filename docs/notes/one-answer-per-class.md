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
stamp, is the blocker.

**Measured on the launch operand itself (strict branch): the kernel item is `Any` and the
cfg is empty.** The gate trace names the launch's own operand nodes: the kernel item
(`NodeId(250v1)`) holds `LowValue(Any)` (a wildcard passes the gate), and the cfg item
(`NodeId(2454v1)`) has **no node value and no class value**. So the plrun's operands are
still template/cell nodes, not the applied call's arguments: the `a` parameter and
`k.native` inside `plrun`'s body have not resolved to the argument's cells under the strict
version. The former copy's evaluation is what resolved them. Per SPEC §Function the apply's
clone is what must put the argument in the cloned parameter's cells; that is the piece still
missing, and it is larger than everything tried so far.

**Measured: the apply's clone never visits the native call.** A trace of `node_apply` filtered to
External operators prints **zero** lines for `convolution2d`: the clone walk covers the
parameter, the return, the condition and the asserts (`instantiate_stamped`), and a decided
read carries its mapped value instead of cloning its operator graph. So the plrun body's
native call is never cloned per application, and its operands stay the template's cells.
The next piece is to make the body's statement nodes part of the clone: the read that returns
the body's field must clone the field's own operator when the template answer is open, not
only its value.

**Measured: the compute wrapper is not applied at run time at all.** The call `compute.plrun k a`
resolves to a **frozen module** callee (`Static(StaticFunctionRef { module: ModuleKey(1), .. })`,
seen in `check_app`), and its check-time signature apply runs; but the run-time
`static_function_apply` trace contains only `ModuleKey(0)` applies -- **none for module 1** -- so
the wrapper's body (and its native call) is never instantiated per call. The launch the dev
branch sees is the wrapper body's single template op (built while the wrapper was checked),
whose parameter cells the checker's copies fill per call; the strict version leaves them the
template's cells. Per SPEC §Function the launch must be the *applied* body -- one clone per
call -- which means the compute wrapper's run-time apply path has to work, or the launch has to
move to the call site.

**Measured: the run-time apply never runs for the compute wrapper.** The four Apply
evaluations whose callee is an empty cell sit in the prelude functions' own blocks (9, 13, 15,
22); the root-level `blurred`/`replicated` calls never reach the callee evaluation, and the
static-apply trace has no `ModuleKey(1)` application at all. So `static_function_apply` is
never entered for the compute module: the wrapper body is never instantiated per call at run
time, and the dev branch's launches are made at check time, with the checker's copies wiring
each call into the single template op.

**Corrected: the traces above used a stale package cache.** The shipping cache
(`~/.lichen/compilers/…/registry`) held a compute prelude compiled by an older binary;
`check_native_call` never fired, so the prelude was not re-checked. With a fresh `LICHEN_HOME`
the prelude **is** checked (`NATIVECALL op=plrun`, sixteen native calls; the `$plrun` op is
built in the prelude's function 21), and the launch still does not run: no `PLRUN eval`, no
module-1 static apply. So the stale cache does not explain the blocker, but it does mean the
earlier "the prelude is not checked" readings were cache artefacts, not the architecture.

**Corrected: the clone needs the function reference, not a materialized value.** The apply
already carries the callee expression (operand 3) and resolves `Dynamic`/`Static` from it
(`apply_callee_type`), so "materialize the function value" was the wrong requirement. The
question is why the wrapper's application is not exercised at run time at all: the launch's
op stays the prelude body's single template op, and the checker's copies are what fill its
cells per call on dev.

**The package cache is shared across branches: measure with a per-branch `LICHEN_HOME`.**
`~/.lichen/compilers/<slot key>/registry` is one cache for every binary and branch, so a run
from one branch leaves a prelude the next run loads without re-checking. Measured: with the
shipping cache the strict branch prints no `NATIVECALL op=plrun`, i.e. it runs a prelude
compiled by an older binary; with a fresh `LICHEN_HOME` the prelude is re-checked. Every
measurement in this note, and the examples survey, must therefore be taken with a fresh or
per-branch `LICHEN_HOME` (`target/lichen-home-<branch>`); a CLI run and a suite (in-memory
store) are otherwise measuring different preludes.

**Baseline with fresh caches (CLI, `run examples/convolution2d.lichen`).** Dev (the checker's
copies) fails `expected ?, found ?` at `102:79` — the collide. Strict (no copies) checks and
prints `(raw none, raw none)` with **no `PLRUN eval`**. With `DBG_PL` the running launch node
is printed (`module.current_operator_node()`): on dev the launch runs at **five distinct op
nodes** (33k–45k), not the prelude's one template op — so the apply clone does produce a
per-call launch when the copies are present.

**Corrected: those Apply nodes are the prelude's own calls, not the program's.** With the
block printed in `check_app`, `NodeId(1198…)` and its siblings are in blocks 9/13/15/22 — the
prelude functions' blocks during the definition pass. The program's root-block calls resolve
to `Dynamic(FunctionId(7))`, `Static(ModuleKey(0), …)` or `None`; **no root call has a
module-1 callee**. So the earlier sentence "the compute calls' Apply nodes are never evaluated"
was about the prelude's internal calls, which are expected to stay lazy at definition time.
A second scan found no dynamic function whose node list contains an External operator, so the
`$plrun` native op (`NodeId(2456)`, built while the prelude was checked in function 21) is not
reachable through any dynamic function's scope. How the program's `compute.plrun` call reaches
the wrapper body on the host side is the open question; the next probe is the root call's
`Dynamic(FunctionId(7))` identity and whether the native op is represented in the frozen module
only.

**Measured: the imported wrapper bindings are Functions, but the call's callee expr is an
undecided field read.** The checker's `ExprKind::Static` (an imported export) materializes the
frozen `[value, type]` expr; its first item for the compute module is a
`LowValue::Function(Static(module 1, index N))` (measured: four used names bind
`StaticFunctionId(1..4)`). Two resolution paths were added on the WIP branch
(`spec/instance-v` `05fa1c42`): resolve the callee from the name's own value
(`static_function_of_callee(value_of(function))`) and through a curried application's next stage
(`static_function_return_function`, the frozen return expr's value). They make **30 module-1
callees resolve inside the prelude**, but the program's five root `compute.plrun` calls still
resolve to `None`: their callee expr's value half is a leaf with no value (an undecided field
read of the imported binding), so neither `function_apply_function` nor
`static_function_of_callee` finds the function. Resolving *that* field read is the next piece —
and it is what makes the apply, and therefore the per-call clone, happen at run time.

**Measured: the field read resolves structurally, and the apply ops run -- but the launch does
not.** Two more WIP additions (`spec/instance-v` `3fa55ccb`): a structural
`static_function_of_callee(callee_expr)` (the field-read chain names the frozen function), and a
fresh function-value leaf as the apply's callee operand when the read has no value. After them
the root calls resolve (to `Static(ModuleKey(0), 19/20/30)`) and their apply ops are evaluated
(`APPLYOP` appears for them), but `PLRUN eval` is still zero: applying those functions does not
reach the `$plrun` native op. The probe answers why: module 0's 19/20/30 have
`has_compute_op=false` (measured with `static_function_compute_operator`), so they are **not**
the wrappers. The functions that do carry the launch are module 0's 21/22/23 and module 1's
2/3 (`has_compute_op=true`), and they resolve in the prelude's own blocks but not for the
program's calls. So the field-read fallback **mis-resolves**: `static_function_of_callee`
reads a subscript (`resolve_read(callee, 0)`), while `compute.plrun` is a *name* read whose
chain is `TableGet(names, name) + base`, so it picks the wrong node.
Resolving the imported package's field by name is the next piece, and it must not go through
the subscript helper.

**Measured: the field-by-name resolution names the wrapper, and the launch is still not run.**
The new WIP path (`spec/instance-v` `8ca8493b`) resolves a `NamedField` callee through the
namespace value's own field (`resolved_field` + the export slot's value half), ordered before the
subscript helper. With it, `compute.plrun` resolves to module-0 **19** (the outer lambda) and the
outer apply to module-0 **20** (the inner body, **four foreign operators** = the launches). So
the wrapper is resolved correctly now -- the earlier "19/20/30 are not the wrappers" was the
subscript helper's mis-resolution. The remaining gap: the program's five root calls' Apply ops
are still **not evaluated** by the deep pass (the four `APPLY-NONE` cases are the prelude's own
definition-pass calls), so the cloned body and its launch never run. Forcing the annotated
statement's term (`ANN-EVAL`) evaluates it to a value but still no `PLRUN`: the call op behind
the annotation is not what the deep pass forces. That is the next probe.

**Measured: the launch call's apply runs and the static apply is entered; the clone does not
launch.** With the root
apply ids printed (`ROOTAPPLY`) and the deep pass traced, the launch call (`e=524`, apply
`30076`) *is* evaluated: `APPLYOP node=NodeId(30076)` and its callee is
`Some(Function(Static(ModuleKey(0), 20)))` -- the wrapper body with four foreign operators.
`SAPPLY-ENTER function=Static(20) node=30076` prints, so `static_function_apply` runs and clones
the body. Yet `PLRUN eval` is zero: the cloned body's native op never runs. The static walk
judges the native op's class to be outside the applying scope and **references** the template
op (whose parameter cells are open) instead of cloning it, so the clone evaluates to an open
body. Whether the node is a fresh clone or the template's ref is the next probe, and the scope
rule that answers it is SPEC §Clone's "if its class is in the applying function's scope".

**Measured: the native op is cloned, and its operand gate is not reached.** With a probe in
`static_node_apply`, the frozen native ops are **cloned** (259 `SNODE-FOREIGN` lines, fresh node
ids) -- the "referenced, not cloned" guess was wrong. One of the launch clones (`3095`) does
reach the External arm (`EXTERNAL node=3095 operand=3096 operand_value=Some(Array)`) but the
compute gate never prints for it (`GATE op=3095` absent): its `run_deferred` returns before the
item loop, i.e. the operand deep evaluation answers `None` although the operand node holds an
`Array`.

**Measured: the dispatch reaches the compute run_deferred; the item check is the blocker.** A
probe inside the compute `run_deferred` shows `RD operand=.. deep=true undecided=true` for the
compute calls: the deep walk answers `Some` (the operand array), and `node_is_undecided` is true
because an item is open. So the previous round's "deep answers None" was wrong — the item loop
is reached and returns `None` on an undecided item, exactly the earlier gate. The open item is a
cloned parameter cell whose value never resolves.

**A warning on build hygiene.** The same clean WIP tree (`8ca8493b`) answered `(raw none)` in the
round-6 run and `expected ?, found ?` at `102:79` in this round's run, both with a fresh
`LICHEN_HOME`. Branch switching shares one `target/`, so incremental artifacts from another
branch can be linked (the build note's warning). Every WIP measurement must be taken after a
`cargo clean -p` of the workspace crates or with a per-branch `CARGO_TARGET_DIR`; the strict
baseline needs a clean re-measure.

**Clean re-measure and the real blocker.** With a clean rebuild of the workspace crates the
strict tree answers `(raw none, raw none)` again: the collide in the previous round was a stale
build. The gate trace then shows the launch clones' operand items: the kernel item
(`k.native`) is an open cell. Inspecting why: the frozen wrapper's parameter `k` is *decided*
at freeze (its value is the type `K`), so per SPEC §Clone its clone is a **reference** and the
body's `k.native` reads the type `K`, not the argument kernel. But SPEC §Unify says `a = b` is
`a <: b`, so `'k = K` is an annotation: the formal's *value* should stay fresh and its *type*
be `K`. The implementation writes `K` into the parameter's **value** cell. That is the next
piece: compile `'k = K` as value-fresh plus type-`K`, so the argument fills the cloned
parameter's value and `k.native` reads the kernel.

**Measured: the formal-type branch is inert because the detection is wrong.** The WIP branch
(`spec/instance-v` `efc0f03f`) adds the branch: a direct formal unified with a type constrains its
**type** (`check_unify(actual_ty, expected_value)`), leaving the value fresh. But
`is_universe_any(expected_ty)` is **false for every direct formal** (`FORMAL-UNIFY ... is_type=false`
in the prelude), so the old value-write path still runs. The right predicate is the next fix: the
checker does not type a type expression (`K`) with the universe node, so the test must read the
expected side the way `type_denotation`/`universe_is_any` does rather than comparing
`expected_ty` to the universe class.

**Measured: the IR kind is not enough; the expected value can be a Table.** The next WIP
(`spec/instance-v` `d6cf7479`) adds a `parameter_side` flag around the lambda parameter check and
treats `Struct`/`TypeSet`/`TypeTable` expected expressions there as type annotations
(`check_unify(actual_ty, expected_value)`). It **breaks the prelude**: `cannot load package
compute.lichen: expected Struct, found Table` -- a type expression's value half is not always
the type value (a `TypeTable` yields a `Table`), so the type-vs-value distinction needs a
semantic predicate, not the IR kind. The formal-annotation branch is the right shape but the
detection is still the open question.

**Measured: even a struct-only annotation is mis-read; the value half is the name table.** The
WIP `2ffb405b` restricts the annotation detection to `ExprKind::Struct` and unifies
`check_unify(actual_ty, expr_value_half(expected_term))`. It still fails with `expected Struct,
found Table`: at unify time a struct literal's value half is its **name table**, not the struct
value. So the formal annotation cannot be read through the value half; it needs the checker's
type-denotation representation (what `check_ann` uses for `x : T`). Last known-good WIP is
`8ca8493b`; `d6cf7479`, `2ffb405b` are red checkpoints that break the compute package.

## What the diagnostic is now

`write_node_answer` still writes the operator's answer into the node's class; a class that
holds nothing takes it, and a value a unification wrote is not the operator's answer, so the
operator runs (the run stamp is the other axis). Where a different value does reach one
class, the renderer prints `expected X, found Y`, spelling the two values when neither side is
a handle. The earlier note that named a second answer, the sentence that named one, and the
`value_a`/`value_b` special case for two open sides are removed.

# The un-annotated recursive fold's type stays open

> Status: current — decided `wontfix:D18`. A recursive function whose result type must
> be known states its **parameter's shape** (the arity cannot be inferred from uses;
> `(s : tuple{_, _})` is enough, the field types then infer per call), so an open type
> without one is the documented contract rather than a defect to repair. The value is
> still right, the mechanism and the engine experiments are kept as the record, and §7
> states the annotation.
> Points at: [function-normalization](function-normalization.md) (ruling 1: a
> function's types are determined at its normalize), `crates/lichen-language/src/
> compile.rs` (the `if` desugar), `crates/lichen-highlevel/src/checker/indexing.rs`
> (`check_array_term`, `check_index`), `crates/lichen-lowlevel/src/loop_run.rs`,
> `function.rs` (the clone walk), `apply.rs`.

## 1. The symptom

A recursive fold written without an annotation loses its **named reads**:

- state a **buffer**: typed without the annotation (the seed decides it);
- state an **array**: the value survives and the type stays open (`?a`);
- state a **struct** (`XOut = struct{.y (compute.Buf Float)}`): `raw none`, no diagnostic.

Annotating the recursive function (`… : tuple{Int, XOut} -> XOut`) restores both the value
and the type. Annotating only the **parameter** is enough, and so is annotating only the
read's **result** (`(settled : XOut).y`) — each of the three turns the open type into a real
one, so the missing piece is the fold's **return type**, not the value (rounds measured this
on the three-step ring; the value itself is always the right struct).

## 2. The minimal repro (no compute), and what the loss needs

```lichen
S = struct{.y Int}
@loop f = (s => if s(0) == 0 then s(1) else f (s(0) - 1, S(.y (s(1).y + 1))))
(f (0, S(.y 3))).y
```

answers `raw none: ?a`; so does the identity fold `… else f (s(0) - 1, s(1))`. The control
is a **tuple of Ints** with the same shape (`f (0, 7)` → `raw 7: ?a`): the loss needs the
base branch to be a **parameter-derived read** whose type is a struct.

**The desugar is not the cause (measured at `dev@575ad0be`).** Writing the branch as the
desugared form by hand — `s => [f (s(0) - 1, S(.y (s(1).y + 1))), s(1)][s(0) == 0]` — answers
the very same `raw none: ?a`, with and without `@loop` (block-wide recursion), so the fold
and the `if` spelling are one program. A **tuple** container is not an alternative:
`(1, 2)[1]` is refused `expected TypeArray, found TypeTuple`, so an index read only ever
reads an array, and the branch container the desugar produces is the only legal one. What
the base arm *is* decides the outcome: a **struct literal** base
(`[f (…), S(.y 9)][s(0) == 0]`) answers `9: Int`, while a **parameter-derived read** base
(`s(1)`) answers `raw none` — the loss needs that read, not the sugar.

## 3. The mechanism

1. `if c then t else e` desugars to `Index([else, then], c)` — there is no conditional IR
   kind, and **no conditional typing rule**.
2. `[else, then]` is an **array literal**: `check_array_term` unifies every element against
   one shared `element_ty` (`DiagKind::ArrayElement`), so a heterogeneous `if` is *refused*
   — measured: `x = if 1 == 1 then 1 else "a"` answers `expected string, found Int`.
3. The `Index` read's type is that shared `element_ty` — a bare cell. For the fold both arms
   are **open** at check, so the unify succeeds; the `else` arm is the recursive application,
   so `element_ty`'s class holds the function's own return type (a cycle) and the base arm's
   derived read.
4. **The last mile**: a per-call clone of a bare cell shares the *template's* class, and the
   class's deciding member is the *template's* derived read (of the template's parameter
   type, which is never bound), because **the clone walk does not traverse class members**.
   Deep-evaluating the exit's type cell changes nothing (measured).

So the fold's silent `none` is a **clone-walk limitation on class members**. The value never
depends on it; a named read needs the container's *type* for its name table, and that type is
the open cell.

## 4. The author's ruling

*"既然是脱糖，那么 checker 就不应该有 if 这个分支，直接就都是 array 分支"* — `if` is a
desugar, so the checker must not have an `if` branch; it is the array branch. The ruling
rules out a branch-type join (the alternative that would have made the fold's type decidable
at check), and it matches what the code does.

## 5. The measured negative routes

Each was implemented, measured on the repro, and reverted:

| route | result |
|---|---|
| carry the runtime error's node and print its type | prints `TypeArray` / `Type` / `TypeTuple` — the type *chain's* kind, not the user's type |
| record the container expression's type in the checker | prints `?a` — the argument's type only reaches the call's clone, never the template node |
| make `check_index`'s element type follow the runtime subscript | breaks even the working tuple-of-Ints fold with two *"not a container"* errors |
| clone the return expr's type half explicitly, and deep-evaluate the exit's type cell | no change (the class still answers the template's read) |

## 6. The fix, decided

**The verdict, after the measurements: `wontfix:D18` — the parameter's shape is the
annotation the contract requires.** The route-A experiments below stay as the record:
they show the clone walk *can* carry a cloned cell's class members and the topology then
becomes right, but the decider has nothing to read, and the only thing that made the
repro answer was an evaluation that hangs the deep budget's test. Meanwhile the arity is
what the un-annotated form lacks and cannot infer (`g = s => s(1)` prints `?a -> ?b`),
while `(s : tuple{_, _})` is enough for the field types to infer (§7), so the engine is
left as is.

**The first attempt, and what it measured.** Implemented as stated: a helper over the
class's member list, a per-pass `classes` set on the apply context (a class is carried
once), and — on the clone path — the other members cloned after the clone is
registered, so the pass's `regroup_clones` puts their clones in the clone's class.
Measured on the repro: **no change** (`raw none: ?a`), and the working controls are
unchanged (annotated fold `3: Int`, struct-literal base `7: Int`, un-annotated gcd
`raw 6: ?a`, annotated gcd `6: Int`, the tuple control `7: Int`).

A probe of the walk explains why: **every bare cell the walk clones arrives with a
singleton class** (`size = 1`), and no `LowValue::Any` clone has a class larger than
one. So the membership this note describes — the shared element cell's class holding
the base arm's derived read — is **not present in the module the apply runs on**, even
though it is what the checker's own unification built. The next step is therefore not
in the walk's carry logic but in where the topology lives: find whether the checker's
classes survive into the run module for this cell, or whether the deciding edge reaches
the return type through a *reference* rather than a class (a shared cell referenced in
place, which the walk correctly leaves alone).

**The second reading (dev@725024a6).** Instrumenting both sides, the repro's two `.y`
reads' container-type nodes come from the checker (`check_named_field`): `95v1` and
`235v1`. `95v1` is a type node **inside the template** — every call walks it and clones
it (`enter → clone`), so it is not shared in place. `235v1`, the outer read's container
type — the cell the final `.y` actually reads — is **never walked** by the clone pass at
all, and it is the cell that answers `none`. The class sizes in the walk are real: of
384 clones, 352 arrive at size 1, 20 at 2, 6 at 3 and 6 at 9; every in-place node with a
class larger than one already carries a **value** (`op=false value=true`), so
referencing it in place is correct. The read chain therefore reaches an undecided cell
that no clone pass supplies: the outer apply's result cell is bound by the runtime
`wire_apply_result` to the fold's cloned return expr, and that expr's type is undecided
because its decider (the base-arm read, whose type sits in the shared element cell's
class) is rebuilt only from members the walk reaches. The next step is to instrument
`wire_apply_result` for the outer apply and print the return expr's type clone and its
class, so the missing link is measured rather than inferred.

**The third reading (dev@8ba354a6), and it names the class.** Instrumenting
`apply_type`'s bind: the outer apply binds `cell=235v1` (the read's container type) to
`codomain=366v1` (the applied expr's type item), and the unify forms the class
`{356v1, 366v1, 235v1}` — **every member is a bare cell with no operation and no
value** (`op=false value=false` for all three). All six *body* applies in the same run
bind with `value=true` on both sides; only the outer apply's codomain arrives open. So
the fold's return-type clone (`366v1`) is a bare cell whose template decider — the
`if`'s shared element cell — never joined it, exactly as the singleton-class reading
said. The class cannot be decided by anything inside it: what is missing is the
**boundary** between the fold's return type and the `if`'s element cell, not a value
some member already holds. The next reading is the template node whose clone is
`366v1` (the fold's return expr's type item) and that node's class at check time.

**The fourth reading: the class has an operation, and it stays open.** Instrumenting
`instantiate_stamped` for the return expr: the fold's return expr is `210v1`, its type
item `41v1`, and **before the walk** `41v1`'s class is `{155v1 (bare, open), 41v1 (bare,
open), 174v1 (op=true, open)}` — all in block 2, function 1. So the class is *not*
memberless: `174v1` is the derived read that should decide it, and it never gets a value.
The body's own applies tell the contrast: their return type item `12v1` starts at class
size 3 and grows 3→8 across the six applies with `value=true` — the class members pile up
*and* the cell is decided there.

That narrows the remaining question to the clone of an operation member: with the carry
from the first attempt in place, is `174v1` cloned (present in the pass's `remap`) and is
its clone evaluated? The next probe answers that and tells whether the fix is
"carry the members" (already written) or "carry **and evaluate** them at the apply".

**The fifth reading: the carry reaches the operation, and evaluating it fixes the repro
— but breaks the deep budget.** With the carry in place, `174v1` and `155v1` **are**
cloned (`377v1`/`375v1`), and the clones arrive open, so the missing half is indeed the
evaluation. Adding it — after the bind, evaluate the codomain's class members whose
operand is decided — makes the repro answer **`3: Int`**, the annotated and
struct-literal controls unchanged, and the un-annotated `gcd` improve from
`raw 6: ?a` to **`6: Int`**. But it also makes
`recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget` **hang** (killed
at 300 s; the deep budget never fires), and gating it to shallow passes only
(`deep_depth == 0`) makes the repro regress to `raw none: ?a` — the fix fires **in the
deep pass**, which is exactly where that test's budget discipline lives. So route A's
second half is not "evaluate the members": it must decide the class **without
re-entering the deep pass** — either by a re-entrancy guard on the class (a class being
decided is skipped), or by deciding only the *shown* member's shallow evaluation, with
the deep budget's accounting untouched. Both are owner-visible choices about the deep
pass, and they are the next step; the code above was reverted, so the repro still
answers `raw none` at the tree's tip.

The repro above is the whole test surface: `S`-state fold answers `raw none` today and must
answer `3` once the type is decided; the tuple-of-Ints control must keep answering `7`; the
struct-literal base (`@loop f = (n => if n == 0 then S(.y 7) else f (n - 1))`) must keep
working.

**The sixth reading: the topology is right, and the decider still cannot answer.** With
the carry in place the class is exactly the one the note wants — at the outer bind,
`codomain=376v1` is in one class with `377v1` (the clone of the decider `174v1`) and
`375v1` (the clone of `155v1`), and `instantiation.node_of` reports those very clones.
The engine already has the re-entrancy guard this note was about to add —
`evaluation.rs`'s `evaluate_class_first`, with `evaluating_classes` holding each class
while its members are forced — so the member walk is not missing. What is missing is
that the **decider has nothing to read**: `174v1`'s clone is the derived read, and its
operand is the *type apply's* parameter clone, which is never bound (the type apply runs
`instantiate(..., check_parameter = false)`, and it is the value apply — a different
clone pass — that binds its own parameter). Three attempts measured and reverted:
running `apply_parameter_check` in the type apply, binding only the parameter's **type**
half there, and preferring the value apply's codomain (`expr_type_item(node)`) — the repro
stays `raw none: ?a` in all three.

So the remaining choice is structural, and it is the owner's:

- **A. The read chain reads the apply node's own expr.** `check_app` sets the
  application's type to a fresh cell bound by the type apply; the value apply's expr —
  the node's own value — is already decided. A lazy `Index(value_apply, 1)` would read
  it. Risk: a *chained* apply's callee type (`(f x) y`) becomes a read rather than a
  decided cell, which touches the force path P1-49's refusal relies on.
- **B. The type apply binds the value apply's codomain** (checker/runtime channel
  needed: `apply_type` does not know the value apply's `applied` expr).
- **C. Accept the annotation as the contract** for struct-state folds and say so in the
  spec (the compute kernels already annotate; the workaround section below is that
  rule).

## 7. The workaround, and its minimal form

Annotate the recursive function's **parameter** or its **result type**, or annotate the read's
result. The compute kernels' folds are annotated for this reason.

**The minimal annotation is the arity alone, and it works (measured at `dev@575ad0be`).**
The parameter's *shape* is what the un-annotated form lacks, and the field types then follow
from the application:

| program | answer |
|---|---|
| `@loop f = ((s : tuple{_, _}) => …)` — the repro with the desugared array | `3: Int` |
| the same with `if` | `3: Int` |
| `(s : tuple{Int, _})` / `(s : tuple{Int, S})` | `3: Int` |
| `g = (s : tuple{_, _}) => s(1)` ; `g (0, 7)` | `7: Int` |
| `g = s => s(1)` ; `g` | `?a -> ?b` — no arity, no field type |
| `g = (s : tuple{_, _}) => s(1)` ; `g` | `tuple{raw[?a, ?b], raw[?c, ?d]} -> raw[?c, ?d]` — the arity is fixed, the field types are open until the call |

So the arity **cannot** be inferred from uses (`?a -> ?b` above) while the **field types
can**: with the arity annotated they are decided per call. The defect this note records is
therefore only the *un-annotated shape*; an annotation of `tuple{_, _}` is enough, and the
type system already does the inference the annotation leaves open.
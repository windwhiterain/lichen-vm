# The un-annotated recursive fold's type stays open

> Status: current — a measured defect with its handover. The value is right and the
> **type** is open, so a named read on the fold's result answers `none`; the author
> ruled that `if` has no typing rule of its own, and the remaining fix is an
> engine-side clone-walk change (the owner's call). The workaround is an annotation.
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
| clone the return pair's type half explicitly, and deep-evaluate the exit's type cell | no change (the class still answers the template's read) |

## 6. The fix, decided

**The owner's verdict: route A — the clone walk carries a cloned cell's class members.**
(The alternative, the apply deciding the fold's return cell from the argument, was
rejected as the narrower special case.) The clone walk must, per call, clone the
members that decide a cloned cell's class, so the fold's return cell is decided by the
clone's own derived read instead of the template's. A fix must keep the working cases
unchanged and must not create a fourth unify site
([function-normalization](function-normalization.md) ruling 6).

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
`wire_apply_result` to the fold's cloned return pair, and that pair's type is undecided
because its decider (the base-arm read, whose type sits in the shared element cell's
class) is rebuilt only from members the walk reaches. The next step is to instrument
`wire_apply_result` for the outer apply and print the return pair's type clone and its
class, so the missing link is measured rather than inferred.

**The third reading (dev@8ba354a6), and it names the class.** Instrumenting
`apply_type`'s bind: the outer apply binds `cell=235v1` (the read's container type) to
`codomain=366v1` (the applied pair's type item), and the unify forms the class
`{356v1, 366v1, 235v1}` — **every member is a bare cell with no operation and no
value** (`op=false value=false` for all three). All six *body* applies in the same run
bind with `value=true` on both sides; only the outer apply's codomain arrives open. So
the fold's return-type clone (`366v1`) is a bare cell whose template decider — the
`if`'s shared element cell — never joined it, exactly as the singleton-class reading
said. The class cannot be decided by anything inside it: what is missing is the
**boundary** between the fold's return type and the `if`'s element cell, not a value
some member already holds. The next reading is the template node whose clone is
`366v1` (the fold's return pair's type item) and that node's class at check time.

The repro above is the whole test surface: `S`-state fold answers `raw none` today and must
answer `3` once the type is decided; the tuple-of-Ints control must keep answering `7`; the
struct-literal base (`@loop f = (n => if n == 0 then S(.y 7) else f (n - 1))`) must keep
working.

## 7. The workaround

Annotate the recursive function's **parameter** or its **result type**, or annotate the read's
result. The compute kernels' folds are annotated for this reason.
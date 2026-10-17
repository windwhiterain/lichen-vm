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

## 6. The open fix (the owner's call)

The clone walk needs to carry a cloned cell's **class members** (the operations that decide
it) per call, or the apply needs to decide the fold's return cell from the argument. Either is
an engine change; a fix must keep the working cases unchanged and must not create a fourth
unify site ([function-normalization](function-normalization.md) ruling 6).

The repro above is the whole test surface: `S`-state fold answers `raw none` today and must
answer `3` once the type is decided; the tuple-of-Ints control must keep answering `7`; the
struct-literal base (`@loop f = (n => if n == 0 then S(.y 7) else f (n - 1))`) must keep
working.

## 7. The workaround

Annotate the recursive function's **parameter** or its **result type**, or annotate the read's
result. The compute kernels' folds are annotated for this reason.
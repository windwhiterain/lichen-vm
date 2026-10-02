# Deferred unification does not recognise every form of a type value

> Status: **current — the defect is open.** Reproduced on `26d9ee4`.
> §4 is measured; §5 names the one link I could **not** isolate, and nothing in
> this note should be read as a cause for it. The line numbers are that
> revision's.
>
> Companions: [checker-encoding-instability](checker-encoding-instability.md)
> (the same "structural guess about an open encoding" weakness, at a different
> site), [shape.rs](../../crates/lichen-highlevel/src/shape.rs)'s own
> *Known weakness* notes.

## 1. The symptom

A partially applied type function, instantiated at one field:

```lichen
P = ins => struct<.I type_of ins.I>
y = (P _)(.I Int)
```

`y` builds as `struct<.I ?a>` — and **`?a` is never bound**. `?a` is not the
unknown `?`: [`TypePrinter::class_name`](../../crates/lichen-render/src/render/type_printer.rs)
(`:84`) names an *unbound cell's class*, so two cells of one class share a name
and the type is a genuinely undecided, bindable one. `P _`'s field type and the
argument's type were merged and nothing was written.

Substituting what goes in the slot changes the outcome, and this is the whole
discriminator:

| slot holds | outcome |
|---|---|
| `Int` | builds, `struct<.I ?a>` — field type **undecided** |
| `array<Int, 3>` | builds, `struct<.I ?a>` — field type **undecided** |
| `(Int, Int)` | builds, `struct<.I ?a>` — field type **undecided** |
| `struct<.x _>` | **`error: expected [?a], found [TypeStruct]`** |
| `struct<.x Int>` | **`error: expected [?a], found [TypeStruct]`** |

The struct rows fail whether the struct type is concrete or carries a
placeholder, and inlined (`(P _)(.I struct<.x Int>)`) as well as named. So the
axis is the *kind* of type value in the slot, not its contents and not how it is
spelled.

Each of these is a complete program; `P` is fixed and the slot is the only
thing that varies.

## 2. Why none of the five should be an error

Three facts, each from the code's own documentation:

- **`?a` is bindable.** It is an unbound cell's class name
  (`type_printer.rs:84`), not [`LowShape::Unknown`](../../crates/lichen-lowlevel/src/lib.rs)
  — the printer reserves the bare `?` for "no class to name".
- **Arrays unify elementwise.** `equality.rs:585` recurses per element when both
  sides are arrays of equal length, so `[?a]` against `[Type]` is a plain bind
  of `?a`, not an atomic comparison
  ([`unify_inner`](../../crates/lichen-lowlevel/src/equality.rs), `:451`).
- **The deferral exists for exactly this shape.** `equality.rs:552` consults
  `P::defer_pending` when a side is a *pending* read; and
  [`defer_pending`](../../crates/lichen-highlevel/src/shape.rs) (`:359`) is
  documented as the policy for "a pending field/positional **read** with a class
  that **holds a type**", on the grounds that *"the read resolves to its field's
  actual type once the container binds"*.

So the intended reading of the struct rows is a deferral, and of the other three
a bind. Neither happens: the struct rows error, and the other three merge
without binding.

## 3. The code path that decides

1. `type_of ins.I` is a read of an **unbound** container (`ins` is the `_`), so
   its class is not a pure cell: `equality.rs:479`'s `class_is_pure_cell`
   declines and `:494`'s block runs instead of a bind.
2. The read cannot be forced and is not a resolvable `Index`, so
   `resolved_a`/`resolved_b` fail (`:502`, `:506`) and control reaches `:510`.
3. The all-unbound-skeleton merge (`:519`) and the both-pending-reads merge
   (`:564`) do not apply, so `P::defer_pending` decides (`:552`).
4. [`class_holds_type`](../../crates/lichen-highlevel/src/shape.rs) (`:327`)
   answers *"does this class hold a type"* by a **structural guess**: take
   `kind_of(rep)` — which only succeeds if `rep` is a two-element array (`:248`)
   — then require the kind's universe slot to be self-referential.
5. The universe `K = [Type, ↺]` **is** a two-element self-referential array, so
   that guess happens to be true for it; a bare atomic marker makes `kind_of`
   return `None` and the guess false.

This is the same class of weakness the file already admits for
`is_struct_marker_any` (`shape.rs:518`):

> **Known weakness** (see the module docs): this is a structural guess about an
> open encoding — any 2-element array in a marker slot passes, with no
> `TypeStruct` tag checked. Phase 1 names it; Phase 4 may replace it with an
> honest tag check.

`class_holds_type` carries no such note, and it is load-bearing for a
*correctness* decision rather than a classification one.

## 4. The two failures are different, and the silent one is worse

- **The struct rows record a spurious error.** A round-trip the deferral
  exists to permit is refused, and the user sees `expected [?a], found
  [TypeStruct]` for a program that has no type error in it.
- **The other three merge without binding.** The build succeeds and the field
  type stays `?a` forever. Nothing later can recover it, and no diagnostic
  points at it — this is the *silent* failure mode the project treats as the
  more serious of the two.

They are two symptoms of one site, and a fix has to address both: making the
deferral fire for the struct rows alone would leave the silent half in place.

## 5. What is not isolated

**Why `array<Int, 3>` and `(Int, Int)` behave like `Int` rather than like
`struct<…>` is unexplained.**

An earlier version of this section argued from two *printed* type names, and
that argument is **withdrawn**. [type-rendering-inconsistent](type-rendering-inconsistent.md)
shows a type value's spelling depends on its form: in one and the same
diagnostic slot, `struct<.a Int>`, `array<Int, 3>` and `Int -> Int` print the
bare tags `TypeStruct`, `TypeArray` and `TypeFunction`, while `(Int, Int)` prints
the structured `<Type, Type>`. A printed name is therefore not an identity test,
and the two measurements that used to stand here settled nothing:

- `type_of` over the four kinds printed `<Type, Type, TypeTuple, Type>`; and
- the failing comparison printed `TypeStruct` where `type_of struct<.x Int>`
  printed `Type`.

Neither says which node the unifier actually compared. The axis — the build
outcome splitting by the form in the slot — is real and reproducible; the
mechanism behind it is not known. Pinning it needs instrumentation: a probe
inside `class_holds_type` printing the representative's value, `kind_of`'s
answer, and the `is_self_referential` verdict, for each of the five rows of §1.
A targeted unit test over `unify` would do as well. Neither is written.
**Do not build a fix on §3 alone, and do not let a printed type name decide any
of it.**

## 6. Scope

Reachable from plain lichen with no compute involvement, so it is a checker
defect and not a consequence of any `lichen-compute` work. It is independent of
[the compute kernel-parameter refactor](lichen-compute.md) — that refactor's
`P` only meets this path if it keeps a `type_of` over an unbound container's
field; taking the field's *value* instead (`struct<.I ins.I, …>`) does not
traverse it.

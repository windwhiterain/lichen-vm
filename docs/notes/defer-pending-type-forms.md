# Deferred unification does not recognise every form of a type value

> Status: **current — the defect is open.** §1 is a minimal repro plus a
> measured matrix, reproduced on `74bcfab` (first seen on `26d9ee4`); §5 names
> the links I could **not** isolate, and nothing in this note should be read as
> a cause for them. Line numbers are `74bcfab`'s.
>
> Companions: [type-rendering-inconsistent](type-rendering-inconsistent.md)
> (why no printed type name can decide any of this),
> [checker-encoding-instability](checker-encoding-instability.md) (the same
> "structural guess about an open encoding" weakness, at a different site), and
> [shape.rs](../../crates/lichen-highlevel/src/shape.rs)'s own *Known weakness*
> notes.

## 1. The minimal repro

Two field reads of one unbound placeholder, in one type expression:

```lichen
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
```

```
error: expected [[?a, ?b], [?c, ?d]], found TypeStruct
```

**Two reads is the minimum.** One read does not error at all — it merges and
leaves the field type undecided:

```lichen
P = ins => struct<.I ins.x>
y = (P _)(.I Int)          -- builds, struct<.I [?a, ?b]>
```

`?a` is not the unknown `?`:
[`TypePrinter::class_name`](../../crates/lichen-render/src/render/type_printer.rs)
(`:84`) names an *unbound cell's class*, so two cells of one class share a name
and the type is a genuinely undecided, bindable one. The read and the argument's
type were merged and **nothing was written**.

### The axis is the number of reads, not the number of fields

A second read of the same placeholder is what turns the silent merge into a
recorded error; fields that are not reads change nothing:

| reads | fields | outcome |
|---|---|---|
| 1 | 1 | builds, `struct<.I [?a, ?b]>` — **undecided** |
| 1 | 2 | builds, `struct<.I [?a, ?b], .O Int>` |
| 1 | 3 | builds |
| 2 | 2 | **`expected [[?a, ?b], [?c, ?d]], found TypeStruct`** |
| 2 | 3 | **`expected [[?a, ?b], [?c, ?d], Int], found [Type, Type, Type]`** |
| 3 | 3 | **`expected [[?a, ?b], [?c, ?d], [?e, ?f]], found [Type, Type, Type]`** |

All three `found` sides are the same kind of argument, and two of them are
spelled differently from the third — that is
[type-rendering-inconsistent](type-rendering-inconsistent.md)'s subject, not
this note's.

### …and the argument's kind is a second one

Holding the reads and varying what the slot receives gives a complete 2×4
matrix — every cell below is measured:

| slot holds | 1 read | 2 reads |
|---|---|---|
| `Int` | builds, `struct<.I [?a, ?b]>` | **error** |
| `array<Int, 3>` | builds, `struct<.I [?a, ?b]>` | builds, `struct<.I [?a, ?b], .O [?c, ?d]>` |
| `(Int, Type)` | builds, `struct<.I [?a, ?b]>` | builds, `struct<.I [?a, ?b], .O [?c, ?d]>` |
| `struct<.a Int>` | **error** | **error** |

Two things fall out of it. The declared field type is a `[value, type]` **pair**
where the type position holds a field read — the error shows `[?a, ?b]` per
field — which is why a pair- or array-shaped argument is accepted at either
arity while an atomic one is accepted only at one read. And a struct type value
is refused at every arity, which is the row §5 cannot yet account for.

Every row is a complete program; `P` and the placeholder are fixed and the
table's two columns are the only things that vary.

## 2. Why none of these should be an error

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

So the intended reading of §1's rows is: a deferral wherever the argument is a
struct type value, and a bind everywhere else. Neither happens — every row in
which the argument is a struct type fails, every two-read row fails, and every
one-read row merges without binding.

## 3. The code path that decides

1. The field read in §1's `P` is a read of an **unbound** container (`ins` is
   the `_`), so its class is not a pure cell: `equality.rs:479`'s
   `class_is_pure_cell` declines and `:494`'s block runs instead of a bind.
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

§1's matrix splits the rows into two outcomes, and they are two symptoms of one
site:

- **Rows that record a diagnostic.** Every row whose argument is a struct type
  value, and every row with two or more reads, prints `expected …, found …` for
  a program that has no type error a reader could act on.
- **Rows that merge without binding.** One read with an atomic, array or pair
  argument: the build succeeds and the field type stays a pair of undecided
  cells (`[?a, ?b]`) forever. Nothing later recovers it, and no diagnostic
  points at it.

The silent half is the worse one — a program that compiles while carrying an
undecided field type where a caller is about to read a type. And the boundary
between the halves is a **count of reads**: one merges, two error. No reader
could anticipate that distinction, and it is not one a diagnostic should be
drawing either.

A fix has to address both. Making the deferral fire for the struct-argument rows
alone leaves the silent half standing; making the two-read rows merge like the
one-read rows leaves the undecided type standing.

## 5. What is not isolated

**Why the outcome turns on the *number of reads* is unexplained.** One read
merges silently and two record an error, from the same site, over the same
operand kinds, differing only in how many reads the field-type list contains.
Nothing in §3's path accounts for a count deciding which way the unify goes.

**Why a struct type value is refused at one read, where an atomic, array or pair
argument is not, is unexplained as well.**

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

Neither says which node the unifier actually compared. Pinning either link needs
instrumentation: a probe inside `class_holds_type` printing the representative's
value, `kind_of`'s answer, and the `is_self_referential` verdict, for each cell
of §1's matrix. A targeted unit test over `unify` would do as well. Neither is
written. **Do not build a fix on §3 alone, and do not let a printed type name
decide any of it.**

## 6. Scope

Reachable from plain lichen with no compute involvement, so it is a checker
defect and not a consequence of any `lichen-compute` work. It is not, however,
avoidable from the compute side by choosing one spelling over another: a
partially applied `(P _)(…)` whose type positions hold **field reads of an
unbound container** meets this path whether the read is spelled `ins.I` or
`type_of ins.I` — both are reads, and §1's two-read rows fail either way. What
the spelling decides is *which* of §4's two halves the row lands in.

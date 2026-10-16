# A type value's rendering depends on its form

> Status: **closed — the written type is unreachable where the diagnostic renders** (§6). §1–§2 are measured; §3 names what is not
> traced, and nothing in this note should be read as a cause for the rows it
> cannot explain.
>
> Companions: [raw-rendering-mark](raw-rendering-mark.md), which marks the
> fallback step 6 below without touching the tag/structured split this note is
> about, and [function-type-merge](function-type-merge.md), which owns how a
> struct type's nominal identity is built and printed.

## 1. The symptom

Four analogous type values, each passed as the argument to a function whose
parameter is `Int`, so each is the *found* side of the same diagnostic:

```lichen
f = (x : Int) => x
```

| argument | what the message prints as the found side |
|---|---|
| `struct{.a Int}` | `TypeStruct` |
| `array{Int, 3}` | `TypeArray` |
| `Int -> Int` | `Int -> Int` — **fixed since this table was written** |
| `(Int, Int)` | `tuple{Type, Type}` |

Re-measured on `dev@16e50e6b`, with `f = (x : Int) => x` and one call per row:

```text
f (struct{.a Int})   =>  error: expected Int, found TypeStruct
f (array{Int, 3})    =>  error: expected Int, found TypeArray
f (Int -> Int)       =>  error: expected Int, found Int -> Int
f ((Int, Int))       =>  error: expected Int, found tuple{Type, Type}
```

So the function case now prints what was written (it was `TypeFunction` in the table below), and the other
three still do not: two print their marker tag and the tuple prints its elements' kinds. The standard the
section states — *none prints the value that was written* — is now the whole of the open defect.

**And the tuple's spelling is pinned by examples, not accidental.** Two programs declare it as their output:
`examples/struct_generic.lichen` states `(struct{.f Int}, struct{.f Type}, struct{.f Int}): tuple{TypeStruct,
TypeStruct, TypeStruct}` and `examples/type_of.lichen` states `(Int, Type, Int, array{Int, 2}, Type, 5):
tuple{Type, Type, Type, TypeArray, Type, Int}`. So of the two vocabularies the section contrasts, the
structured one is what the tuple already uses and what those programs expect, and the remaining question is
whether a *type value* on a diagnostic's found side should print as the type that was written (`struct{.a
Int}`, `array{Int, 3}`) -- which the function case now does -- rather than as its kind's marker. That choice
changes those two declared outputs, so it is a ruling rather than a repair.
Three print their **kind's marker tag**; the tuple prints a **structured form of
its element types**. None prints the value that was written, and two values of
the same standing — two compound type expressions — print in two different
vocabularies.

## 2. The paths, and where they diverge

[`TypePrinter::elements`](../../crates/lichen-render/src/render/type_printer.rs)
is a cascade, and each branch spells a node differently:

1. `is_struct_kind(node)` → the literal `"TypeStruct"`;
2. `[head, K]` with a self-looping universe tail → the head, through
   `type_constant`, which spells the markers `Type`, `TypeStruct`, `TypeArray`,
   `TypeFunction`, `TypeTuple` and `TypeId(n)`;
3. a full struct type `[shape, [[payload, TypeStruct], K]]` → `struct{…}`,
   optionally `#n`;
4. `[shape, [marker, K]]` → `in -> out` / `tuple{T…}` / `array{T, len}`;
5. a checker-registered arrow shape;
6. **fallback: the raw elements**, spelled `raw[a, b]` — the raw mark.

So a tag, a structured form and a raw form are all reachable from the same
printer, and step 6 is the cascade admitting it may recognise nothing at all.

The tuple is the row that reaches step 4. `fields_any` renders a shape
elementwise, and each shape element is a `[value, type]` pair — whose rendering
yields the pair's **type** half. That is why the shape of `tuple{Int, Int}` prints
`Type` per field rather than `Int`.

## 3. What is not traced

**Which step fires for each of the four rows is not traced.** Step 1 is
documented as matching a struct *kind* "as opposed to a struct type term
`[shape, kind]`, whose kind slot is such a node", so a full `struct{.a Int}`
should not match it and should reach step 3 — yet it printed the tag. Two
readings fit, and this note does not choose between them:

- the printer misclassifies a struct type term as a struct kind; or
- the diagnostic reports a **different node** than the argument — the kind's
  marker rather than the type term — so the cascade is handed something else and
  the inconsistency is in the *diagnostic's choice of node*, not in the printer.

Distinguishing them needs instrumentation or a unit test over the printer.
Neither is written.

## 4. Why it matters

A type diagnostic is how a user reads a type error. Two type values of the same
standing printing as `TypeStruct` and `tuple{Type, Type}` means the message cannot be
used to decide *what was compared*. A raw fallback is reachable in the same slot,
but it is spelled `raw[a, b]`, so it no longer reads as a value list where a type
is meant; what a reader still cannot separate is a **marker tag** from a
**structured form** — the split §1 measures.

## 5. What is not this

The merging question. Whether two classes unify, and which pending reads are
deferred, is decided in `lichen-lowlevel` and is
[eval-before-unify](eval-before-unify.md)'s subject. This note is about
*spelling*: the same underlying structure rendering in more than one vocabulary.
A fix for one is not a fix for the other.


## 6. Why this closes without a code change

The maintainer's answer to §1 was to unify the spelling on *the type that was written*, and to update the two
examples that pin the tuple's vocabulary. Measuring where that would be implemented shows it is out of reach
at the point the diagnostic renders, so the item closes as recorded rather than as repaired.

The found side is the apply's `argument_type` (`apply.rs`'s `apply_parameter_check` records it in
`ApplyError`, and the `Runtime` arm of `render.rs` prints it), which is the **type slot** of the argument's
pair. Re-measured on `dev@dd13f417`, one call per row:

| argument | argument pair | type slot | printed |
|---|---|---|---|
| `struct{.a Int}` | `[n29, n36]` | `n36` = `[[payload, TypeStruct], ↺]` | `TypeStruct` |
| `array{Int, 3}` | `[n30, n31]` | `n31`, kind-shaped | `TypeArray` |
| `(Int, Int)` | `[n29, n32]` | `n32`, kind-shaped | `tuple{Type, Type}` |
| `Int -> Int` | `[n32, n34]` | `n34` = the written arrow term | `Int -> Int` |

Only the function keeps the written term in its type slot, which is why it is already right. The struct's slot
is the bare struct kind: its payload carries the nominal id, a `Table` slot and the field *names*, not the
fields' written types. Feeding that payload through the printer's own struct path yields
`struct{TypeId(16), Table, raw["a"]}` instead of `struct{.a Int}`, and the array's and tuple's slots hold kind
markers rather than shapes.

So printing the written type would mean changing what a compound type value's type slot holds — a change to
the value representation, not to the renderer — and that is not what this note owns.
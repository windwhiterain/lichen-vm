# Deferred unification does not recognise every form of a type value

> Status: **fixed** on `feature/defer-pending-type-forms`. Every row of §1's
> matrix now builds with a **decided** field type (`struct<.I Type>`,
> `TypeArray`, `TypeStruct`, …); nothing errors and nothing stays `?a`.
> §1 keeps the repro; §2 records the mechanism as instrumentation isolated it
> (the earlier reading, kept in git history, had named `class_holds_type`'s
> structural guess as the decider — it was not); §3 lists what the fix
> changed.
>
> Companion: [type-rendering-inconsistent](type-rendering-inconsistent.md)
> (a separate, still-open spelling defect — no printed type name decided any
> of this either), and [applied-struct-nominal-id](applied-struct-nominal-id.md)
> (the third defect found the same way; fixed).
>
> **Open remainder in this machinery**: a *call-result* type read is still not
> usable as a type expression the way the removed builtin's raw `Index` read
> was — a library `type_of` in a struct field's declared type resolves to the
> enclosing struct kind.  Reproducer, mechanism and what was already ruled out:
> [`type-of-in-std` § *Open defect*](type-of-in-std.md#open-defect-a-library-read-is-not-the-builtin-read-in-a-field-type-position).

## 1. The repro

Two field reads of one unbound placeholder, in one type expression:

```lichen
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
```

Before the fix this errored (`expected [[?a, ?b], [?c, ?d]], found
TypeStruct`), a one-read variant built but left the field type undecided
(`struct<.I [?a, ?b]>` — merged and **nothing written**), and a struct type
value in the slot errored at every arity. The measured matrix (reads ×
argument kind) is in git history; every cell of it now builds, and the field
types print decided:

| slot holds | 1 read | 2 reads |
|---|---|---|
| `Int` | `struct<.I Type>` | `struct<.I Type, .O Type>` |
| `array<Int, 3>` | `struct<.I TypeArray>` | builds, decided |
| `(Int, Type)` | builds, decided | builds, decided |
| `struct<.a Int>` | `struct<.I TypeStruct>` | builds, decided |

The `type_of ins.I` spelling (a partially applied type function instantiated
at one field) took the same path and is fixed the same way.

## 2. The mechanism, as isolated

Instrumentation in `unify_inner`'s stall branches (a `LICHEN_PROBE` build,
since removed) showed three links, none of them the one the earlier reading
had guessed:

1. **The pending side is not always an `Index`.** `type_of` is an ordinary
   function (`x => {t = _; x: t; t}`, `lichen-std/_.lichen`), so
   `type_of ins.I` compiles to a lazy **`Apply`**, which the deferral's
   `pending_index_read` guard never matched. The bare `ins.x` spelling does
   produce a pending `Index` read.
   Both spellings stalled at the same place: the struct construction unifies
   the declared field type against the argument field's type.
2. **The silent half was an unsound skeleton merge.**
   `class_is_skeleton`/`value_is_skeleton` (lowlevel `equality.rs`) read
   values through the [`LowValue`] projection, so a node holding a **concrete
   extension atom** — any kind marker (`Type`, `TypeTuple`, `ArrayType`) —
   looked identical to an unbound cell. Type-level structures built purely
   from markers (`K = [Type, ↺]`, `[int, K]`, the tuple/array kinds) therefore
   counted as "all-unbound skeletons", and the pending read merged onto a
   class that held concrete content — with `add_equality` writing nothing, so
   the field type read as `?a` forever.
3. **The error half was that same predicate, correct by accident.** A struct
   type value's kind `[TypeStruct{id, names}, K]` contains a
   `LowValue::Table` (the names table; `Void` when anonymous) — a genuine
   structural value, correctly judged non-skeleton. With the skeleton merge
   unavailable and the deferral guard unmatched, the unify recorded the
   spurious `expected [?a], found [TypeStruct]` error. `class_holds_type`'s
   structural guess was load-bearing only at this point, and guessed wrong at
   descent depth (a bare struct marker `[id, names]` is not a 2-element
   `[shape, kind]`).

## 3. The fix

- **The skeleton predicates judge "no concrete value" on the value slot, not
  the projection** (`equality.rs`): a node whose value is an extension atom is
  decided content the lowlevel cannot read, never an empty position. This
  removes the unsound silent merge everywhere, not just in this repro.
- **`class_holds_type` is an honest tag recogniser now** (`shape.rs`): atoms
  are tested against the kind-marker registry (`ValueType::is_kind_marker`,
  itself registry-derived) and `TypeId`, a struct marker is recognised by its
  id-and-names shape, and the universe by its self-referential cycle — no
  arity guesses. The marker set is **open**: extension leaves declare their
  own type constants through `LeafKindMarkers` (compute's
  `TypeBuffer`/`TypeWrite`), and the composed vocabulary's
  `ValueType::is_kind_marker` consults it (`lang_compose_vocabulary!`).
- **The deferral gate covers reads and calls, and a Merge verdict pins.**
  `PendingSide` gained `pending_apply`; `shape::defer_pending` accepts a
  pending `Index` or a pending `Apply` against a class that holds a type. On
  a `Merge` verdict the lowlevel commits the other side's decided value onto
  the class's pending operations and representative (`pin_committed_value`),
  so the field type is decided now — the operation keeps its operand edge,
  and the apply's clone machinery drops the cached value and recomputes
  against the real argument (the deferred check surfacing there, the same
  reconcile `force_pending` performs).
- **Evaluation tolerates the self-read a pin can surface** (`evaluation.rs`):
  an `Index` whose element is its own class (a lazy type slot that reads
  itself) answers from the class's committed value, or stays lazy, instead of
  re-entering the visiting reader and tripping the cycle guard.

## 4. What changed downstream

One existing test's expectation encoded the silent half:
`a_gpu_program_chains_two_kernels_on_a_device` (lichen-language's compute
tests) expected the collected buffer's element type undecided
(`array<?d, ?e>`); it now decides, correctly, to `array<Int, ?d>`.

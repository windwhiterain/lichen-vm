# The raw mark in the printer

> Status: **current.** Points at: `crates/lichen-render`
> (`render/type_printer.rs`, `render/value_printer.rs`).
>
> Companions: [type-rendering-inconsistent](type-rendering-inconsistent.md) (the
> tag-against-structured split, which this mark does **not** fix),
> [defer-pending-type-forms](defer-pending-type-forms.md) (whose diagnostics are
> where the mark shows).

A printer that cannot read a node as a **form** falls back to the node's *raw
layout*. Before the mark that fallback was spelled exactly like a form the
printer *did* recognise, so a reader could not tell `6` — a cell the type chain
never explained — from the `6` of `6: Int`, nor `Int` from the `Int` of
`Int: Type`. The two spellings sat behind entirely different structures. The
fallback is now spelled with the mark, and the mark is **unconditional**: the
fallback has no type chain behind it, so nothing it reads is a form.

```
expected Int, found raw 6          -- an undetermined type: a dump
6: Int                             -- an `Int` type: read through the chain
raw[raw 6, raw 14]: ?a              -- an untyped call's two leaves
raw Kernel                         -- a kernel slot the struct type never named
```

## 1. The mark's spelling follows what it marks

There is one rule, and the two spellings are the same rule applied to two
kinds of reading:

- a **list** reading starts with the list's own bracket, so the mark fuses
  with it: `raw[…]`, and the cells are dumps in their own right —
  `raw[raw 6, raw 14]`;
- an **atomic** reading has no bracket, so the mark prefixes it:
  `raw 6`, `raw Int`, `raw "s"`, `raw none`, `raw Function`.

The mark **nests**, one per reading: each node the fallback gives up on
carries its own, so the outer list *and* each cell inside it are marked. That
is what the type printer's own fallback already did
(`raw[raw[?a, ?b], raw[?c, ?d]]`), so the two printers now agree.

A cycle inside a dump is cut with `…`, the type printer's own cycle spelling.
It is what keeps a self-referential kind (`[Type, ↺]`, the universe) from
unrolling forever — the previous `[head, K]` collapse cut that cycle by
*guessing* the head instead, which is the guessing §3 removes.

## 2. Where the mark appears

| Site | What it dumps |
|---|---|
| `TypePrinter::elements`' last branch | a type node no form recognises: the `[value, type]` cells, directly |
| `TypePrinter::static_elements`' last branch | the same for a frozen module's static refs |
| `ValuePrinter::raw_any` | a runtime value the type chain named **no class** for: a list `raw[…]`, an atomic `raw x` |

```lichen
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
```

```
error: expected raw[raw[?a, ?b], raw[?c, ?d]], found TypeStruct
```

The same branch is reached from a struct field — a kernel binding's `.native`
artifact renders `struct<.native raw[?a, ?b], .sig Int -> Int>` — and from the
value side: `compute.call` on a tuple-codomain kernel types its result as a
fresh cell, so the result prints `raw[raw 6, raw 14]: ?a`. A bare `[in, out]`
shape on the CLI output path is a third entry, since that path carries no
checker arrow registry to recognise it as an arrow.

## 3. What is not the fallback

Two readings look like the fallback's but are **standard forms**, and carry no
mark. Naming them is what makes the mark mean something: it says the printer
*dumped*, so anything unmarked was read.

- An **atomic type read as its head** (`[head, K]` → `head`): the kind's tag
  (`TypeStruct`, `TypeArray`, `TypeFunction`) or an atomic type constant
  (`Int`). In the **type** printer this is not a give-up — `[Int, ↺]` *is* the
  encoding of the type `Int` — so marking it would re-spell every `Int`/`Type`
  in every message. See [type-rendering-inconsistent](type-rendering-inconsistent.md)
  §1, whose open defect is exactly that a tag and a structured form cannot be
  told apart. This note is a distinguishability fix for the fallback, not that
  fix.

  That reading has to **read through a cross-module ref**. A kind read out of a
  frozen module is a *replica*: its two items are refs into the module that
  wrote it, so its tail names **that** module's canonical `[Type, ↺]` rather than
  the replica itself. Requiring the tail to be the very node — the rule until this was
  measured — made every such pair fall back to `raw[…]`: a package's
  `double = x => x + x` printed `(43, 44): <raw[Int, Type], raw[Int, Type]>` where
  the same program printed `<Int, Int>` before the operator's contract was written
  in lichen. The head check is what keeps "is the universe" apart from "contains
  the universe" ([universe-containment](universe-containment.md) §2); the tail only
  has to *be* the universe, so it is read through the ref.

  The **value** printer has no such branch, and this is the deliberate
  difference. There the type chain decides how a value reads, so a `[head, K]`
  reached through the fallback is a cell nobody explained; reading it as its
  head would be a guess, and the guess also collided with the honest dump of a
  bare marker cell (both would have printed `raw Int`). It is dumped as the
  cells it is: `raw[raw[raw Int, raw[raw Type, …]], raw[raw Type, …]]`.

- A **value whose type named a leaf class** — `ValuePrinter::leaf_class`: a
  scalar of an atomic type (`[Int, ↺]`, `[string, ↺]`), or a function of a
  function type (`[Function(fid), ↺]`). These read by their own spelling —
  `1`, `"s"`, `Function` — because the chain *named the class*. §4 is why they
  needed a branch of their own.

- A type constant the vocabulary has **no name for** reads `?`, not `raw ?`:
  the chain read it as a type constant (`ValuePrinter::atomic`), so the answer is
  the printer's own "this vocabulary cannot say" — marking it would claim the
  opposite.

## 4. Why the value printer had to grow a branch

The fallback was not only a fallback: it was also how a *scalar* rendered. An
`Int`-typed value read `1` not through a branch of the cascade but by falling
off its end into the raw layout, which happened to spell `1` correctly. Marking
the fallback therefore marked `1` too, and `examples/array.lichen` printed
`[raw 1, raw 2, raw 3]: array<Int, 3>`.

`leaf_class` is what separates the two: the cascade now reads every value whose
type names a class *before* the fallback is reached, so what is left at the
bottom is a value the chain named nothing for — and only that is a dump. A
value a class does not hold (a list under an atomic class) is a mismatch, and
falls through to the dump like any other.

## 5. What the mark buys

A reader can tell which parts of a rendered type or value the printer **read**
and which it merely **dumped**. Two things follow that a reader could not do
before: a bare `6` and an `Int` can no longer pass for a read value and a read
type, and the *reason* a value prints the way it does is legible from the
spelling itself — a `?a` type explains `raw 6`, and no type does.

It is also what makes the two defect notes above legible: in
[defer-pending-type-forms](defer-pending-type-forms.md) §1's matrix every
`[?a, ?b]` is a raw reading, and a diagnostic whose whole expected side is raw
means no form was recognised at all.

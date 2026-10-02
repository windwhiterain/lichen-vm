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
printer *did* recognise, so a reader could not tell `[?a, ?b]` — two cells the
type chain never explained — from a list the printer had understood. The
fallback is now spelled `raw<[…]>`.

## 1. Where the mark appears

| Site | What it dumps |
|---|---|
| `TypePrinter::elements`' last branch | a type node no form recognises: the `[value, type]` cells, directly |
| `TypePrinter::static_elements`' last branch | the same for a frozen module's static refs |
| `ValuePrinter::raw_any`'s array arm | a runtime value with no type to read it against |

```lichen
P = ins => struct<.I ins.x, .O ins.y>
y = (P _)(.I Int, .O Int)
```

```
error: expected raw<[raw<[?a, ?b]>, raw<[?c, ?d]>]>, found TypeStruct
```

The mark nests: each node the printer gives up on carries its own `raw<…>`, so
the outer list *and* each pair inside it are marked. The same branch is reached
from a struct field — a kernel binding's `.native` artifact renders
`struct<.native raw<[?a, ?b]>, .sig Int -> Int>` — and from the value side:
`compute.call` on a tuple-codomain kernel types its result as a fresh cell, so
the result prints `raw<[6, 14]>: ?a`. A bare `[in, out]` shape on the CLI output
path is a third entry, since that path carries no checker arrow registry to
recognise it as an arrow.

## 2. What is deliberately not marked

- A **scalar** reading (`5`, `"s"`, `1.5`): the raw layout of a scalar is its
  own spelling, so `5` is not ambiguous and `raw<5>` would say nothing.
- A **type pair read as its head** (`[head, K]` → `head`): the kind's tag
  (`TypeStruct`, `TypeArray`, `TypeFunction`) or an atomic type constant
  (`Int`). That reading is part of the encoding's surface rather than a
  give-up, and marking it would re-spell every `Int`/`Type` in every message —
  see [type-rendering-inconsistent](type-rendering-inconsistent.md) §1, whose
  open defect is exactly that a tag and a structured form cannot be told apart.
  This note is a distinguishability fix for the fallback, not that fix.

## 3. What the mark buys

A reader can now tell which parts of a rendered type or value the printer
**read** and which it merely **dumped**, which is what makes the two defect
notes above legible: in
[defer-pending-type-forms](defer-pending-type-forms.md) §1's matrix every
`[?a, ?b]` is a raw reading, and a diagnostic whose whole expected side is raw
means no form was recognised at all.

# Appendix C: The Documentation Map

The book teaches; four other bodies of documentation answer different
questions. Knowing which to read saves time.

## The book (this)

Read the book to **learn the language**: one concept at a time, each with
a complete program and the compiler's real output. Start at
[Getting Started](ch01-getting-started.md) and read the parts in order.

## The language spec

[`docs/language-spec.md`](../../language-spec.md) is the **precise
contract**: the grammar, the desugarings, the name-resolution rules, the
diagnostic vocabulary, and the semantic decisions. When the book says
"roughly" and you need "exactly", this is the document. It is the single
source of truth for the source language, and the rest of the documentation
refers to it for syntax rather than restating it.

## The feature notes

[`docs/notes/`](../../notes/) is the **record of decisions and
measurements**. Each note takes one concern — a feature, a defect, a
measurement, a design — and states what was decided, what was rejected,
and what was measured. They are written for someone changing the system
rather than someone using it, and they are where the surprising
behaviours are explained (and sometimes where they are blamed).

A few entry points:

| question | note |
|---|---|
| how do values, types and unification work underneath? | [lowlevel VM](../../notes/lowlevel-vm.md) |
| why does `@gc` collect when it does, and what is safe? | [the `@gc` trigger](../../notes/gc-trigger-design.md) |
| why is a perspective a divisibility lattice? | [attributes](../../notes/attributes.md) |
| how do the operators' classes work? | [operator polymorphism](../../notes/operator-polymorphism.md) |
| what does the compute layer guarantee, and where are its walls? | [the operator ladder](../../notes/llm-operators.md) |
| how are the examples checked? | [README sync](../../notes/readme-sync.md) |
| what is the live audit queue? | [code audit](../../notes/code-audit.md) |

## The examples

[`examples/`](../../../examples) is the **living specification**: programs
that compile, run, and declare their output, checked by the suite
(Chapter 16). Where a note explains a decision, an example shows the
language doing it. The README's Examples section is generated from the
files' own headers, so it is always current.

## The code

The crates' `lib.rs` and module documentation is the **authority for the
implementation**. A note points at the modules it is about; the rustdoc
is what a change must agree with. The
[architecture overview](../../notes/overview.md) is the map of the
layering, and Chapter 18 lists the crates.

## Which one first?

- Learning a feature: **the book**, then the spec's section for it.
- A behaviour that surprises you: the **note** whose subject it is
  (search `docs/notes/` for the word), then the code.
- Writing a program against a contract: **the spec** and the
  **examples**.
- Changing the implementation: the **code**, the **note**, and the audit
  queue, in that order.

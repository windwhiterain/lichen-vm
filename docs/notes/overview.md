# Architecture overview

> Status: current
> Points at: [README](../../README.md) (entry) and each crate's `lib.rs` rustdoc.

lichen-vm is a **lazy, unification-based type-checking infrastructure where a type
is just a value.** The whole system rests on one idea, and everything else falls out
of it.

## The core idea: `[value, type]`

Every expression evaluates to a **pair of two graph nodes**: the value and its type.
Because the pair is *just data*, the checker and the runtime are the same thing —
unifying is a runtime operation, so **checking happens while building**. Types are
first-class values (`Int`, `Type`, function/tuple/array types) living in the pair's
type slot; `Type : Type` holds in a single universe, so there is no separate kind
system — a "kinding" mistake is an ordinary type error. This is why the README says
"the runtime *is* the typechecker."

## Layering

The workspace is four crates, one per layer:

```
lichen-language   frontend: text → highlevel IR → checked program
      │  produces the IR the highlevel checker consumes
      ▼
lichen-highlevel  IR, checker, and the attribute extension point
      │  builds lowlevel modules
      ▼
lichen-lowlevel   the VM: values, lazy evaluation, unification, block GC
      ◆
lichen-utils      shared extension plumbing (enum_ext!, disjoint, compose)
```

- **`lichen-utils`** — small generic tools. `extend` provides the `enum_ext!` macro
  that lets a downstream layer compose its own operator/attribute union over a base
  enum. This is the mechanism behind "macro-based enum extension": plug in your own
  typechecker without touching the core.
- **`lichen-lowlevel`** — the interpreter: lazy evaluation, block-level GC, first-class
  functions/closures, recursion, normalization, unification, and a shared `Registry`
  of static modules. Values and types are one runtime kind.
- **`lichen-highlevel`** — builds a lowlevel `Module` from an expression tree (`IR`) in
  a single checking-and-building pass, and hosts the **attribute extension point**
  (`attr`) — the language's first genuinely static idea.
- **`lichen-language`** — a small real source language (see the spec) that compiles to
  the highlevel IR, with a package store and a CLI (`lichen`).

## Where to look

- The `[value, type]` machinery and how checking *is* unification:
  [lowlevel-vm](lowlevel-vm.md).
- Static modules and the shared registry: [static-modules](static-modules.md).
- The schema / attribute extension (perspective): [attributes](attributes.md).
- The compiler-plugin model (how a native package extends the vocabularies):
  [compiler-plugin](compiler-plugin.md) — with [lichen-compute](lichen-compute.md) as the
  worked example.
- The source language and the `@{…@}` preprocessor block:
  [the spec](../language-spec.md) and [packages](packages.md).

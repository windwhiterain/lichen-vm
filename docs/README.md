# lichen-vm documentation

This folder is the project's **documentation map**. lichen-vm keeps its
documentation in exactly two places, on purpose:

- **In-code Rust doc comments** (`//!` and `///`) — the authoritative, always-current
  description of what each module and item does. They live next to the code they
  describe, so they cannot drift.
- **This folder** — human/agent-oriented *feature notes* plus the **language spec**.
  A note explains what a feature is, why it exists, and how to use it, then points at
  the modules that implement it. It never repeats the implementation detail the
  rustdoc already carries.

> **Rule: one doc per concern.** Each feature has exactly one note here, and each fact
> is stated in exactly one place — the note, the spec, or the rustdoc. If you are
> about to repeat a fact, *link to it* instead. A note's source of truth is the code;
> read the note, then read the modules it names.

## Read order

1. [README](../README.md) — what lichen is, the crate map, quick start.
2. [Architecture overview](notes/overview.md) — the core idea (`[value, type]`
   pairs, `Type : Type`) and the layering.
3. The feature note for the crate you are working in.

## Status legend

Every note opens with a `> Status:` line:

- `current` — describes shipped behaviour (matches today's code).
- `historical` — a past decision or investigation, kept for context; never the
  authority on current behaviour.

## Feature notes

| Note | Crate(s) | Status |
|---|---|---|
| [Architecture overview](notes/overview.md) | — | current |
| [Lowlevel VM](notes/lowlevel-vm.md) | `lichen-lowlevel` | current |
| [Static modules & registry](notes/static-modules.md) | `lichen-lowlevel`, `lichen-language` (`persist`) | current |
| [Optional static shape](notes/lichen-lowlevel-shape.md) | `lichen-lowlevel` (`LowShape`), `lichen-language` (`compute`/`persist`) | current |
| [Extensible attributes](notes/attributes.md) | `lichen-highlevel` (`attr`/`ir`/`checker`), `lichen-language` (`program`) | current |
| [Packages & import](notes/packages.md) | `lichen-language` (`preprocess`/`package`/`persist`/`run`) | current |
| [Build performance](notes/build-performance.md) | `lichen-language` (`parse`) | historical |

## The language spec

[language-spec.md](language-spec.md) is the single source of truth for the lichen
**source language**: syntax, grammar, semantics, name resolution, and diagnostics.
It is the one document from the project's early days that is retained as the language
reference, because it describes a stable, user-facing contract. Feature notes and
rustdoc refer to it for syntax and never restate it.

## Reference

- The attribute extension is *inspired by* a paper — see
  [Typed Perspectives](reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).

# LichenVM

*A lazy, unification-based programming language and type-checker VM — where a type is just a value.*

[English](README.md) · [简体中文](README.zh-CN.md)

LichenVM is a minimal programming language built around one idea: **type checking and evaluation are the same process**. A program compiles to a graph of nodes; each node's value and its type are both computed by the same interpreter, and structural unification is the only type-checking rule. `Type : Type` holds in a single universe — there is no separate type system to learn, because types are ordinary values.

The project doubles as *infrastructure for custom type checkers*: the VM is a generic interpreter where the checker's runtime behavior (unification, per-application instantiation) is the language runtime.

## Highlights

- **Type = value.** There is no type/value distinction. `Int`, `Type`, `Int -> Int`, `Int<3>` are all ordinary values you can bind, pass around, and put in a tuple.
- **Automatic let-polymorphism.** Every lambda is polymorphic for free: each application instantiates the parameter with fresh cells, so one binder works at `Int` and at `Type` in the same program. No generalize/instantiate special form exists or is needed.
- **Dependent array types, lazily.** The length of `Int<n>` is an arbitrary expression. A bound length is resolved lazily and pinned at check time — `((n => ([1, 2, 3] : Int<n>)) 3)` checks, and applying any other length fails.
- **First-class types.** Type instantiation happens through partial function application.
- **A real frontend.** A hand-written lexer, parser, name resolver, and IR emitter with proper diagnostics (spans + carets) at every stage — including statements, bindings, and indexing.
- **A lean VM.** Lazy evaluation, block-level garbage collection, minimal allocation.

## Quick start

Requires a Rust toolchain.

```bash
cargo build
cargo test
```

Run every example program (one `file: output` line each):

```bash
cargo run -p language -- crates/language/examples/programs
```

Run a single program:

```bash
cargo run -p language -- crates/language/examples/programs/bindings.lang
# 1
```

## A taste

```text
5 : Int                             -- 5
(x => x) 5 : Int                    -- 5
a = [1, 2]; b = 0; a[b]             -- statements: 1
((id => ((id 5 : Int), (id Type : Type))) (x => x)) : <Int, Type>
                                    -- let-polymorphism: [5, Type]
((n => ([1, 2, 3] : Int<n>)) 3) : Int<3>
                                    -- dependent array length: [1, 2, 3]
([1, 2, 3])[1]                      -- indexing: 2
(i => [10, 20][i]) 1 : Int          -- indexing as a branch: 20
```

## How it works

A program is one expression. Every expression compiles to a **pair `[value, type]`**, and type spines bottom out at the canonical universe `K = [Type, ↺]` (`Type : Type` via a self-loop). The checker is an interpreter: it compiles the IR, runs a definition pass so apply-time type checks fire, and evaluates the root — the program's value.

- **Unification is the check.** `Module::unify` is a structural, lazy unification. A pure unbound cell binds; a class with an unevaluated computation is a pending computation that must resolve before anything binds over it.
- **Application is the binding.** `f x` unifies a fresh clone of `f`'s parameter with `x` — this is what makes every lambda let-polymorphic, and it is why the runtime *is* the type checker.
- **Statements are graph sharing.** `a = e; …` compiles `e` once and pre-resolves every use of `a` to that node — the IR stays a plain expression graph, with no `let` node.
- **Block-level GC.** Nodes live in blocks; evaluation compacts only the return-reachable tree and releases vacated blocks.

## Crates

| crate | role |
|---|---|
| [`lichen-lowlevel`](crates/lichen-lowlevel) | The VM: nodes, blocks, lazy evaluation, structural unification, garbage collection. |
| [`lichen-highlevel`](crates/lichen-highlevel) | The IR and checker: every expression is a `[value, type]` pair; checking and runtime are one process. |
| [`language`](crates/language) | The frontend: source text → IR with diagnostics, plus a CLI and example programs. |
| [`lichen-utils`](crates/lichen-utils) | Shared utilities. |

## Documentation

- [`docs/language.md`](docs/language.md) — the language spec (syntax, semantics, diagnostics)
- [`docs/highlevel.md`](docs/highlevel.md) — the checker design
- [`docs/hm-loc.md`](docs/hm-loc.md) — a beginner's explainer of HM-loc, the inference approach
- [`README.old.md`](README.old.md) — the original design notes

## Status

v1 is a complete pipeline — text → lex → parse → resolve → IR → check → run — with diagnostics at every stage and 190+ tests. Not yet: arithmetic and conditionals, recursion, per-application dependent checking, parameter annotations, error recovery, JIT compilation.

## Design philosophy

- Minimal memory usage, minimal allocation.
- High speed for trivial programs (complex ones JIT-compiled in the future).

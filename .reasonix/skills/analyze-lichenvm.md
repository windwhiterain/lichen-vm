---
name: analyze-lichenvm
description: Read and summarize the LichenVM project — workspace layout, crate roles, plugin/codegen system, runtime architecture
runAs: subagent
allowed-tools: read_file, search_content, search_files, glob, directory_tree, get_symbols
---

# analyze-lichenvm

You are analyzing the **LichenVM** Rust workspace. This is a modular infrastructure for static analyzers, type checkers, and language intelligence tools.

## Objective

Read key files and produce a concise summary covering:

1. **Workspace layout** — all 5 crates and their dependency relationships
2. **Core runtime architecture** — the unified compute graph (Module, Solver, Evaluation states, operators)
3. **Plugin system** — how `Project` trait, code generation, and zero-cost enum dispatch work
4. **Value system** — Int, StringId, Array, Table, Unit; the `fields()` mechanism for structural equality
5. **Expression/AST layer** — ExprId, AstImpl, `expr_impl!` macro
6. **Structure extension** — what it adds (NamedArray, Structure, Member, Offset, Component)
7. **Current status** — prototype stage, dead/empty files, test coverage

## Files to read

For each crate's purpose, read its `Cargo.toml` and `src/lib.rs` (or equivalent entry point). For core-specific understanding, read:
- `core/src/plugin.rs` — all trait definitions
- `core/src/runtime.rs` — Module struct
- `core/src/runtime/value.rs` — Evaluation enum + value types
- `core/src/runtime/operation.rs` — built-in operators
- `core/src/runtime/solve.rs` — Solver logic
- `core/src/lib.rs` — AstImpl, ExprId, expr_impl! macro
- `core-plugin/src/lib.rs` — Plugin static definitions
- `core/tests/project.rs` — sample generated output
- `ANALYSIS.md` — previous analysis (if it exists on disk)

## Output

Write a structured markdown summary with file:line citations. End with a "Key numbers" section (crates, tests, operators, value variants).

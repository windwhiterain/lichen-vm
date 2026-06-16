---
name: analyze-lichenvm
description: Read and summarize the LichenVM project — workspace layout, crate roles, plugin/codegen system, runtime architecture
runAs: subagent
allowed-tools: read_file, search_content, search_files, glob, directory_tree, get_symbols
---

# analyze-lichenvm

You are analyzing the **LichenVM** Rust workspace. This is a modular infrastructure for static analyzers, type checkers, and language intelligence tools — a "LLVM for static analysis" where values, types, and other expression properties are all nodes in a unified compute graph solved by constraint propagation.

## Objective

Read key files and produce a concise summary covering:

1. **Workspace layout** — all 5 crates and their dependency graph
2. **Core runtime architecture** — Module, Solver, Evaluation states, node graph, operators
3. **Plugin/codegen system** — how `Project` trait, `Plugin` static definitions, code generation via `core-plugin` / `structure-plugin` produce zero-cost enum dispatch
4. **Value system** — Int, StringId, Array, Table, Unit; the `fields()` / `for_field_pairs()` mechanism for structural equality and recursive solving
5. **Expression/AST layer** — ExprId, AstImpl, `expr_impl!` macro, multi-property expressions
6. **Structure extension crate** — NamedArray, NameSet, Structure, Member expr, Offset/Component operators
7. **Utils crate** — Arena allocator (bump with exponential chunk growth), ArenaArray, ArenaHashMap (open-addressing with quadratic probing), StableVec (bit-indexed power-of-two chunks)
8. **Current status** — test coverage (3 tests across core + structure), what's empty (`core/src/property.rs`, `core/src/runtime/switch.rs`)

## Files to read

For each crate, read `Cargo.toml` and `src/lib.rs` (or entry point). For core:
- `core/src/plugin.rs` — all trait definitions (principal_traits + plugin traits)
- `core/src/runtime.rs` — Module struct (nodes, operations, evaluations, solves, equations, arena)
- `core/src/runtime/value.rs` — Evaluation enum (Value/Ref/Auto), Array/Table/Int/Unit types
- `core/src/runtime/operation.rs` — Operation struct, built-in Sum/Index/Find, `operands!` macro
- `core/src/runtime/solve.rs` — Solver struct, solve loop, dependency tracking, equation application
- `core/src/runtime/equation.rs` — LocalEquation / Equation types
- `core/src/runtime/diagnostic.rs` — Diagnostic, EqualityError
- `core/src/lib.rs` — AstImpl, ExprId, `expr_impl!` macro, trait blanket impls
- `core-plugin/src/lib.rs` — Plugin static definition (value/operator/diagnostic/expr variants)
- `core/tests/project.rs` — generated code output (the enum dispatch machinery)
- `core/tests/runtime.rs` — runtime test (sum + equation constraint solving)
- `core/tests/ast.rs` — AST-level test (expression building + solving)
- For structure:
- `structure/src/lib.rs` — NamedArray, Structure, Member builder
- `structure/src/operator.rs` — Offset, Component operators
- `structure/src/plugin.rs` — structure plugin traits
- `structure-plugin/src/lib.rs` — structure Plugin static def
- `structure/tests/project.rs` — generated code with structure extensions
- `structure/tests/main.rs` — structure test (member access on named fields)
- For utils:
- `utils/src/lib.rs` — Arena, StableVec, bit math
- `utils/src/arena.rs` — Arena allocator
- `utils/src/arena/array.rs` — ArenaArray
- `utils/src/arena/hashmap.rs` — ArenaHashMap
- `utils/src/stable_vec.rs` — StableVec

## Output

Write a structured markdown summary with file:line citations. End with a "Key numbers" section (crates, tests, operators, value variants).

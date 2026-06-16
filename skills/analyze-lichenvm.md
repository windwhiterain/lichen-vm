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
2. **Core module organization** — flat top-level modules vs. nested runtime — how the layout makes plugin authorship clearer
3. **Core runtime architecture** — Module, Solver, Evaluation states, node graph, operators
4. **Plugin/codegen system** — how `Project` trait, `Plugin` static definitions, code generation via `core-plugin` / `structure-plugin` produce zero-cost enum dispatch
5. **Value system** — Int, StringId, Array, Table, Unit; the `fields()` / `for_field_pairs()` mechanism for structural equality and recursive solving
6. **Expression/AST layer** — ExprId, AstImpl, `expr_impl!` macro, multi-property expressions
7. **Structure extension crate** — NamedArray, NameSet, Structure, Member expr, Offset/Component operators
8. **Utils crate** — Arena allocator (bump with exponential chunk growth), ArenaArray, ArenaHashMap (open-addressing with quadratic probing), StableVec (bit-indexed power-of-two chunks)
9. **Current status** — test coverage (3 tests across core + structure), no dead files, `rustfmt` runs on generated output

## Module organization (plugin author's view)

The `core` crate has been reshaped from a deep hierarchy into 6 flat top-level modules — the entire public API is visible at a glance:

| Module | Contents | Why it matters for a plugin author |
|---|---|---|
| `core::ast` | `ExprId`, `Ast`, `AstImpl` | AST node IDs and the concrete builder — you use these to construct and inspect expression trees |
| `core::value` | `Int`, `StringId`, `Array`, `Table`, `Unit` | Built-in value types — your new value types plug into the same `fields()` / `for_field_pairs()` system |
| `core::operator` | `Sum`, `Index`, `Find`, `operands!` macro | Built-in operators your operators can delegate to — no need to reimplement lookup logic |
| `core::diagnostic_kind` | `EqualityError` | The one built-in diagnostic — your plugin can add more |
| `core::expr_impl` | `expr_impl!` macro invocations | Macro-generated glue that connects trait methods to operator dispatch — look here to see the pattern for wiring up new expression types |
| `core::plugin` | (generated) traits: `Project`, `Value`, `Operator`, `DiagnosticKind`, `Ast` | The **contract** — implements these to define a new project/plugin |

Under `core::runtime` live the engine internals that concrete plugins mostly don't touch:
- `runtime::evaluation` — `Evaluation` enum (Value/Ref/Auto)
- `runtime::operation` — `Operation` struct (operand + operator)
- `runtime::solve` — `Solver` (constraint propagation engine)
- `runtime::equation` — `LocalEquation`
- `runtime::diagnostic` — `Diagnostic` (produced during solving)

## Files to read

For each crate, read `Cargo.toml` and `src/lib.rs` (the lib.rs is now just module declarations). For core:
- `core/src/plugin.rs` — **generated** — all trait definitions (principal_traits + plugin traits). This is the plugin contract.
- `core/src/ast.rs` — `ExprId`, `Ast` trait, `AstImpl` (moved from old lib.rs)
- `core/src/operator.rs` — `operands!` macro, `Sum`, `Index`, `Find` (moved from old runtime/operation.rs)
- `core/src/value.rs` — `Int`, `StringId`, `Array`, `Table`, `Unit`, `Evaluation::AUTO` const, Module helper methods (moved from old runtime/value.rs)
- `core/src/diagnostic_kind.rs` — `EqualityError` (moved from old runtime/diagnostic.rs)
- `core/src/expr_impl.rs` — `expr_impl!` macro invocations that wire up Sum/Index/Find expression builders (moved from old lib.rs)
- `core/src/runtime.rs` — `Module` struct, Ptr, NodeId, NodeIdLocal, ModuleId, helper methods
- `core/src/runtime/evaluation.rs` — `Evaluation` enum (Value/Ref/Auto) — now its own file
- `core/src/runtime/operation.rs` — `Operation` struct (operand + operator) — **no longer contains operator implementations**
- `core/src/runtime/solve.rs` — `Solver` struct, solve loop, dependency tracking, equation application
- `core/src/runtime/equation.rs` — `LocalEquation` / `Equation`
- `core/src/runtime/diagnostic.rs` — `Diagnostic` struct (EqualityError moved to diagnostic_kind.rs)
- `core-plugin/src/lib.rs` — Plugin static definition (value/operator/diagnostic/expr variant declarations)
- `core/tests/project.rs` — generated code output (the zero-cost enum dispatch machinery)
- `core/tests/runtime.rs` — runtime test (sum + equation constraint solving)
- `core/tests/ast.rs` — AST-level test (expression building + solving)
- For structure:
- `structure/src/lib.rs` — just module declarations
- `structure/src/value.rs` — `NamedArray`, `NameSet`, `Structure` (moved from old lib.rs)
- `structure/src/operator.rs` — `Offset`, `Component` (delegates to `Find`/`Index`)
- `structure/src/expr_impl.rs` — `Member` builder (moved from old lib.rs)
- `structure/src/plugin.rs` — generated structure plugin traits
- `structure-plugin/src/lib.rs` — structure Plugin static def
- `structure/tests/project.rs` — generated code with structure extensions
- `structure/tests/main.rs` — structure test (member access on named fields)
- For utils:
- `utils/src/lib.rs` — Arena re-exports, bit math helpers
- `utils/src/arena.rs` — Arena bump allocator
- `utils/src/arena/array.rs` — ArenaArray
- `utils/src/arena/hashmap.rs` — ArenaHashMap (open-addressing with quadratic probing)
- `utils/src/stable_vec.rs` — StableVec (bit-indexed power-of-two chunks)

## Output

Write a structured markdown summary with file:line citations. End with a "Key numbers" section (crates, tests, operators, value variants).

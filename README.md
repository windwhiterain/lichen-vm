# LichenVM

*A lazy, unification-based infrastructure for type-checker — where a type is just a value.*

[English](README.md) · [简体中文](README.zh-CN.md)

## Highlevel

Built program from lowlevel from IR (intermediate representation), checking and runtime are unified.

### Features:

- Hindley-Milner System
  - Let-Polymorphism Everywhere
- Dependent Type: via laziness.
- First Class Type: type instantiation via partial function application.

### Philosophy:
- No Curry
- `Type : Type`

## Lowlevel

Interpreter that type and value are same thing.

### Features:

- Lazy Evaluation.
- Block Level Garbage Collection.
- Lmbda Calculas: first class function, higher order function, closure.
- Recursive Function Apply.
- Function Normalization.
- Unification.

### Philosophy:

- Minimal Memory Usage.
- Minimal Allocation.
- High Speed For Trivial Program (complex one will be JIT compiled in the future).

## Quick start

Requires a Rust toolchain.

```bash
cargo build
cargo test
```

Run every example program (one `file: output` line each):

```bash
cargo run -p lichen-language -- crates/lichen-language/examples/programs
```

Run a single program:

```bash
cargo run -p lichen-language -- crates/lichen-language/examples/programs/bindings.lichen
# 1
```

Install the CLI (a binary named `lichen`):

```bash
cargo install --path crates/lichen-language  # from a checkout of this repo
cargo install --git git@github.com:windwhiterain/lichen-vm.git lichen-language
```

then run it directly:

```bash
lichen crates/lichen-language/examples/programs/bindings.lichen
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
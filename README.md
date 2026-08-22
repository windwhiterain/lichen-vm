# LichenVM

*A lazy, unification-based infrastructure for type-checker — where a type is just a value.*

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

## Language

A minimal language demostrating the features.

- [spec](docs/language-spec.md)

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

## Examples

This section is generated from [`examples/programs`](crates/lichen-language/examples/programs) by `cargo run -p lichen-language --bin sync-readme` and enforced by [`tests/readme.rs`](crates/lichen-language/tests/readme.rs).

<!-- begin: examples -->

### `array.lichen`

```text
[1, 2, 3] : Int<3>
```

output:
```text
[1, 2, 3]: Int<3>
```

### `dependent.lichen`

```text
a = x => (1, Int)[x];
((a 0),(a 1))
```

output:
```text
[1, Int]: <?a, ?b>
```

### `function.lichen`

```text
f1 = x => x;
f2 = x => f1 x;
f2 1
```

output:
```text
1: Int
```

### `index.lichen`

```text
a = [1, 2];
b = (1, Int);
(a[0], a[1], b[0], b[1])
```

output:
```text
[1, 2, 1, Int]: <Int, Int, Int, Type>
```

### `placeholder.lichen`

```text
[1, 2, 3] : Int<_>
```

output:
```text
[1, 2, 3]: Int<3>
```

### `polymorphism.lichen`

```text
-- Polymorphism
--
-- `a` applies its argument to 1; `b` selects an element of `[1, 2]` by
-- index.  `a b` passes the function `b` to `a`, which applies it to 1 —
-- the result is `[1, 2][1]`, i.e. 2.  A call's result type is a lazy
-- record, so the checker derives the root type from the evaluated value:
-- an unannotated polymorphic call runs and prints its result.

a = x => x 1;
b = x => [1,2][x];
a b
```

output:
```text
2: Int
```

### `struct_instance.lichen`

```text
-- Struct instantiation
--
-- `s(1, 2)` wraps a positional tuple in the nominal type: an application
-- whose callee is a struct type compiles to the `Instantiate` expression,
-- the element types are checked against the fields, and the result has the
-- struct type.  Bind the struct type once and reuse it.
s = struct<Int, Int>; s(1, 2)
```

output:
```text
[1, 2]: struct<Int, Int>
```

### `structs.lichen`

```text
-- Nominal struct types
--
-- `struct<T1, ..., Tn>` is a *new type* with positional fields: each
-- source occurrence allocates a fresh nominal id, so two occurrences never
-- unify.  Bind one occurrence and it is reusable — here the same bound type
-- fills an array, and the element check sees a single nominal id.
s = struct<Int>; [s, s]
```

output:
```text
[[Int], [Int]]: TypeId(0)<2>
```

### `tuple.lichen`

```text
(1, Int) : <Int, Type>
```

output:
```text
[1, Int]: <Int, Type>
```

### `types.lichen`

```text
-- First-class types
--
-- Types are ordinary values: `Type : Type` (the single universe), `Int`,
-- and a function type `Int -> Int` can all sit in a tuple.

((Type : Type), Int, (x => x) : (Int -> Int))
```

output:
```text
[Type, Int, Function]: <Type, Type, Int -> Int>
```

<!-- end: examples -->
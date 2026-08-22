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

### `tuple.lichen`

```text
(1, Int) : <Int, Type>
```

output:
```text
[1, Int]: <Int, Type>
```

### `index.lichen`

```text
a = [1, 2]
b = (1, Int)
(a[0], a[1], b[0], b[1])
```

output:
```text
[1, 2, 1, Int]: <Int, Int, Int, Type>
```

### `closure.lichen`

```text
a = 1
f1 = x => {
    b = 2
    f2 = y => [a, b, x, y]
    f2
}
f1 3 4 
```

output:
```text
[1, 2, 3, 4]: Int<4>
```

### `dependent_type.lichen`

```text
a = x => (1, Int)[x]
(a 0, a 1)
```

output:
```text
[1, Int]: <Int, Type>
```

### `let_polymorphism.lichen`

```text
f = x => x
(f 1, f Int)
```

output:
```text
[1, Int]: <Int, Type>
```

### `nested_function.lichen`

```text
f1 = x => {
    f2 = y => [y, y]
    f2 x 
}
f1 1
```

output:
```text
[1, 1]: ?a
```

### `placeholder.lichen`

```text
f1 = x => x : Int
f2 = x => {
    x: <Type, _>
    f1 x[1]
}
f2
```

output:
```text
Function: <Type, [?a, ?b]> -> ?c
```

### `structs.lichen`

```text
s = struct<Int, Type>
a = s(1, Int)
(a, a[0], a[1])
```

output:
```text
[[1, Int], 1, Int]: <struct<Int, Type>, Int, Type>
```

<!-- end: examples -->
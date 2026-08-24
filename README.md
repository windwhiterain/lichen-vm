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
- Macro Based Enum Extension: impl your own typechecker with no performance concern.

### Philosophy:

- Minimal Memory Usage.
- Minimal Allocation.
- High Speed For Trivial Program (complex one will be JIT compiled in the future).

## Language

A minimal language demostrating the features.

- [spec](docs/language-spec.md)

## How To Use

- Compile your program to lichen program with:
    - Computatal operator replaced by fake operator.
    - Complex value replaced by fake value. 
- Then run lichen program

Since lichen-highlevel preserve the consistancy of `value : type` pair, as long as input `value : type` pairs are consistant, any evaluated `value : type` pair are.

Warning: `Type : Type` gives you most flexibility, the decidability of the lichen program is your integration's responsibility, e.g. encode your type-system's universes into lichen.

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
(1, Int): <Int, Type>
```

### `index.lichen`

```text
a = [1, 2]
b = (1, Int)
(a[0], a[1], b[0], b[1])
```

output:
```text
(1, 2, 1, Int): <Int, Int, Int, Type>
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
(1, Int): <Int, Type>
```

### `lazy_infinite.lichen`

```text
f = x => [x, ~ f (x + 1)]
inf = f 0
(inf[1][0], inf[1][1][0], inf[1][1][1][0])
```

output:
```text
(1, 2, 3): <Int, Int, Int>
```

### `let_polymorphism.lichen`

```text
f = x => x
(f, f 1, f Int)
```

output:
```text
(Function, 1, Int): <?a -> ?a, Int, Type>
```

### `mutual_recursion.lichen`

```text
is_even = x => [is_old (x - 1), 1][x == 0]
is_old = x => [is_even (x - 1), 0][x == 0]
(is_even 3, is_old 3)
```

output:
```text
(0, 1): <Int, Int>
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
[1, 1]: Int<2>
```

### `recursion.lichen`

```text
fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]
fib 10
```

output:
```text
55: Int
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
Function: <Type, Int> -> Int
```

### `struct.lichen`

```text
A = struct<Int, Type>
a = A(1, Int)
B = struct<Int>
b = B((1,))
(A, a, a[0], a[1], B, b, b[0])
```

output:
```text
(struct<Int, Type>, (1, Int), 1, Int, struct<Int>, (1,), 1): <TypeStruct, struct<Int, Type>, Int, Type, TypeStruct, struct<Int>, Int>
```

### `struct_recursion.lichen`

```text
A = struct<Int, B>
B = struct<Type, A>
a = A(1, b)
b = B(Int, a)
(A , B, a, b)
```

output:
```text
(struct<Int, struct<Type, struct<Int, …>>>, struct<Type, struct<Int, struct<Type, …>>>, (1, (Int, (1, …))), (Int, (1, …))): <TypeStruct, TypeStruct, struct<Int, struct<Type, struct<Int, …>>>, struct<Type, struct<Int, …>>>
```

<!-- end: examples -->
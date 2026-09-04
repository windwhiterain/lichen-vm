# LichenVM

*A lazy, unification-based infrastructure for type-checker — where a type is just a value.*

## [Highlevel](crates/lichen-highlevel/)

Build type system over untyped lowlevel from IR (intermediate representation).

### Features:

- Hindley-Milner System
  - Let-Polymorphism Everywhere
- Dependent Type: via laziness.
- First Class Type: type instantiation via partial function application.

### Philosophy:
- No Curry
- `Type : Type`

## [Lowlevel](crates/lichen-lowlevel/)

Interpreter that has no type.

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

## [Language](crates/lichen-language/)

A minimal language built over highlevel.

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
@{
  order = "0"
  output = "[1, 2, 3]: Int<3>"
@}
[1, 2, 3] : Int<3>
```

### `tuple.lichen`

```text
@{
  order = "0"
  output = "(1, Int): <Int, Type>"
@}
(1, Int) : <Int, Type>
```

### `index.lichen`

```text
@{
  order = "1"
  output = "(1, 2, 1, Int): <Int, Int, Int, Type>"
@}
a = [1, 2]
b = (1, Int)
(a[0], a[1], b(0), b(1))
```

### `closure.lichen`

```text
@{
  order = "2"
  output = "[1, 2, 3, 4]: Int<4>"
@}
a = 1
f1 = x => {
    b = 2
    f2 = y => [a, b, x, y]
    f2
}
f1 3 4 
```

### `dependent_type.lichen`

```text
@{
  order = "2"
  output = "(1, Int): <Int, Type>"
@}
a = x => (1, Int)(x)
(a 0, a 1)
```

### `lazy_infinite.lichen`

```text
@{
  order = "2"
  output = "(1, 2, 3): <Int, Int, Int>"
@}
f = x => [x, ~ f (x + 1)]
inf = f 0
(inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))
```

### `let_polymorphism.lichen`

```text
@{
  order = "2"
  output = "(Function, 1, Int): <?a -> ?a, Int, Type>"
@}
f = x => x
(f, f 1, f Int)
```

### `mutual_recursion.lichen`

```text
@{
  order = "2"
  output = "(0, 1): <Int, Int>"
@}
is_even = x => [is_old (x - 1), 1][x == 0]
is_old = x => [is_even (x - 1), 0][x == 0]
(is_even 3, is_old 3)
```

### `nested_function.lichen`

```text
@{
  order = "2"
  output = "[1, 1]: Int<2>"
@}
f1 = x => {
    f2 = y => [y, y]
    f2 x 
}
f1 1
```

### `recursion.lichen`

```text
@{
  order = "2"
  output = "55: Int"
@}
fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]
fib 10
```

### `placeholder.lichen`

```text
@{
  order = "3"
  output = "Function: <Type, Int> -> Int"
@}
f1 = x => x : Int
f2 = x => {
    x: <Type, _>
    f1 x(1)
}
f2
```

### `struct.lichen`

```text
@{
  order = "3"
  output = "(struct<Int, Type>, (1, Int), 1, Int, struct<Int>, (1,), 1): <TypeStruct, struct<Int, Type>, Int, Type, TypeStruct, struct<Int>, Int>"
@}
A = struct<Int, Type>
a = A(1, Int)
B = struct<Int>
b = B(1,)
(A, a, a(0), a(1), B, b, b(0))
```

### `struct_recursion.lichen`

```text
@{
  order = "3"
  output = "(struct<Int, struct<Type, struct<Int, …>>>, struct<Type, struct<Int, struct<Type, …>>>, (1, (Int, (1, …))), (Int, (1, …))): <TypeStruct, TypeStruct, struct<Int, struct<Type, struct<Int, …>>>, struct<Type, struct<Int, …>>>"
@}
A = struct<Int, B>
B = struct<Type, A>
a = A(1, b)
b = B(Int, a)
(A , B, a, b)
```

### `struct_generic.lichen`

```text
@{
  order = "4"
  doc = "A struct constructor is generic: `Box t` (juxtaposition — a space, not a
parenthesized argument) builds the *same* nominal type for every field
type — the `Fresh` id is per occurrence and shared, so `Box Int` and
`Box Type` differ only in their field lists (the value shape), not in
their type (the shared kind).  They therefore coexist in one homogeneous
tuple."
  output = "(struct<Int>, struct<Type>, struct<Int>): <TypeStruct, TypeStruct, TypeStruct>"
@}
Box = t => struct<t>
(Box Int, Box Type, Box Int)
```

### `table.lichen`

```text
@{
  order = "4"
  output = "(10, 20): <Int, Int>"
@}
t = table{ [1, 2] :: 10, [3, 4] :: 20 }
(t{[1, 2]}, t{[3, 4]})
```

### `import`

```text
@{
  order = "5"
  math = import "math.lichen"
  geo = import "geometry.lichen"
  output = "(42, 12): <Int, Int>"
@}
(math 41, geo 5)
```

#### `import/math.lichen`

```text
@{
  order = "0"
  output = "Function: Int -> Int"
@}
succ = x => x + 1
add = x => y => x + y
succ
```

#### `import/geometry.lichen`

```text
@{
  order = "1"
  math = import "math.lichen"
  output = "Function: Int -> Int"
@}
double = x => math x + math x
double
```

### `perspective.lichen`

```text
@{
  order = "6"
  output = "3: Int"
@}
((1 # 4) + (2 # 6)) # 2
```

### `assert.lichen`

```text
@{
  order = "7"
  output = "(1, 5): <Int, Int>"
@}
n = 5
(! (n <= 5), n)
```

### `assert_in_function.lichen`

```text
@{
  order = "8"
  output = "1: Int"
@}
f = x => ! (x <= 10)
f 5
```

### `compute_jit.lichen`

```text
@{
  order = "9"
  compute = import "compute.lichen"
  output = "8: Int"
@}
k_double = compute(0) (y => y + y)
k_outer  = compute(0) (x => compute(1) k_double (x + 1))
compute(1) k_outer 3
```

<!-- end: examples -->
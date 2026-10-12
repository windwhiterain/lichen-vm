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
- `Type : Type`: the type chain closes in a cycle at `Type` (the terminal of the chain, not a supertype) — the cycle is what admits recursive types. A function's type is the function itself (`f : f`), so its chain closes at `f` the same way ([function-type-merge](docs/notes/function-type-merge.md)).

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

Warning: lichen has no subtyping, and a compound type is typed by its kind rather than by `Type` (`array{Int, 3} : Type` does not check), so what `Type : Type` buys is that types are ordinary values and a recursive type is an ordinary cyclic one — the cost is that the decidability of a lichen program is your integration's responsibility, e.g. encode your type-system's universes into lichen.

## Quick start

Requires a Rust toolchain.

```bash
cargo build
cargo test
```

Run every example program (one `file: output` line each):

```bash
cargo run -p lichen-compiler -- examples
```

Run a single program:

```bash
cargo run -p lichen-compiler -- examples/bindings.lichen
# 1
```

Install the compiler (a binary named `lichen-compiler`, formerly `lichen`):

```bash
cargo install --path crates/lichen-compiler  # from a checkout of this repo
cargo install --git git@github.com:windwhiterain/lichen-vm.git lichen-compiler
```

then run it directly:

```bash
lichen-compiler examples/bindings.lichen
# 1
```

The package manager (a binary named `lichen`, in `crates/lichen-package`)
fetches the git dependencies a file declares in its `---…---` block
(`name = depend "url"`) into the lichen-home source cache, downloads the
`lichen-compiler` and `lichen-language-server` binaries, and rebuilds the
compiler when a native plugin is imported: see [`crates/lichen-package`](crates/lichen-package/README.md).

## Examples

This section is generated from [`examples`](examples) by `cargo run -p lichen-tools --bin sync-readme` and enforced by [`tests/readme.rs`](crates/lichen-language/tests/readme.rs).

<!-- begin: examples -->

### `array.lichen`

```text
---
  order = "0"
  output = "[1, 2, 3]: array{Int, 3}"
---
[1, 2, 3] : array{Int, 3}
```

### `tuple.lichen`

```text
---
  order = "0"
  output = "(1, Int): tuple{Int, Type}"
---
(1, Int) : tuple{Int, Type}
```

### `index.lichen`

```text
---
  order = "1"
  output = "(1, 2, 1, Int): tuple{Int, Int, Int, Type}"
---
a = [1, 2]
b = (1, Int)
(a[0], a[1], b(0), b(1))
```

### `closure.lichen`

```text
---
  order = "2"
  output = "[1, 2, 3, 4]: array{Int, 4}"
---
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
---
  order = "2"
  output = "(1, Int): tuple{Int, Type}"
---
a = x => (1, Int)(x)
(a 0, a 1)
```

### `gc.lichen`

```text
---
  order = "2"
  output = "[8, 16]: array{Int, 2}"
---
add    = x => y => x + y
double = n => @gc { a = n + 1
                   b = add a a
                   b }
@gc [double 3, double 7]
```

### `lazy_infinite.lichen`

```text
---
  order = "2"
  output = "(1, 2, 3): tuple{Int, Int, Int}"
---
f = x => [x, @shallow f (x + 1)]
inf = f 0
(inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))
```

### `let_polymorphism.lichen`

```text
---
  order = "2"
  output = "(Function, 1, Int): tuple{?a -> ?a, Int, Type}"
---
f = x => x
(f, f 1, f Int)
```

### `mutual_recursion.lichen`

```text
---
  order = "2"
  output = "(0, 1): tuple{Int, Int}"
---
is_even = x => [is_old (x - 1), 1][x == 0]
is_old = x => [is_even (x - 1), 0][x == 0]
(is_even 3, is_old 3)
```

### `nested_function.lichen`

```text
---
  order = "2"
  output = "[1, 1]: array{Int, 2}"
---
f1 = x => {
    f2 = y => [y, y]
    f2 x 
}
f1 1
```

### `recursion.lichen`

```text
---
  order = "2"
  output = "55: Int"
---
fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]
fib 10
```

### `placeholder.lichen`

```text
---
  order = "3"
  output = "Function: tuple{Type, Int} -> Int"
---
f1 = x => x : Int
f2 = x => {
    x: tuple{Type, _}
    f1 x(1)
}
f2
```

### `struct.lichen`

```text
---
  order = "3"
  output = "(struct{.x Int, .y Type}, (1, Int), 1, Int, struct{.x Int}, (1,), 1): tuple{TypeStruct, struct{.x Int, .y Type}, Int, Type, TypeStruct, struct{.x Int}, Int}"
---
A = struct{.x Int, .y Type}
a = A(.x 1, .y Int)
B = struct{.x Int}
b = B(.x 1,)
(A, a, a.x, a.y, B, b, b.x)
```

### `struct_recursion.lichen`

```text
---
  order = "3"
  output = "(struct{.f Int, .g struct{.f Type, .g struct{.f Int, .g …}}}, struct{.f Type, .g struct{.f Int, .g struct{.f Type, .g …}}}, (1, (Int, (1, …))), (Int, (1, …))): tuple{TypeStruct, TypeStruct, struct{.f Int, .g struct{.f Type, .g struct{.f Int, .g …}}}, struct{.f Type, .g struct{.f Int, .g …}}}"
---
A = struct{.f Int, .g B}
B = struct{.f Type, .g A}
a = A(1, b)
b = B(Int, a)
(A , B, a, b)
```

### `struct_generic.lichen`

```text
---
  order = "4"
  doc = "A struct constructor is generic: `Box t` (juxtaposition — a space, not a
parenthesized argument) builds the *same* nominal type for every field
type — the `Fresh` id is per occurrence and shared, so `Box Int` and
`Box Type` differ only in their field lists (the value shape), not in
their type (the shared kind).  They therefore coexist in one homogeneous
tuple."
  output = "(struct{.f Int}, struct{.f Type}, struct{.f Int}): tuple{TypeStruct, TypeStruct, TypeStruct}"
---
Box = t => struct{.f t}
(Box Int, Box Type, Box Int)
```

### `table.lichen`

```text
---
  order = "4"
  output = "(10, 20): tuple{Int, Int}"
---
t = table([1, 2] ==> 10, [3, 4] ==> 20)
(t <== [1, 2], t <== [3, 4])
```

### `import`

```text
---
  order = "5"
  math = import "math.lichen"
  geo = import "geometry.lichen"
  output = "(42, 10, 7): tuple{Int, Int, Int}"
---
(math.succ 41, geo.double 5, geo.inc_twice 5)
```

#### `import/math.lichen`

```text
---
  order = "0"
  output = "(Function, Function): struct{.succ Int -> Int, .add raw[?a, ?b] -> raw[?a, ?b] -> raw[?a, ?b]}"
---
succ = x => x + 1
add = x => y => x + y
```

#### `import/geometry.lichen`

```text
---
  order = "1"
  math = import "math.lichen"
  output = "(Function, Function): struct{.double raw[?a, ?b] -> raw[?a, ?b], .inc_twice Int -> Int}"
---
double = x => math.add x x
inc_twice = x => math.succ (math.succ x)
```

### `type_of.lichen`

```text
---
  order = "5"
  std = import "../lichen-std/_.lichen"
  output = "(Int, Type, Int, array{Int, 2}, Type, 5): tuple{Type, Type, Type, TypeArray, Type, Int}"
---
f = std.type_of
g = x => std.type_of x
n = 5 : std.type_of (1)
(std.type_of (1), f Int, g 2, std.type_of [1, 2], std.type_of Type, n)
```

### `perspective.lichen`

```text
---
  order = "6"
  output = "3 # 2: Int"
---
((1 # 4) + (2 # 6)) # 2
```

### `assert.lichen`

```text
---
  order = "7"
  output = "(1, 5): tuple{Int, Int}"
---
n = 5
(@assert (n <= 5), n)
```

### `assert_in_function.lichen`

```text
---
  order = "8"
  output = "1: Int"
---
f = x => @assert (x <= 10)
f 5
```

### `compute_jit.lichen`

```text
---
  order = "9"
  compute = import "compute.lichen"
  output = "6: Int"
---
k_double = compute.jit (y : Int => y + y)
k_outer  = compute.jit (x : Int => compute.launch k_double x)
compute.launch k_outer 3
```

### `rmsnorm.lichen`

```text
---
  order = "9"
  compute = import "compute.lichen"
  doc = "RMSNorm: y[i] = x[i] / sqrt(mean(x²) + eps) * w[i], the entry of a transformer
  block. It is two kernels because the surface wants two — a row-wise `Σ x²` reduction, then a
  per-index rescale — and the two claims are what lets the floor see the pair as one region.
  The row is `7, 3, 1, 2`, whose squares sum to `63`, so the mean is `63 / 4 = 15.75` and
  `15.75 + 0.25 = 16`: `sqrt` answers `4`, and every division below divides by it exactly."
  output = "[3.5, 3.0, 2.0, 8.0]: array{Float, 4}"
---
Row  = struct{.x (compute.Buf Float)}
Sums = struct{.s (compute.Buf Float)}
ParS = compute.P (compute.KT Float)(.I Row, .O Sums)
sumsq_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.s, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
sumsq = compute.parallel (sumsq_raw ~ "claim:Reduction(combine,identity,element)") "cpu"
Scale = struct{.x (compute.Buf Float), .w (compute.Buf Float), .s (compute.Buf Float)}
OutY  = struct{.y (compute.Buf Float)}
ParY  = compute.P (compute.KT Float)(.I Scale, .O OutY)
rescale_raw = (k : ParY) => {
  i = compute.range k.n
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  w = compute.read ((compute.Read _)(.from k.in.w, .at i))
  d = sqrt (compute.read ((compute.Read _)(.from k.in.s, .at 0)) / int2float k.n + 0.25)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((x / d) * w ~ "element")))
}
rescale = compute.parallel (rescale_raw ~ "claim:Reduction(element)") "cpu"
x = (compute.buffer ([7.0, 3.0, 1.0, 2.0] : array{Float, 4}) : (compute.Buf Float))
w = (compute.buffer ([2.0, 4.0, 8.0, 16.0] : array{Float, 4}) : (compute.Buf Float))
sums = (compute.plrun sumsq ((compute.A Row)(.n 4, .I Row(.x x))) : Sums)
out = (compute.plrun rescale ((compute.A Scale)(.n 4, .I Scale(.x x, .w w, .s sums.s))) : OutY)
compute.collect out.y : array{Float, 4}
```

### `doc.lichen`

```text
---
  order = "10"
  output = "5 # 4 ? tag = 7: Int"
---
Doc = struct{.tag Int}
five = 5 # 8 ? Doc(.tag 7)
five # 4
```

### `type_access.lichen`

```text
---
  order = "11"
  output = "(Int, string, Int, string, 1): tuple{Type, Type, Type, Type, Int}"
---
S = struct{.a Int, .b string}
s = S(.a 1, .b "h")
(tuple{Int, string}{0}, tuple{Int, string}{1}, S::a, S::b, s.a)
```

### `gcd.lichen`

```text
---
  order = "13"
  doc = "Euclid's algorithm — the shape a real algorithm has: a recursive function
whose base case is a comparison and whose step is a remainder.  The parameter is a
pair, so the recursion carries both numbers.  A function's type is the function
itself (`f : f`), and an annotation states one by binding that function's own two
cells — so `: tuple{Int, Int} -> Int` *is* the constraint rather than a check beside it,
and the self-recursive result's type follows from it (`6: Int`).  Nothing carries
the `raw` mark, because the type is decided."
  output = "6: Int"
---
gcd = (p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : tuple{Int, Int} -> Int
gcd (48, 18)
```

### `operators.lichen`

```text
---
  order = "14"
  doc = "The arithmetic, comparison and bitwise operators, the five prefix scalar
keywords, and the two precedence facts worth knowing: `* / %` bind tighter than
`+ -`, and a comparison is the loosest binary level — so `1 & 3 == 1` is
`(1 & 3) == 1`.  `& | ^` over two comparison results are the language's
`and`/`or`/`xor`, and `!=` is the generalized equality's other face, so it
compares type values too.  `max`, `min` and the three `Float`-only scalar
operators `exp`, `sqrt` and `floor` take their operands by juxtaposition and
leave a value of the operands' class, unlike a comparison."
  output = "(7, 3, 1, 3, 0, 1, 2, 1, Int, 5, 3, 2.5, 1.0, 2.0, 1.0): tuple{Int, Int, Int, Int, Int, Int, Int, Int, Type, Int, Int, Float, Float, Float, Float}"
---
(1 + 2 * 3, 7 / 2, 7 % 2, 1 | 2 ^ 3 & 1, (2 > 1) & (2 < 1), (1 < 2) == 1, 6 & 3, Int != string, Int, max 3 5, min 3 5, max 1.5 2.5, exp 0.0, sqrt 4.0, floor 1.75)
```

### `convert.lichen`

```text
---
  order = "15"
  doc = "The two class crossings. `Int` and `Float` never convert on their own —
`int2float` and `float2int` are the only words that carry a value across, and
each checks its operand against its own **source** class and yields its own
**target**. A prefix conversion binds tighter than `+`, so `int2float 1 + 2.0`
converts the `1` and not the sum. `float2int` truncates toward zero, and a float
with no `Int` to truncate toward (`NaN`, an infinity, a negative) is refused at
run time as `operator.out_of_range`."
  output = "(3.0, 3, 1, 1.0, 3.0): tuple{Float, Int, Int, Float, Float}"
---
half = int2float 1 + 2.0
(int2float 3, float2int 3.7, float2int (3.0 / 2.0), int2float (2 > 1), half)
```

### `silu.lichen`

```text
---
  order = "16"
  compute = import "compute.lichen"
  doc = "SwiGLU's gate half: `y[i] = silu(g[i]) * u[i]`, where `silu(x) = x / (1 + exp(-x))`.
  It is two kernels because the surface fuses a **pair**, and one kernel would have claimed the
  whole of `silu(g) * u` and fused with nothing. The split puts the quotient in the producer and
  leaves a plain product to the consumer, which is also where a reader looking for the operator
  looks first. `g = 0, 1, -1, 2` and `u = 3, 1, 0.5, 4`, so lane 0 is `0 / (1 + exp(0)) = 0`
  exactly and lane 1 is `exp(-1)`; `0 - 1` is written `0.0 - 1.0` because the language has no
  unary minus. The other three lanes are the language's own `exp` — a degree-10 Taylor sum over
  seven halvings, not `e` — so the declared output is that sum evaluated in `f32`, which is at
  most 3 parts in `10⁶` away from `x / (1 + e⁻ˣ)` and is not claimed to be equal to it."
  output = "[0.0, 0.73105794, -0.13447031, 7.0463777]: array{Float, 4}"
---
Gate  = struct{.g (compute.Buf Float)}
SiluA = struct{.a (compute.Buf Float)}
ParA  = compute.P (compute.KT Float)(.I Gate, .O SiluA)
silu_raw = (k : ParA) => {
  i = compute.range k.n
  g = compute.read ((compute.Read _)(.from k.in.g, .at i))
  compute.write ((compute.Write _)(.to k.out.a, .at i, .value ((g / (1.0 + exp (0.0 - g))) ~ "element")))
}
silu = compute.parallel (silu_raw ~ "claim:Elementwise(element)") "cpu"
UpIn  = struct{.a (compute.Buf Float), .u (compute.Buf Float)}
OutY  = struct{.y (compute.Buf Float)}
ParB  = compute.P (compute.KT Float)(.I UpIn, .O OutY)
up_raw = (k : ParB) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.a, .at i))
  u = compute.read ((compute.Read _)(.from k.in.u, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value ((a * u) ~ "element")))
}
up = compute.parallel (up_raw ~ "claim:Elementwise(element)") "cpu"
g = (compute.buffer ([0.0, 1.0, (0.0 - 1.0), 2.0] : array{Float, 4}) : (compute.Buf Float))
u = (compute.buffer ([3.0, 1.0, 0.5, 4.0] : array{Float, 4}) : (compute.Buf Float))
act = (compute.plrun silu ((compute.A Gate)(.n 4, .I Gate(.g g))) : SiluA)
out = (compute.plrun up ((compute.A UpIn)(.n 4, .I UpIn(.a act.a, .u u))) : OutY)
compute.collect out.y : array{Float, 4}
```

### `softmax.lichen`

```text
---
  order = "17"
  compute = import "compute.lichen"
  doc = "Row softmax: y[i] = exp(x[i] - max(x)) / sum_j exp(x[j] - max(x)), the entry of attention. It \
   is two kernels because the surface wants two — a `max` reduction over the row, then one kernel \
   that folds `sum exp` and divides by that fold on the way out — and the two claims are what lets \
   the floor see the pair as one region. The row is `1, 2, 3, 4`, so the row's maximum is `4` and \
   the exponents are `-3, -2, -1, 0`; dividing every `exp` by the common factor `exp(-3)` leaves \
   the closed form `e^k / (1 + e + e² + e³)` for `k = 0…3`, and `1 + e + e² + e³` is \
   `31.1928748505774` in exact arithmetic. The answer is **not** that closed form rounded to f32: \
   the language's `exp` is the degree-10 Taylor sum of `docs/notes/floating-point.md` §6, whose f32 \
   relative error is a few parts in 10⁶, so the declared output is the sum this arithmetic \
   computes."
  output = "[0.03205853, 0.08714417, 0.23688345, 0.6439139]: array{Float, 4}"
---
Row  = struct{.x (compute.Buf Float)}
Maxs = struct{.m (compute.Buf Float)}
ParM = compute.P (compute.KT Float)(.I Row, .O Maxs)
rowmax_raw = (k : ParM) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.m, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity)") "cpu"
Soft = struct{.x (compute.Buf Float), .m (compute.Buf Float)}
OutY = struct{.y (compute.Buf Float)}
ParY = compute.P (compute.KT Float)(.I Soft, .O OutY)
softmax_raw = (k : ParY) => {
  i = compute.range k.n
  m = compute.read ((compute.Read _)(.from k.in.m, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) - m) ~ "element")) ~ "combine"))
  total = fold (k.n, 0.0 ~ "identity")
  here = exp (compute.read ((compute.Read _)(.from k.in.x, .at i)) - m)
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (here / total)))
}
softmax = compute.parallel (softmax_raw ~ "claim:Reduction(combine,identity,element)") "cpu"
x = (compute.buffer ([1.0, 2.0, 3.0, 4.0] : array{Float, 4}) : (compute.Buf Float))
mx = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x x))) : Maxs)
out = (compute.plrun softmax ((compute.A Soft)(.n 4, .I Soft(.x x, .m mx.m))) : OutY)
compute.collect out.y : array{Float, 4}
```

<!-- end: examples -->
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
- [The lichen book](https://windwhiterain.github.io/lichen-vm/) — the language,
  taught from scratch, Rust-book style; every program in it was run against
  this compiler, and the sources live in [docs/book](docs/book/)

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
f = (x : Int) => [x, @shallow f (x + 1)]
inf = (f 0 : tuple{Int, tuple{Int, tuple{Int, tuple{Int, _}}}})
(inf(1)(0), inf(1)(1)(0), inf(1)(1)(1)(0))
```

### `let_polymorphism.lichen`

```text
---
  order = "2"
  output = "(?a => ?a, 1, Int): tuple{?a -> ?a, Int, Type}"
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
  output = "tuple{Type, Int} => Int: tuple{Type, Int} -> Int"
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
  output = "(Int => Int, raw[?a, ?b] => raw[?a, ?b] -> raw[?a, ?b]): struct{.succ Int -> Int, .add raw[?a, ?b] -> raw[?a, ?b] -> raw[?a, ?b]}"
---
succ = x => x + 1
add = x => y => x + y
```

#### `import/geometry.lichen`

```text
---
  order = "1"
  math = import "math.lichen"
  output = "(raw[?a, ?b] => raw[?a, ?b], Int => Int): struct{.double raw[?a, ?b] -> raw[?a, ?b], .inc_twice Int -> Int}"
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
  array-to-buffer conversion (`docs/notes/kernel-buffer-declarations.md`).
  The row is `7, 3, 1, 2`, whose squares sum to `63`, so the mean is `63 / 4 = 15.75` and
  `15.75 + 0.25 = 16`: `sqrt` answers `4`, and every division below divides by it exactly. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
  conversion (`docs/notes/kernel-buffer-declarations.md`). The fold's trip count is the launch's extent read as `k.n + 0`: the extent's own node cannot carry a loop's count (`docs/notes/loop-conversion.md`), and the arithmetic leaves it the same number."
  output = "[3.5, 3.0, 2.0, 8.0]: array{Float, 4}"
---
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Sums = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS = struct{.n Int, .in Row}
sumsq_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine" ~ "element"))
  b_s = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Sums)(.s (compute.write ((compute.Write _)(.to b_s, .at i, .value fold (k.n + 0, 0.0 ~ "identity"))))))
}
sumsq = compute.parallel (sumsq_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Sums
Scale = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY  = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParY  = struct{.n Int, .in Scale}
rescale_raw = (k : ParY) => {
  i = compute.range k.n
  x = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  w = (compute.read ((compute.Read _)(.from k.in.w, .at i)) : Float)
  d = sqrt ((compute.read ((compute.Read _)(.from k.in.s, .at 0)) : Float) / int2float k.n + 0.25)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value ((x / d) * w ~ "element"))))))
}
rescale = compute.parallel (rescale_raw ~ "claim:Reduction(element)") "cpu" OutY
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 7.0 else 0.0) + (if i == 1 then 3.0 else 0.0) + (if i == 2 then 1.0 else 0.0) + (if i == 3 then 2.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 4)) : xBuf).x
wBuf = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_w_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 2.0 else 0.0) + (if i == 1 then 4.0 else 0.0) + (if i == 2 then 8.0 else 0.0) + (if i == 3 then 16.0 else 0.0)
  ((wBuf)(.w (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_w = compute.parallel fill_w_raw "cpu" wBuf
w = (compute.plrun fill_w ((Fill)(.n 4)) : wBuf).w
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
gcd = ((p : tuple{Int, Int}) => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : tuple{Int, Int} -> Int
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
  seven halvings, not `e` — so the declared output is that sum evaluated in `f32`, which is at The host's data is built by **fill kernels**: the surface has no array-to-buffer \
  conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[0.0, 0.73105794, -0.13447031, 7.0463777]: array{Float, 4}"
---
Gate  = struct{.g (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
SiluA = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParA  = struct{.n Int, .in Gate}
silu_raw = (k : ParA) => {
  i = compute.range k.n
  g = (compute.read ((compute.Read _)(.from k.in.g, .at i)) : Float)
  b_a = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((SiluA)(.a (compute.write ((compute.Write _)(.to b_a, .at i, .value ((g / (1.0 + exp (0.0 - g))) ~ "element"))))))
}
silu = compute.parallel (silu_raw ~ "claim:Elementwise(element)") "cpu" SiluA
UpIn  = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .u (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY  = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParB  = struct{.n Int, .in UpIn}
up_raw = (k : ParB) => {
  i = compute.range k.n
  a = (compute.read ((compute.Read _)(.from k.in.a, .at i)) : Float)
  u = (compute.read ((compute.Read _)(.from k.in.u, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value ((a * u) ~ "element"))))))
}
up = compute.parallel (up_raw ~ "claim:Elementwise(element)") "cpu" OutY
Fill = struct{.n Int}
gBuf = struct{.g (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_g_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 1 then 1.0 else 0.0) + (if i == 2 then (0.0 - 1.0) else 0.0) + (if i == 3 then 2.0 else 0.0)
  ((gBuf)(.g (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_g = compute.parallel fill_g_raw "cpu" gBuf
g = (compute.plrun fill_g ((Fill)(.n 4)) : gBuf).g
uBuf = struct{.u (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_u_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 3.0 else 0.0) + (if i == 1 then 1.0 else 0.0) + (if i == 2 then 0.5 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((uBuf)(.u (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_u = compute.parallel fill_u_raw "cpu" uBuf
u = (compute.plrun fill_u ((Fill)(.n 4)) : uBuf).u
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
   the floor see the pair as one region. The `max` kernel seeds its fold with `((0.0 - 1.0) / 0.0)`, \
   which **is** the unit of `max`: the language has no unary minus, so `−∞` is written as a \
   literal over a zero literal. A finite stand-in for it is a claim that holds only for rows above \
   it. The row is `1, 2, 3, 4`, so the row's maximum is `4` and the exponents are `-3, -2, -1, 0`; \
   dividing every `exp` by the common factor `exp(-3)` leaves the closed form `e^k / (1 + e + e² + \
   e³)` for `k = 0…3`, and `1 + e + e² + e³` is `31.1928748505774` in exact arithmetic. The answer \
   is **not** that closed form rounded to f32: the language's `exp` is the degree-10 Taylor sum of \
   `docs/notes/floating-point.md` §6, whose f32 relative error is a few parts in 10⁶, so the \ The host's data is built by **fill kernels**: the surface has no array-to-buffer \
   conversion (`docs/notes/kernel-buffer-declarations.md`). The fold's trip count is the launch's extent read as `k.n + 0`: the extent's own node cannot carry a loop's count (`docs/notes/loop-conversion.md`), and the arithmetic leaves it the same number."
  output = "[0.03205853, 0.08714417, 0.23688345, 0.6439139]: array{Float, 4}"
---
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Maxs = struct{.m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParM = struct{.n Int, .in Row}
rowmax_raw = (k : ParM) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float)) ~ "combine"))
  b_m = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Maxs)(.m (compute.write ((compute.Write _)(.to b_m, .at i, .value fold (k.n + 0, ((0.0 - 1.0) / 0.0) ~ "identity"))))))
}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity)") "cpu" Maxs
Soft = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParY = struct{.n Int, .in Soft}
softmax_raw = (k : ParY) => {
  i = compute.range k.n
  m = (compute.read ((compute.Read _)(.from k.in.m, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) - m) ~ "element")) ~ "combine"))
  total = fold (k.n + 0, 0.0 ~ "identity")
  here = exp ((compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float) - m)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (here / total))))))
}
softmax = compute.parallel (softmax_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutY
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 4)) : xBuf).x
mx = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x x))) : Maxs)
out = (compute.plrun softmax ((compute.A Soft)(.n 4, .I Soft(.x x, .m mx.m))) : OutY)
compute.collect out.y : array{Float, 4}
```

### `rope.lichen`

```text
---
  order = "18"
  compute = import "compute.lichen"
  doc = "Rotary position embedding: `y[2i] = x[2i]·cos θ − x[2i+1]·sin θ` and \
   `y[2i+1] = x[2i]·sin θ + x[2i+1]·cos θ`, the per-pair rotation that carries a position \
   into an attention score. It is **three** kernels because the surface wants one kernel per \
   output buffer and this operator has three of them — a cosine per pair, a sine per pair, \
   and the rotation — and their claims are what let the floor see the chain. The rotation \
   runs **one lane per element**, not per pair, because a lane may write only one position: \
   lane `j` is in pair `(j - j % 2) / 2`, its partner is `j + 1 - 2 * (j % 2)`, and the sign of \
   the cross term is `1.0 - 2.0 * int2float (j % 2)` — so the whole pairwise rotation is \
   one expression rather than two. **`sin` and `cos` did not exist in this language at all \
   until now, and they are range-reduced polynomials rather than IEEE's**, so the declared \
   output is what the VM's own `sin_body`/`cos_body` compute, derived outside the workspace \
   by re-implementing those sequences in `f32`; over this table that and the real \
   functions agree to the printed precision, and lane 0 is exact. \
   `θ = position·inverse_frequency` is `0, 1, 2, 3`, every one an `f32` exactly, so \
   lane 0's `θ = 0` gives `cos 0 = 1` and `sin 0 = 0` exactly and lanes 0 and 1 answer \
   `x` untouched. **The angles are not restricted to `π/4`**: `θ = 1`, `2` and `3` cross \
   `π/4`, `π/2` and `3π/4`, so the table reaches three quadrant residues rather than one, \
   and `θ = 3` is past the `2.4` where a mis-reduced quadrant used to overflow. \
   The domain the reduction answers for is `|θ| ≤ 8192`, and above that it clamps rather \
   than refusing, so the answer there is the function of the clamp. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
   conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[1.0, 2.0, -1.744977, 4.685622, -7.536518, 2.0496058, -8.0589075, -6.9321]: array{Float, 8}"
---
Turn  = struct{.position (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .inverse_frequency (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutC  = struct{.c (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParC  = struct{.n Int, .in Turn}
cos_raw = (k : ParC) => {
  i = compute.range k.n
  angle = (compute.read ((compute.Read _)(.from k.in.position, .at i)) : Float) * (compute.read ((compute.Read _)(.from k.in.inverse_frequency, .at i)) : Float)
  b_c = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutC)(.c (compute.write ((compute.Write _)(.to b_c, .at i, .value (cos angle ~ "element"))))))
}
cosine = compute.parallel (cos_raw ~ "claim:Elementwise(element)") "cpu" OutC
OutS  = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS  = struct{.n Int, .in Turn}
sin_raw = (k : ParS) => {
  i = compute.range k.n
  angle = (compute.read ((compute.Read _)(.from k.in.position, .at i)) : Float) * (compute.read ((compute.Read _)(.from k.in.inverse_frequency, .at i)) : Float)
  b_s = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutS)(.s (compute.write ((compute.Write _)(.to b_s, .at i, .value (sin angle ~ "element"))))))
}
sine = compute.parallel (sin_raw ~ "claim:Elementwise(element)") "cpu" OutS
Pair  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .c (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY  = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
ParR  = struct{.n Int, .in Pair}
rotate_raw = (k : ParR) => {
  j = compute.range k.n
  odd = j % 2
  pair = (j - odd) / 2
  partner = j + 1 - 2 * odd
  c = (compute.read ((compute.Read _)(.from k.in.c, .at pair)) : Float)
  s = (compute.read ((compute.Read _)(.from k.in.s, .at pair)) : Float)
  here = (compute.read ((compute.Read _)(.from k.in.x, .at j)) : Float)
  other = (compute.read ((compute.Read _)(.from k.in.x, .at partner)) : Float)
  turned = here * c - (1.0 - 2.0 * int2float odd) * other * s
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at j, .value (turned ~ "element"))))))
}
rotate = compute.parallel (rotate_raw ~ "claim:Elementwise(element)") "cpu" OutY
Fill = struct{.n Int}
positionBuf = struct{.position (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_position_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 1 then 1.0 else 0.0) + (if i == 2 then 2.0 else 0.0) + (if i == 3 then 3.0 else 0.0)
  ((positionBuf)(.position (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_position = compute.parallel fill_position_raw "cpu" positionBuf
position = (compute.plrun fill_position ((Fill)(.n 4)) : positionBuf).position
inverse_frequencyBuf = struct{.inverse_frequency (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_inverse_frequency_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 1.0 else 0.0) + (if i == 2 then 1.0 else 0.0) + (if i == 3 then 1.0 else 0.0)
  ((inverse_frequencyBuf)(.inverse_frequency (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_inverse_frequency = compute.parallel fill_inverse_frequency_raw "cpu" inverse_frequencyBuf
inverse_frequency = (compute.plrun fill_inverse_frequency ((Fill)(.n 4)) : inverse_frequencyBuf).inverse_frequency
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0) + (if i == 6 then 7.0 else 0.0) + (if i == 7 then 8.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 8)) : xBuf).x
cosines = (compute.plrun cosine ((compute.A Turn)(.n 4, .I Turn(.position position, .inverse_frequency inverse_frequency))) : OutC)
sines = (compute.plrun sine ((compute.A Turn)(.n 4, .I Turn(.position position, .inverse_frequency inverse_frequency))) : OutS)
out = (compute.plrun rotate ((compute.A Pair)(.n 8, .I Pair(.x x, .c cosines.c, .s sines.s))) : OutY)
compute.collect out.y : array{Float, 8}
```

### `matmul.lichen`

```text
---
  order = "19"
  compute = import "compute.lichen"
  doc = "Matmul: `C[m][n] = Σ_k A[m][k] · B[k][n]`, the product every transformer layer spends
   its parameters on. It is **one lane per element of `C`** with the `k` loop **inside** the lane
   — the `k` loop is an `@loop` fold over the lane's own invocation, exactly as RMSNorm's is — so
   one lane reads `A[m][k]` and `B[k][n]` three times and writes one number. This is the **naive**
   matmul: there is no shared-memory staging, no `k`-blocking and no double buffering here, and
   all three of those need a **two-dimensional workgroup** that does not exist yet
   (`Plan::group_width` is one number, `EXECUTION_MODE_LOCAL_SIZE` is `(x, 1, 1)`, and neither
   `LocalInvocationId` nor `WorkgroupId` is bound at all). `docs/notes/two-dimensional-launch.md`
   §3(b) is that wall and `docs/notes/llm-operators.md` is the ladder it stands on.
   The row and the column are **named, not written**: `r = compute.row k` and `c = compute.col k`
   take the **second extent** out of the config itself. The primitive reads the launch's own leaf,
   not the `cfg` it is handed, so a body cannot pass a shape this launch never declared — and a
   config that declares only one extent is refused by name rather than answered. The argument is
   the one concession to the surface: the language has **no nullary lambda** (`parse.rs:837` wants a
   name before `=>`), so the wrapper takes one and ignores it, exactly as `compute.range` does with
   `k.n`. `k.cols` is still read below, for `B`'s row-major flattening, and that read is the
   ordinary runtime scalar it always was: it is the **same** local the coordinates divide by, so
   naming the shape costs nothing and hides nothing.
   The language also has no two-dimensional array, so both matrices are **row-major flattened** and the
   body indexes them itself: `A[r * k.k + k]` and `B[k * k.cols + c]`. **The inner dimension is a
   parameter now, not a literal** — `k.k`, carried as a scalar leaf of the config beside `k.cols` —
   where it used to be a `3` because a kernel parameter could not cross the graph path at all
   (`c92b6471` carries every leaf for what it is, at any depth; `e24d00fb` lets a value table hold a
   float). **Two ordering rules come with it, and both are positional**: the **first** scalar leaf is the
   *extent* (`scalar_leaf_classes` reads it as the count), and the **second** is what
   `compute.row`/`compute.col` divide by — so `K` has to sit **after** `cols`, not between `.n` and it.
   Putting it between them does not refuse: `[.n, .k, .cols]` answered `[58.0, 64.0, 31.0, 139.0]`
   instead of `[58.0, 64.0, 139.0, 154.0]`, because the coordinates then divided by `k = 3` and every
   lane took its row and column from the wrong pair. An `@loop` fold
   counts **down** — RMSNorm's does too — so `k` runs `2, 1, 0` and
   the last addend is the `k = 0` product; over exact integers the order changes nothing, and it is
   stated here because it is the order the body actually has.
   **The shape need not be square, and that is measured.** A device run uploads each host input at
   its **own length** — the invariant is `[0, len)`, with the padding past `len` filled on the
   device (`lichen-compute-gpu/src/dispatch.rs:458-461`; `own_elements` at `:320`, asked by both the
   staging reservation at `:976-984` and the copy at `:1066-1078`, so the two cannot drift) — so an
   element a lane reads is real as long as it is **inside its buffer**, whatever the lane count is.
   The device's other length rule — a host input **shorter than the count** refused by name while
   the staging was sized — **has been removed** (the owner's decision), so both backends answer the
   cleared padding with `0`. Measured on the RTX 3060: this `2 × 3 · 3 × 2` answers what the
   `cpu` path answers, element for element; under the old upload rule the same program answered
   `[25, 28, 28, 32]` there against `[58, 64, 139, 154]` here, because every entry past index `3`
   was read back as the zero padding. What is still uncaught is the other direction: a body that
   reaches **past the buffer** while the count stays inside it is an out-of-bounds device read, and
   the region that silently answers `0` shrank from `[count, padded)` to `[len, padded)` rather than
   closing.
   `A` is `1 2 3 / 4 5 6` and `B` is `7 8 / 9 10 / 11 12`, so
   `C[0][0] = 1·7 + 2·9 + 3·11 = 58`, `C[0][1] = 1·8 + 2·10 + 3·12 = 64`,
   `C[1][0] = 4·7 + 5·9 + 6·11 = 139` and `C[1][1] = 4·8 + 5·10 + 6·12 = 154`.
   **The declared output is that arithmetic worked out by hand, not a transcription of what the
   program printed.** Every entry is a product of integers under `19`, the largest sum is `154`, and
   `f32` represents every integer up to `2^24` exactly — so unlike SiLU and softmax, whose declared
   values are *modelled* because this language's `exp` is a Taylor sum and its `sqrt` a Newton
   iteration, a matmul over small integers has no approximation in it at all and this is an
   **exact** correctness check. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
   conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[58.0, 64.0, 139.0, 154.0]: array{Float, 4}"
---
Ain  = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 6))), .b (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
OutC = struct{.c (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Par  = struct{.n Int, .cols Int, .k Int, .in Ain}
Arg  = struct{.n Int, .cols Int, .k Int, .I Ain}
matmul_raw = (k : Par) => {
  i = compute.range k.n
  r = compute.row k
  c = compute.col k
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.a, .at (r * k.k + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.b, .at ((s(0) - 1) * k.cols + c))) : Float))) ~ "combine" ~ "element"))
  b_c = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutC)(.c (compute.write ((compute.Write _)(.to b_c, .at i, .value (fold (k.k, 0.0 ~ "identity")))))))
}
matmul = compute.parallel (matmul_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutC
Fill = struct{.n Int}
aBuf = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
fill_a_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0)
  ((aBuf)(.a (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_a = compute.parallel fill_a_raw "cpu" aBuf
a = (compute.plrun fill_a ((Fill)(.n 6)) : aBuf).a
bBuf = struct{.b (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
fill_b_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  v = (if i == 0 then 7.0 else 0.0) + (if i == 1 then 8.0 else 0.0) + (if i == 2 then 9.0 else 0.0) + (if i == 3 then 10.0 else 0.0) + (if i == 4 then 11.0 else 0.0) + (if i == 5 then 12.0 else 0.0)
  ((bBuf)(.b (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_b = compute.parallel fill_b_raw "cpu" bBuf
b = (compute.plrun fill_b ((Fill)(.n 6)) : bBuf).b
out = (compute.plrun matmul ((Arg)(.n 4, .cols 2, .k 3, .I Ain(.a a, .b b))) : OutC)
compute.collect out.c : array{Float, 4}
```

### `layernorm.lichen`

```text
---
  order = "20"
  compute = import "compute.lichen"
  doc = "LayerNorm: `y[i] = (x[i] - mean) / sqrt(var + eps) * w[i]`, with `mean = Σx / n` and \
   `var = Σ(x - mean)² / n` — the normaliser every transformer block opens with, and the \
   operator that makes **the mean and the variance two producers one consumer needs both**. \
   It is **three kernels in a diamond**: `Σx` and `Σx²` are two independent producers, and \
   one normalise kernel reads **both**. **The diamond now forms and merges** (`f973544a`: a \
   region is a weakly-connected component, where it used to be a forward-only adjacency scan \
   that left two independent producers in sibling regions), **but this operator still refuses** \
   — at a wall that is not the grouping's. **The one-pass identity `var = Σx²/n - mean²` is what makes it \
   a diamond at all** — the textbook two-pass form is mean-kernel → variance-kernel → \
   normalise, and there the mean is read by the variance kernel *and* by the normalise kernel, \
   which is `ProducerFeedsTwice` (`crates/lichen-runtime/src/fusion_claim/schema.rs:992`, in \
   `region_edges`: one value, two slots, and the language has no spelling for it). One-pass, \
   both folds are independent, so both edges **arrive** at one consumer and the shape **is** a \
   fan-in, and the region now holds all three **and merges** (`c9e65140`: a projection may read the \
   fused step's slots and the invocation, so `count = int2float k.n` bridges; `2fd3971f`: a runtime \
   scalar is a third kind of free value, so `k.eps` is one). **This file drives three `plrun`s and \
   records no graph**, so there is nothing here for the pass to merge — the shape it spells is what \
   merges when it is driven through the graph path, measured with `tile ≡ body` **bit for bit** on the \
   device, and the merged node's element frees are `d`, `n`, `r`, `r1` and `s0`, where `s0` is the \
   runtime scalar. softmax's diamond \
   does merge (`d364f547`) because its producers are **chained** \
   — the sum reads the maximum — so all three nodes group. **The folds are \
   two kernels and not two folds in one body**: two folds in one body is a chain of two \
   links, and a chain is not the shape this operator exists to measure. \
   `eps` is a **parameter now** — `k.eps`, passed as `1.0` — and that is measured here rather than \
   assumed: a recorded config carries the first scalar leaf as the extent and **every other leaf for \
   what it is, at any depth**, and the graph's value table carries a **float at its own class** \
   (`c92b6471`, `e24d00fb`), where a kernel parameter used to cross the graph path not at all. Two \
   limits are worth knowing: a `Float` leaf can only be a **literal or an argument**, never a value \
   computed inside the source function; and `compute.graphrun` refuses the `cpu` backend, so driving \
   `eps` *through a graph* would mean moving these kernels to `gpu` — the leg below is the direct \
   one. The value is still chosen so the \
   denominator lands on an exact square, `1.25 + 1.0 = 2.25`. It is **not** a stabiliser a \
   production LayerNorm would ship; this example's job is the exact arithmetic and the shape. \
   **What is exact**: the row is `1, 2, 3, 4`, so `Σx = 10` and `Σx² = 30`; `mean = 10/4 = 2.5` \
   is a sum of small integers over a division by a power of two, `Σx²/4 = 7.5` likewise, and \
   `7.5 - 6.25 = 1.25` is a difference of exact `f32` values. The centred values `-1.5, -0.5, \
   0.5, 1.5` are differences from `2.5` and are exact. Every integer here is below `2^24`. \
   **What is modelled**: `sqrt` alone. This language's `sqrt` is a Newton iteration over a \
   range reduction (`crates/lichen-kernel-ir/src/unary.rs:203`) — six steps from the seed \
   `2.0` — not the correctly rounded square root, so the scale is derived by re-implementing \
   **that sequence** in `f32` outside the workspace, never by calling the crate under test. \
   Its trace on `2.25` is `1.5625, 1.50125, 1.5, 1.5, 1.5, 1.5`: a modelled number that lands \
   **exactly** on `1.5` at step three and holds it for the rest, which is why the rest of the \
   answer is exact too. With `w = 3, 6, 3, 6` the row then answers `-3, -2, 1, 6`: `±1.5 / 1.5 = \
   ±1` exactly, and `0.5 / 1.5` is the **one** inexact step — it is `0.33333334` — whose \
   product with `3` or with `6` rounds back to an exact integer. An `@loop` fold counts \
   **down**, as matmul's does, so each sum accumulates `x[3], x[2], x[1], x[0]`; over exact \
   integers the order changes nothing and it is stated because it is the order the body has. \
   **The RTX 3060 answers `[-3.0, -2.0, 1.0, 6.0]` for the same three kernels through \
   `graphrun`, element for element with the `cpu` path**, so the Newton `sqrt` above did not \
   drift on a device — which it might, since this device does not round `f32` division \ The host's data is built by **fill kernels**: the surface has no array-to-buffer \
   conversion (`docs/notes/kernel-buffer-declarations.md`). The fold's trip count is the launch's extent read as `k.n + 0`: the extent's own node cannot carry a loop's count (`docs/notes/loop-conversion.md`), and the arithmetic leaves it the same number."
  output = "[-3.0, -2.0, 1.0, 6.0]: array{Float, 4}"
---
Row   = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Sums  = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS  = struct{.n Int, .in Row}
row_sum_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine"))
  b_s = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Sums)(.s (compute.write ((compute.Write _)(.to b_s, .at i, .value (fold (k.n + 0, 0.0 ~ "identity")))))))
}
row_sum = compute.parallel (row_sum_raw ~ "claim:Reduction(combine,identity)") "cpu" Sums
Sumsq = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParQ  = struct{.n Int, .in Row}
row_sumsq_raw = (k : ParQ) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float))) ~ "combine" ~ "element"))
  b_q = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Sumsq)(.q (compute.write ((compute.Write _)(.to b_q, .at i, .value (fold (k.n + 0, 0.0 ~ "identity")))))))
}
row_sumsq = compute.parallel (row_sumsq_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Sumsq
Moments = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .s (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .q (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY    = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ArgY    = struct{.n Int, .eps Float, .I Moments}
ParY    = struct{.n Int, .eps Float, .in Moments}
normalise_raw = (k : ParY) => {
  i = compute.range k.n
  count = int2float k.n
  m = (compute.read ((compute.Read _)(.from k.in.s, .at 0)) : Float) / count
  v = (compute.read ((compute.Read _)(.from k.in.q, .at 0)) : Float) / count - m * m
  d = sqrt (v + k.eps)
  x = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  w = (compute.read ((compute.Read _)(.from k.in.w, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (((x - m) / d * w) ~ "element"))))))
}
normalise = compute.parallel (normalise_raw ~ "claim:Reduction(element)") "cpu" OutY
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 4)) : xBuf).x
wBuf = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_w_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = (if i == 0 then 3.0 else 0.0) + (if i == 1 then 6.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 6.0 else 0.0)
  ((wBuf)(.w (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_w = compute.parallel fill_w_raw "cpu" wBuf
w = (compute.plrun fill_w ((Fill)(.n 4)) : wBuf).w
sm = (compute.plrun row_sum ((compute.A Row)(.n 4, .I Row(.x x))) : Sums)
sq = (compute.plrun row_sumsq ((compute.A Row)(.n 4, .I Row(.x x))) : Sumsq)
out = (compute.plrun normalise ((ArgY)(.n 4, .eps 1.0, .I Moments(.x x, .w w, .s sm.s, .q sq.q))) : OutY)
compute.collect out.y : array{Float, 4}
```

### `attention.lichen`

```text
---
  order = "21"
  compute = import "compute.lichen"
  doc = "Attention: `out[q][d] = Σ_k softmax_k(scores[q][k]) · V[k][d]` over \
   `scores[q][k] = Σ_j Q[q][j] · K[k][j] / sqrt(dim)` — the operator the ladder's last row names, and \
   the one that **does not run the way the row describes**. It is two kernels, not one: a `scores` \
   kernel with the `j` fold inside a lane, then an `attend` kernel that is **one lane per output \
   element** and recomputes the softmax's row maximum and its denominator **inside the lane** by \
   looping over the keys with `@loop`, exactly as `examples/softmax.lichen` does. The program's \
   other three kernels are **fill kernels**: the surface has no array-to-buffer conversion, so the \
   host builds `Q`, `K` and `V` by running kernels that write them \
   (`docs/notes/kernel-buffer-declarations.md`). **This is a \
   demonstration of what the surface can express today, not a schedule anyone would ship**: there \
   is no shared-memory staging, no tiling and no `k`-blocking, and the one-kernel form the ladder's \
   row really asks for — the score recomputed per lane, so `O(n·dim)` work per lane per fold — \
   **cannot be written at all** (the reason is below, with the pass's own words and the refusal \
   verbatim). What ships here is the nearest spelling the surface takes, and the distance between \
   the two is the finding.\n\n\
   **Two leaves are positional and both kernels lean on it.** The **first** scalar leaf is the \
   launch extent (`scalar_leaf_classes`, `crates/lichen-compute/src/compute.rs:1849`) and the \
   **second** is what `compute.row`/`compute.col` divide by \
   (`crates/lichen-compute/src/compute/body.rs:788`). So `ParS`'s second leaf is `keys` — its \
   lanes are `queries × keys`, the query over the rows and the key over the columns — while \
   `ParA`'s second leaf is `dim`, its lanes being `queries × dim`. The same field name would have \
   meant two different shapes, so each kernel names its own: `compute.row k` is the **query** row in \
   `attend` and in `scores`, and `compute.col k` is the key there and the head-dimension column in \
   `attend`. `dim` and `keys` are **parameters, not literals** — both are third leaves, after the \
   second extent, which is where a new leaf has to go (`examples/matmul.lichen` records what a leaf \
   put between them answers instead). Nothing here is a 2-dimensional array: `Q`, `K`, `V` and the \
   scores are **row-major flattened** and the bodies index them, `query * k.dim + s(0) - 1` and \
   `s(0) * k.dim + key` for the score, `query * k.keys + s(0) - 1` for the softmax's read of the \
   scores and `(s(0) - 1) * k.dim + column` for `V`. The language still has no 2-D array, and a \
   **per-row reduction that is not a whole-kernel fold** is still not a thing: the row maximum is a \
   fold over the keys of the lane, and the *scores* matrix is a kernel of its own rather than a \
   reduction a lane shares.\n\n\
   **The one-kernel form is refused, and the refusal is about the loop nest, not about attention.** \
   A body that scores `(q, k)` inside the lane needs the `j` fold **nested inside** the key fold. \
   Nesting works — an `@loop` recursion applied inside another `@loop`'s step lowers, and the two \
   nests are real (measured: a `@loop` inside a step, and a third inside that, both run) — but \
   **the enclosing step's own value is then computed in the enclosing block, after the inner nest \
   has returned, and read back on a branch that block does not dominate**. The lowering demands \
   values from the codomain outwards (`crates/lichen-compute/src/compute/body.rs:222`) and \
   `lower_loop_steps` sets the step's terminator **after** its operands \
   (`crates/lichen-compute/src/compute/body.rs:1200`), so the inner nest's entry terminator is \
   overwritten and the inner nest's result has no dominator. The refusal is by name, verbatim: \
   *`Bin(Float, Div) reads ValueId(32), which block 7 receives as a parameter, and block 7 does not \
   dominate block 2`*. Three folds that each need the score therefore cannot share one body; the \
   score is a kernel. **That is a rule, not a preference**, and it is the same rule that makes \
   `scores` and `attend` two kernels rather than one.\n\n\
   **One workaround, and it is a preference.** The value fold carries the row's maximum in its \
   state — the entering call binds the maximum as `acc`'s third state slot and the step carries it \
   as `s(2)` — because the same \
   demand rule fixes the order the nests are entered in: `acc`'s **entering call** is what pulls `m` \
   in, so `top`'s nest is entered from the entry block and dominates the two nests after it, while \
   `m` demanded from `acc`'s *body* would enter `top`'s nest **inside** `acc`'s and leave \
   `denominator`'s fold reading a value it cannot see. Carrying the max is the honest spelling of \
   what a tiled attention does anyway — a lane holds its row's maximum across the key loop — and it \
   is **a preference**: dropping it and reading `m` from a third kernel's buffer would be equally \
   correct and is `examples/softmax.lichen`'s own shape.\n\n\
   **What the fusion machinery says, measured with the floor installed** \
   (`install_floor`, region merge then automatic tiling, over the two recorded fragments as a \
   two-node graph with counts `4` and `8`). **The region groups** — a weakly-connected component, \
   so the scores kernel and the attend kernel are one region — and the merge then refuses it by \
   name, and the tiling stage refuses each node after it. Verbatim:\n\n   ```\n   region 0 (nodes 0, 1): refused: \
   ElementWithoutProducer: the instance at 1 reads the one at 0 and its element `d` does not read \
   the producer's value\n   node 0: refused: BeyondANode: the instance at 0 claims the role(s) element beyond \
   `combine` and `identity`, whose code reads the value another node produced, and this reader \
   names no producer\n   node 1: refused: BeyondANode: the instance at 1 claims the role(s) element beyond `combine` \
   and `identity`, whose code reads the value another node produced, and this reader names no \
   producer\n   ```\n\n\
   That is the ladder's *small lie the surface forces* arriving at attention: both kernels claim \
   `Reduction(combine,identity,element)`, and each one's `element` **is** a whole fold, so there is \
   no element expression left for a producer to flow into. The grouping is no longer the wall; \
   the **claim** is. A pair whose consumer's `element` genuinely reads the producer's value would \
   be a different shape, and this one is not.\n\n\
   **The device leg, measured.** The `attend` kernel **runs on the RTX 3060** through \
   `compute.plrun` on `gpu`, fed the same scores, and answers \
   `[2.510164, 3.510164, 4.510164, 5.510164, 3.924232, 4.9242315, 5.9242315, 6.9242315]` against \
   the cpu path's `[2.5101643, 3.5101643, 4.5101643, 5.5101643, 3.924232, 4.924232, 5.9242315, \
   6.924232]`: **three of the eight differ in the last bit**, which is the division the device does \
   not round correctly (`docs/notes/floating-point.md` §7), not a drift in either definition. \
   `matmul`, `layernorm`, `rmsnorm` and `softmax` all run there unchanged, so the device is \
   available and the difference is this operator's. The `scores` kernel is **refused outright** by \
   the SPIR-V emitter: *a `Select`'s two arms are computed in different classes ([Int, Float])* — \
   and the trigger is narrow and reproducible. `sqrt 4.0` runs, `sqrt (int2float k.n + 0.25)` (which \
   is what `examples/rmsnorm.lichen` writes) runs, and **`sqrt (int2float k.dim)` does not**, for \
   the second extent or for the third leaf alike; adding one arithmetic term between the crossing \
   and the root, `sqrt (int2float k.dim + 0.0)`, runs and answers the same numbers. The crossing's \
   result feeding the Newton definition directly is what leaves the `Select` inside `sqrt_body` \
   with one arm unclassed (`crates/lichen-kernel-ir/src/unary.rs:203`). This file keeps the honest \
   spelling and declares the `cpu` backend, which is the one the example suite runs.\n\n\
   **What a tiled attention would need from the workgroup geometry.** A row of keys is a reduction \
   *across lanes*, and the only cross-lane exchange the surface has is a `Shared` staging slice and \
   a barrier. Three things, and the newest note has already measured two of them. **(a)** A real \
   two-dimensional workgroup: `Q[q][·]` and `K[k][·]` want one axis for the query and one for the \
   head dimension, and `docs/notes/two-dimensional-workgroup.md` §1.1 names the core construction \
   site — a `Shared` slice is an `OpTypeArray` with **one** length constant and `slice_pointer` \
   emits a **one-index** access chain, so a row-shaped exchange needs a nested array and a \
   **two-index** chain. **(b)** The rectangle itself is **not** the wall, and that is worth knowing \
   before planning around it: the same note §4 measures `OpExecutionModeLocalSize = (16, 4, 1)` \
   compiling, validating and running on the RTX 3060. What is the wall is (a)'s companion under \
   it — the same §4 measures a staged `Shared` exchange at `count = 64` under a rectangle answering \
   a wrong number and **writing nothing past index 15, with no error, no refusal and no report \
   line**. That silent class is exactly what a tiled attention's partial sums would go through, so \
   it is the thing to gate before the first tile. **(c)** the scores tile must be **recomputed per \
   query** rather than shared, which is the `dot` fold above and is the same recomputation a \
   `Max`/`Min` tile already covers for the softmax's row maximum. `Plan::group_width` is one number \
   and `check_plan` refuses a group narrower than the workgroup \
   (`crates/lichen-kernel-ir/src/plan.rs:110-137`), which is the same one-axis rule from the other \
   end. `compute.row`/`compute.col` over **one** divisor are the landed spelling working as \
   intended and are not part of the wall. `docs/notes/two-dimensional-launch.md` §3(b) and \
   `docs/notes/two-dimensional-workgroup.md` are that wall, and `docs/notes/llm-operators.md` is the \
   ladder it stands on. **Nothing in this file needs any of it** — every fold here is inside one \
   lane — which is exactly why the file can exist and the tiled one cannot.\n\n\
   **What is exact and what is modelled.** Exact, by hand and in `f32`: the four dot products \
   (`2, 1` for query 0 and `0, 2` for query 1, every term a product of integers under `6`), the row \
   maxima (`1.0` and `1.0`), the offsets they leave (`0, -0.5` and `-1, 0`), and `sqrt(dim)`: this \
   language's `sqrt` is a Newton iteration over a range reduction \
   (`crates/lichen-kernel-ir/src/unary.rs:203`), and on `4` its reduction is the identity and its \
   six steps from the seed `2.0` stay at `2.0`, so the scale is the **exact** `2` and every \
   division by it is exact. Every integer is below `2^24`. Modelled: `exp`, which is a clamped, \
   halved, degree-10 Taylor sum squared back up — **never** a reference `eˣ` — and every product, sum \
   and final division that touches one. The declared output was derived **independently**, by \
   re-implementing that published sequence in `f32` outside the workspace and never calling the \
   crate under test: `exp_model(0) = 1`, `exp_model(-0.5) = 0.6065316`, `exp_model(-1) = 0.3678806`; \
   the denominators are `1 + 0.6065316 = 1.6065316` and `0.3678806 + 1 = 1.3678806`; and each \
   `@loop` fold **counts down** — `examples/matmul.lichen` and `examples/layernorm.lichen` say so \
   too — so row 0 accumulates `exp(-0.5)·V[1][d]` before `exp(0)·V[0][d]` and row 1 the other way \
   round, and each lane's sum is divided by its row's denominator once, at the write. That gives \
   `[2.5101643, 3.5101643, 4.5101643, 5.5101643, 3.924232, 4.924232, 5.9242315, 6.924232]`, which \
   is what the program prints. The data is `Q = 2 0 1 1 / 0 0 2 1`, `K = 1 1 0 0 / 0 1 1 0`, \
   `V = 1 2 3 4 / 5 6 7 8`, so the four rows of the answer differ from each other by `4 · p` and \
   by `1` across the head dimension — a transposition of either matrix would change every entry."
  output = "[2.5101643, 3.5101643, 4.5101643, 5.5101643, 3.924232, 4.924232, 5.9242315, 6.924232]: array{Float, 8}"
---
QK = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .k (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
Scored = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS = struct{.n Int, .keys Int, .dim Int, .in QK}
ArgS = struct{.n Int, .keys Int, .dim Int, .I QK}
scores_raw = (k : ParS) => {
  i = compute.range k.n
  query = compute.row k
  key = compute.col k
  sc = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop dot = s => if s(0) == 0 then s(1) else dot (s(0) - 1, (s(1) + (compute.read ((compute.Read _)(.from k.in.q, .at (query * k.dim + s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from k.in.k, .at (key * k.dim + s(0) - 1))) : Float)) ~ "combine")
  ((Scored)(.s (compute.write ((compute.Write _)(.to sc, .at i, .value ((dot (k.dim, 0.0 ~ "identity") / sqrt (int2float k.dim)) ~ "element"))))))
}
scores = compute.parallel (scores_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Scored
QSV = struct{.s (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
OutA = struct{.o (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
ParA = struct{.n Int, .dim Int, .keys Int, .in QSV}
ArgA = struct{.n Int, .dim Int, .keys Int, .I QSV}
attend_raw = (k : ParA) => {
  i = compute.range k.n
  query = compute.row k
  column = compute.col k
  o = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  @loop top = s => if s(0) == 0 then s(1) else top (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) ~ "element")) ~ "combine")
  m = top (k.keys, ((0.0 - 1.0) / 0.0) ~ "identity")
  @loop den = s => if s(0) == 0 then s(1) else den (s(0) - 1, (s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) - m) ~ "element")) ~ "combine")
  denominator = den (k.keys, 0.0 ~ "identity")
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, (s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.s, .at (query * k.keys + s(0) - 1))) : Float) - m) ~ "element") * (compute.read ((compute.Read _)(.from k.in.v, .at ((s(0) - 1) * k.dim + column))) : Float)) ~ "combine", s(2))
  ((OutA)(.o (compute.write ((compute.Write _)(.to o, .at i, .value ((acc (k.keys, 0.0 ~ "identity", m) / denominator) ~ "element"))))))
}
attend = compute.parallel (attend_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutA
QBuf = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
KBuf = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
VBuf = struct{.v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
Fill = struct{.n Int}
fill_q_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 2.0 else 0.0) + (if i == 2 then 1.0 else 0.0) + (if i == 3 then 1.0 else 0.0) + (if i == 6 then 2.0 else 0.0) + (if i == 7 then 1.0 else 0.0)
  ((QBuf)(.q (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_k_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 1.0 else 0.0) + (if i == 5 then 1.0 else 0.0) + (if i == 6 then 1.0 else 0.0)
  ((KBuf)(.k (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_v_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0) + (if i == 6 then 7.0 else 0.0) + (if i == 7 then 8.0 else 0.0)
  ((VBuf)(.v (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_q = compute.parallel fill_q_raw "cpu" QBuf
fill_k = compute.parallel fill_k_raw "cpu" KBuf
fill_v = compute.parallel fill_v_raw "cpu" VBuf
qm = (compute.plrun fill_q ((Fill)(.n 8)) : QBuf)
kk = (compute.plrun fill_k ((Fill)(.n 8)) : KBuf)
vv = (compute.plrun fill_v ((Fill)(.n 8)) : VBuf)
scale = (compute.plrun scores ((ArgS)(.n 4, .keys 2, .dim 4, .I QK(.q qm.q, .k kk.k))) : Scored)
out = (compute.plrun attend ((ArgA)(.n 8, .dim 4, .keys 2, .I QSV(.s scale.s, .v vv.v))) : OutA)
compute.collect out.o : array{Float, 8}
```

### `kvcache.lichen`

```text
---
  order = "22"
  compute = import "compute.lichen"
  doc = "KV-cache append: `K'[p][d] = K[p][d]` for every `p` but the one written, and \
    `K'[t][d] = k_new[d]` at `t` — the write a decoding loop performs once per token and \
    that no example above it in `docs/notes/llm-operators.md` performs. Attention is the \
    **read** side of the same buffers; this is the write side, and it is the one that turns out \
    to be the harder of the two.\n\n\
    **The cache is `capacity` positions of `dim`-wide rows, and the launch is over \
    `capacity * dim` elements — one lane per element of the cache, not one per new row.** The \
    reason is a rule, measured: a `compute.write` names a slot of an output buffer that is \
    **exactly as long as the launch's extent** (`crates/lichen-compute/src/compute.rs:5979`), and \
    `compute.collect` hands back **every** element of it (`:1550-1621`), so a kernel that \
    launched over `dim` and wrote one row could not hand the cache back at all. A second rule \
    forces the rest: a `compute.write` is **the value of an output position, not an effect** \
    (`code-audit.md` P1-42, `wontfix:D17`), so every lane must write and no two lanes may \
    write the same slot. The spelling that satisfies both is a **bijection** from lane to slot, \
    and this one is the **transpose** of the obvious one: lane `i` is \
    `(column, position) = (i / capacity, i % capacity)` and writes at \
    `position * dim + column`. Lane 1 writes slot 2 and lane 4 writes slot 1, so **the write \
    address is computed and is not the lane's own index** — which is the whole point of the \
    exercise. A row-major enumeration would make `.at i` and the question would not be asked. \
    `compute.row` and `compute.col` are the **column** and the **position** here, in that \
    order, because the **second** leaf is what they divide by (below).\n\n\
    **The leaves are positional and the new ones go after the second.** The **first** scalar leaf \
    is the launch extent (`scalar_leaf_classes`, \
    `crates/lichen-compute/src/compute.rs:1849`) and the **second** is what `compute.row` / \
    `compute.col` divide by (`crates/lichen-compute/src/compute/body.rs:788-815`). So `Par` is \
    `.n` (the extent `8`), `.cap` (the divisor `4`), `.dim` (`2`) and `.t` (`2`) in that order: \
    a new leaf goes **after** the divisor, as `examples/matmul.lichen` says for `k.k`. This \
    kernel has **three** runtime scalars and runs on the direct path; the graph path carries all \
    three beside the extent (measured, below).\n\n\
    **The arithmetic is exact, and the declared output is worked out by hand.** The cache's \
    filled prefix is positions `0` and `1` — `K = 1 2 / 3 4` and `V = 5 6 / 7 8` — positions `2` \
    and `3` are spare, the new key row is `100 200`, the new value row is `300 400`, and `t = 2`, \
    which is what a decode loop does: the new row lands on the **first unfilled** position. So \
    `K' = 1 2 / 3 4 / 100 200 / 0 0` and `V' = 5 6 / 7 8 / 300 400 / 0 0`. Every element is an \
    integer below `2^24`, every one of them is the copy or the replacement and not a rounding of it, and \
    there is no `exp` and no `sqrt` anywhere in the body — **nothing here is modelled**, unlike \
    SiLU, softmax, LayerNorm and attention, whose declared values are the language's own \
    transcendental sequences.\n\n\
    **What the write rules say about a region past what the lane's own count covers — three \
    rules, and they do not agree with each other.** (a) A write outside the span is **dropped \
    silently** (`compute.rs:6255-6263`, whose own sentence is *a write outside the span is \
    dropped rather than aliasing another worker's slots*). Measured, on both backends, with the \
    kernel a decode loop would really want — one lane per **column** of the new row, writing at \
    `t * dim + d`, which is `4` and `5` over a count of `2`: the `cpu` path answers `[0.0, 0.0]` \
    and so does the RTX 3060. **No error, no refusal, no report line.** That is the shape this \
    example cannot take, and it is why the cache is rewritten whole. (b) A host input is uploaded \
    at its **own length** (`lichen-compute-gpu/src/dispatch.rs:320`), so an index past it reads `0`. \
    The device used to refuse an input shorter than the count; that rule is gone (the owner's \
    decision), so **the same cache with its keys holding only the filled prefix, `[1, 2, 3, 4]`, \
    answers `[1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0]` on the device as it already did on `cpu`** \
    — the elements past the input's own length reading as `0` on both (`compute.rs:6234`). **A cache \
    whose capacity is larger than its filled prefix can therefore be handed over as a shorter buffer \
    now**, with the zeros past the prefix being the program's own business on either backend. This \
    file still carries the whole cache, spare positions and all. (c) \
    A body that reaches **past its buffer** while the count stays inside it is an \
    out-of-bounds device read, uncaught (`examples/matmul.lichen` measures the half that is \
    caught). And **a written-but-never-read region is not observable**: the output buffer is \
    exactly `count` long on the host, and `collect` reads all of it, so there is no region past \
    the count to see. On the device the output is allocated `padded` — a whole number of \
    workgroups, `64` here (`dispatch.rs:1052`) — and `collect` still reads `count`, so the surplus \
    lanes' slots are written and never handed back.\n\n\
    **The device leg, measured, and it is the leg this operator is not ready for.** With the \
    incoming row padded to the cache's own length — `nk = 100 200 0 0 0 0 0 0` — the RTX 3060 \
    answers `[1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0]` and \
    `[5.0, 6.0, 7.0, 8.0, 300.0, 400.0, 0.0, 0.0]` through the direct `compute.plrun` leg, \
    **element for element what `cpu` answers**. As the example spells it — a `dim`-wide incoming \
    row beside a `capacity * dim` cache — the device used to **refuse** by name \
    (`InputShorterThanCount`, buffer 2, the new key row). **That rule is gone and this program now \
    runs on the device: 16 of 16 elements bit-exact.** This file still declares \
    `cpu`, the backend the example suite runs.\n\n\
    **The graph path records this body and then refuses to dispatch it — measured, both halves.** \
    A single-kernel step over the same operator records cleanly: **1 node, 8 graph inputs, 10 \
    values**, and `node 0: inputs [4, 5, 6, 7] / count Value(0) / scalars [Value(1), Value(2), \
    Value(3)]` — the four buffers at inputs 4 to 7, the extent at value 0 and **all three** \
    runtime scalars carried beside it, which is `record_launch`'s own rule (*the first scalar leaf \
    is the extent, and every other leaf is carried for what it is*, \
    `crates/lichen-compute/src/compute.rs`, §9 of `docs/notes/two-dimensional-launch.md`). The \
    recording is **byte-identical** with the incoming row narrow and with it padded, so nothing in \
    it looks at the data. The **run** is refused, by name, at submit: *the backend failed \
    submitting a kernel node: input buffer 0 holds 4 element(s) but the run covers 8 \
    index/indices*. The same program with 12-element inputs says *holds 6 element(s)*, and with \
    16-element inputs it does not refuse at all — 32 bytes reads as 4, 48 as 6, 64 as 8, so the \
    count this leg hands the backend is the payload length **in eight-byte elements** while every \
    buffer here is `Float`. I did not trace that to a site, so it is stated as the arithmetic I \
    measured and not as a diagnosis; what it costs is the whole graph leg for this operator. For \
    contrast, the same crate's **one-buffer `Float` graph with no runtime scalar** runs on the \
    device through `compute.graphrun` and answers `[2.0, 4.0, 6.0, 8.0, 10.0, 12.0, 14.0, 16.0]`, \
    so the leg itself is available and the refusal is this operator's.\n\n\
    **What a decode loop still needs, stated plainly.** Three things, and two of them are \
    expressible today. (a) **Carried state across calls — yes.** A `compute.plrun`'s output is a \
    buffer a later `compute.plrun` can name as an input; `examples/silu.lichen` and \
    `examples/attention.lichen` both do it, and this operator's output is shaped to be fed \
    straight back in, which is the only reason it rewrites the whole cache rather than one row. \
    (b) **A second kernel reading what the first wrote — yes**, the same two steps. (c) **A loop — \
    no, and this is a wall rather than a gap.** The language has **no iteration form at all**: no \
    `for`, no `while`, no loop keyword appears in any of the 34 examples, and `@loop` is a fold \
    *inside* one lane, not over dispatches. A kernel cannot loop over positions internally \
    either, because a `compute.call` inside a parallel body is refused with a `NodeId` \
    (`code-audit.md` P1-39, `todo`). So `N` decode steps are `N` source-level `compute.plrun` \
    calls with `t` written out at each, and **there is no way to advance `t`**: it is a parameter \
    the host hands in, and the language has no mutable state to hold a counter between calls. \
    That is the missing piece, and nothing in this file's operator has to change for it to land.\n\n\
    **One workaround, and it is a rule rather than a preference.** The lane enumerates the cache \
    **column-major** so the write address is not the lane's own index. It could have enumerated \
    it row-major and written `.at i`, which is shorter and answers the same numbers — but it \
    would not have exercised the write side at all, and the whole reason this example exists is \
    that the write address is computed. The rule underneath is D17's, that a write is the value \
    of a position rather than an effect, which forces one write per lane per output and so \
    forces the bijection. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`). The append's freshness test is an **arithmetic select** rather than a conditional over a read: the surface refuses a conditional whose arm reads a buffer, so the old value and the new value are read unconditionally and the test picks between them with a factor. The arithmetic is exact — every element is an integer below `2^24` — so each position holds the copy or the replacement and not a rounding of it."
  output = "([1.0, 2.0, 3.0, 4.0, 100.0, 200.0, 0.0, 0.0], [5.0, 6.0, 7.0, 8.0, 300.0, 400.0, 0.0, 0.0]): tuple{array{Float, 8}, array{Float, 8}}"
---
KVIn  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2))), .nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
KVOut = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
Par   = struct{.n Int, .cap Int, .dim Int, .t Int, .in KVIn}
Arg   = struct{.n Int, .cap Int, .dim Int, .t Int, .I KVIn}
append_raw = (k : Par) => {
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  row_k = (compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)
  row_v = (compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)
  fresh = position == k.t
  new_k = old_k + ((row_k - old_k) * (if fresh then 1.0 else 0.0))
  new_v = old_v + ((row_v - old_v) * (if fresh then 1.0 else 0.0))
  b_k = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  b_v = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((KVOut)(.k (compute.write ((compute.Write _)(.to b_k, .at slot, .value (new_k ~ "element")))), .v (compute.write ((compute.Write _)(.to b_v, .at slot, .value (new_v ~ "element"))))))
}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "cpu" KVOut
Fill = struct{.n Int}
kbufBuf = struct{.kbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_kbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((kbufBuf)(.kbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_kbuf = compute.parallel fill_kbuf_raw "cpu" kbufBuf
kbuf = (compute.plrun fill_kbuf ((Fill)(.n 8)) : kbufBuf).kbuf
vbufBuf = struct{.vbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_vbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 5.0 else 0.0) + (if i == 1 then 6.0 else 0.0) + (if i == 2 then 7.0 else 0.0) + (if i == 3 then 8.0 else 0.0)
  ((vbufBuf)(.vbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_vbuf = compute.parallel fill_vbuf_raw "cpu" vbufBuf
vbuf = (compute.plrun fill_vbuf ((Fill)(.n 8)) : vbufBuf).vbuf
nkBuf = struct{.nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nk_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 100.0 else 0.0) + (if i == 1 then 200.0 else 0.0)
  ((nkBuf)(.nk (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nk = compute.parallel fill_nk_raw "cpu" nkBuf
nk = (compute.plrun fill_nk ((Fill)(.n 2)) : nkBuf).nk
nvBuf = struct{.nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nv_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 300.0 else 0.0) + (if i == 1 then 400.0 else 0.0)
  ((nvBuf)(.nv (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nv = compute.parallel fill_nv_raw "cpu" nvBuf
nv = (compute.plrun fill_nv ((Fill)(.n 2)) : nvBuf).nv
out = (compute.plrun append ((Arg)(.n 8, .cap 4, .dim 2, .t 2, .I KVIn(.k kbuf, .v vbuf, .nk nk, .nv nv))) : KVOut)
(compute.collect out.k : array{Float, 8}, compute.collect out.v : array{Float, 8})
```

### `gemv.lichen`

```text
---
  order = "23"
  compute = import "compute.lichen"
  doc = "GEMV: `y[m] = Σ_k W[m · kdim + k] · x[k]` — a matrix times a **vector**, one lane per \
    output element and the `k` fold **inside** the lane. It is the shape a token-by-token decoder \
    runs (`examples/matmul.lichen`'s matrix × matrix is the training shape), and it is the \
    companion of `examples/kvcache.lichen`: a decode step is a GEMV against the keys, a softmax, \
    then a GEMV against the values.\n\n\
    **One lane per output element, and that is a rule rather than a preference.** A \
    `compute.write` names a slot of an output buffer that is **exactly as long as the launch's \
    extent** (`crates/lichen-compute/src/compute.rs:5979`), and a write is **the value of an output \
    position, not an effect** (`code-audit.md` P1-42, `wontfix:D17`), so one lane per `m` is a \
    bijection from lane to slot and the only spelling whose answer does not depend on a schedule. \
    The consequence is that this operator needs **no coordinates at all**: `m` is the lane index \
    itself, `i = compute.range k.n`, so — unlike `examples/matmul.lichen` and \
    `examples/attention.lichen` — this body never reads `compute.row` or `compute.col`.\n\n\
    **The leaves are positional, and here the second one is free — measured, not assumed.** The \
    **first** scalar leaf is the launch extent (`scalar_leaf_classes`, \
    `crates/lichen-compute/src/compute.rs:1849`) and the **second** is what \
    `compute.row`/`compute.col` divide by (`crates/lichen-compute/src/compute/body.rs:788-815`), so \
    a new leaf goes **after** the divisor. Both rules are about *coordinates*, and this body has \
    none: `second_extent` is reached only from `coordinate` (`body.rs:788`), so with no \
    `row`/`col` read the second leaf is an ordinary runtime scalar wherever it sits. Three orders \
    over one data set, measured: `.n, .spare, .kdim` answers \
    `[14.0, 32.0, 50.0, 68.0]`; `.n, .kdim, .spare` answers `[14.0, 32.0, 50.0, 68.0]` — **the \
    same**, which is why `kdim` is second here and third in `examples/matmul.lichen`; and \
    `.kdim` **first** answers `[14.0, 38.0, 62.0]` with **no refusal**, because the first leaf is \
    the extent whatever it is called: the launch then runs over `kdim = 3` with `n = 4` as the \
    stride, and `x`'s fourth element reads past its own three and answers `0` \
    (`compute.rs:6234`). **So the positional rule bites on the first leaf even for an operator \
    with no coordinates, and does not bite on the second.**\n\n\
    **An `@loop` fold counts down**, as RMSNorm's and `examples/matmul.lichen`'s do, so `k` runs \
    `2, 1, 0` and the last addend is the `k = 0` product; over exact integers the order changes \
    nothing, and it is stated because it is the order the body actually has.\n\n\
    **Chaining, measured: a two-kernel decode step works today.** A `compute.plrun`'s output is a \
    buffer a later `compute.plrun` names as an input, and this operator takes \
    `examples/kvcache.lichen`'s output **unchanged**, because the cache's own layout *is* this \
    operator's layout: `K[p][d]` row-major flattened is `W[m · kdim + k]` with `m` the position, \
    `kdim` the head dimension and `k` the column. Append, then score one query over the whole \
    cache, is therefore this file's kernel with `.w` = the cache's `.k`, and over the kvcache \
    program it answers `[3.0, 7.0, 300.0, 0.0]` — **element for element what the same kernel over \
    a literal flattened cache answers**. **The values half does not chain through this spelling**, \
    and the reason is the transpose and nothing else: `y[d] = Σ_p a[p] · V[p · dim + d]` reads at \
    `(s(0) - 1) * dim + i` rather than at `i * kdim + s(0) - 1`, because the cache is \
    position-major and this operator's matrix is output-major. That body answers \
    `[312.0, 414.0]` chained and `[312.0, 414.0]` direct. **So the decode step is two kernels and \
    two address forms, not two kernels and one**: the keys half is this file's kernel as \
    committed, and the values half is `examples/attention.lichen`'s own address form.\n\n\
    **The device leg, measured on the RTX 3060 — and the rule that used to refuse it is gone.** The \
    vector is `kdim` long and the count is `M`, so every GEMV with `kdim < M` — which is every \
    decode-step use, where the reduction runs over the cache positions and the outputs are the head \
    dimension — hands the device an input shorter than the run. That used to be \
    `InputShorterThanCount` (*input buffer 1 holds 3 element(s) but the run covers 4 index/indices, \
    so a lane would read past it*); the owner removed the rule, and **this program now runs on the \
    device from this source**: 4 of 4 elements bit-exact against `cpu`, the same \
    `[14.0, 32.0, 50.0, 68.0]` the padded measurement predicted. This file still declares `cpu`, \
    the backend the example suite runs.\n\n\
    **The graph leg carries this operator once the step's result is collected directly.** From \
    the CLI there is no graph leg at all — *compute.graph: this graph dispatches to a device, but \
    no compute backend is installed* — so this was measured through the probe \
    `crates/lichen-compute/tests/lichen_compute/zz_graph_argument_probe.rs` with the GPU backend \
    installed. The two checks that used to refuse it are gone: the graph argument's buffers must \
    sit in `.in` (that refusal was a spelling, and this file's `.in` already does it), and \
    `RaggedInputs` was removed with the length rule. What the graph leg still refuses is **the \
    collect spelling**: `compute.collect (compute.graphrun built (…))` — collecting the run \
    **directly** — runs, while binding it first and collecting a field (`out = (compute.graphrun … \
    : OutG)` and then `compute.collect out.y`) refuses with the collect's *empty value* sentence. \
    GEMV's program binds and collects a field, which is why its graph leg had looked refused; the \
    evidence and the two-buffer control are in `docs/notes/llm-operators.md`. **The fusion floor \
    now reads this operator, and both of its stages refuse it by name**: the merge reports \
    `RegionIsNotAPair` (one node is not a producer and a consumer) and the tiling pass reports \
    `BeyondANode` for the `element` role, with the answer `[14.0, 32.0, 50.0, 68.0]` unchanged \
    either way. The refusal's sentence does not describe this body — it says the `element` code \
    reads a value another node produced, and this operator's reads are graph arguments — so the \
    reading is recorded rather than explained; it is in `docs/notes/llm-operators.md`.\n\n\
    **What a decode step still needs, beyond the walls the KV-cache example names.** The language \
    has no iteration form (`for`, `while`, no loop keyword in any example), `@loop` is a fold \
    *inside* one lane, and a `compute.call` inside a parallel body is refused with a `NodeId` \
    (`code-audit.md` P1-39, `todo`). Those are known, and none of them is this operator's. Three \
    more, each measured above. **(a) The graph leg** — the only path that consults the fusion \
    floor — records a GEMV and runs it once the step's result is collected \
    directly, so the collect spelling is the only thing that stood between this operator and the \
    fusion floor. **(b) The device leg** used to refuse a GEMV whose \
    reduction is shorter than its output; that rule is gone and this program runs on the device \
    (4 of 4, bit-exact), so the decode shape is not blocked by a length. **(c) A position to advance.** \
    A GEMV carries no position of its own — `t` belongs to the cache kernel — but the pair needs \
    one to advance, and the language holds no mutable state between calls. Beyond those three, \
    nothing: the arithmetic, the layout and the chaining are all expressible today, as measured.\n\n\
    **One workaround, and it is a preference.** The lane enumerates the outputs directly, `m` \
    being the lane index, and never takes a coordinate pair. It could have enumerated `M × 1` and \
    taken `m` from `compute.row k`, which is the spelling `examples/matmul.lichen` and \
    `examples/attention.lichen` use and which would have exercised the second extent — but `m` is \
    the row of a one-row matrix, and asking for it costs an instruction per lane to learn \
    something the lane already knows. It is **a preference**: the rule underneath is D17's, that a \
    write is the value of a position and so one lane per output and one write per lane, and that \
    rule is satisfied just as well by the coordinate spelling.\n\n\
    **The arithmetic is exact, and the declared output is worked out by hand.** `W` is \
    `1 2 3 / 4 5 6 / 7 8 9 / 10 11 12` (four rows of three, row-major flattened, because the \
    language has no two-dimensional array) and `x` is `1 2 3`, so `y[0] = 1·1 + 2·2 + 3·3 = 14`, \
    `y[1] = 4·1 + 5·2 + 6·3 = 32`, `y[2] = 7·1 + 8·2 + 9·3 = 50` and `y[3] = 10·1 + 11·2 + 12·3 = \
    68`. Every element is an integer below `2^24` and `f32` represents every such integer exactly, \
    so there is no approximation anywhere in this file: there is no `exp` and no `sqrt` in the \
    body, and **nothing here is modelled** — unlike SiLU, softmax, LayerNorm and attention, whose \
    declared values are the language's own transcendental sequences. The declared output is that \
    arithmetic worked out by hand, not a transcription of what the program printed. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[14.0, 32.0, 50.0, 68.0]: array{Float, 4}"
---
Gin  = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 12))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
Gout = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Par  = struct{.n Int, .kdim Int, .in Gin}
Arg  = struct{.n Int, .kdim Int, .I Gin}
gemv_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Gout)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (k.kdim, 0.0 ~ "identity")))))))
}
gemv = compute.parallel (gemv_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Gout
Fill = struct{.n Int}
wBuf = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 12)))}
fill_w_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 12))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0) + (if i == 6 then 7.0 else 0.0) + (if i == 7 then 8.0 else 0.0) + (if i == 8 then 9.0 else 0.0) + (if i == 9 then 10.0 else 0.0) + (if i == 10 then 11.0 else 0.0) + (if i == 11 then 12.0 else 0.0)
  ((wBuf)(.w (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_w = compute.parallel fill_w_raw "cpu" wBuf
w = (compute.plrun fill_w ((Fill)(.n 12)) : wBuf).w
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 3)) : xBuf).x
out = (compute.plrun gemv ((Arg)(.n 4, .kdim 3, .I Gin(.w w, .x x))) : Gout)
compute.collect out.y : array{Float, 4}
```

### `decode.lichen`

```text
---
  order = "24"
  compute = import "compute.lichen"
  doc = "One decode step, chained: `examples/kvcache.lichen`'s append, `examples/gemv.lichen`'s \
    score, `examples/softmax.lichen`'s normalise, and `examples/attention.lichen`'s weighted \
    values, in that order, each reading the buffer the last one wrote. **Four stages, five \
    kernels, one query, `capacity = 4`, `dim = 2`, `t = 2`.** Every example below this one on \
    the ladder in `docs/notes/llm-operators.md` is one operator or one operator's pair, and the \
    two slices before this one each measured a wall in the **hand-off** between two. This file \
    is the first program here whose subject is the hand-off itself, and what it found is that \
    the hand-off is cheap — the transposed half is not.\n\n\
    **The data, and why it is not the KV-cache example's.** The cache is `K = 1 2 / 3 4 / 0 0 \
    / 0 0` and `V = 5 6 / 7 8 / 0 0 / 0 0`, two filled positions of four; the new rows are \
    `nk = 1 1` and `nv = 30 40`; the query is `q = 1 0`. So the appended cache is `K' = 1 2 \
    / 3 4 / 1 1 / 0 0` and `V' = 5 6 / 7 8 / 30 40 / 0 0`. **The new key row is small on \
    purpose.** A large one scores highest, the softmax collapses onto it, and the weighted row \
    comes back as the new value row with the other three positions invisible in `f32` — a true \
    fact about attention and a useless example. Here the four scores are `1 3 1 0` and the \
    answer is a genuine blend of all four positions: `8.888384` and `10.773098`, against the \
    `30` and `40` any single row holds.\n\n\
    **The two address forms, and the transpose is the whole story.** The cache is \
    **position-major**: `K[p][d]` sits at `p · dim + d`. `examples/gemv.lichen`'s matrix is \
    **output-major**: `W[m · kdim + k]`, the output index on the outside. A decode step reads \
    **one** buffer at two different axes, and which axis the fold runs over is what decides \
    the form. Stage 2, the score, uses GEMV's own form `i · kdim + (s(0) - 1)` **unchanged**: \
    the output is the position, one lane per cached key, so `i` is the position and the fold \
    runs down the row over the head dimension. That is GEMV's layout exactly, with `m` = \
    position and `kdim` = `dim` — the cache *is* this operator's matrix. Stage 4, the weight, \
    needs `examples/attention.lichen`'s form `(s(0) - 1) · dim + column`: the output is the \
    head-dimension column and the fold runs over the position, so the position is on the \
    outside and the column indexes inside the row. **Measured, and the failure is silent:** \
    writing stage 4 at stage 2's form, `column · keys + (s(0) - 1)`, answers `[6.0754085, \
    33.36727]` against this file's `[8.888384, 10.773098]` — wrong in both elements, **no \
    error, no refusal, no report line**, and a plausible-looking row. The language has no \
    spelling that transposes a buffer, so this transpose is the caller's, by hand, at every \
    call site.\n\n\
    **The hand-offs: what each stage's buffers must be shaped like.** A `compute.write` names \
    a slot of a buffer **exactly as long as the launch's extent** \
    (`crates/lichen-compute/src/compute.rs:5979`) and `compute.collect` hands back **every** \
    element of it (`:1550-1621`), so a stage's output buffer is as long as its own extent and \
    not a byte more. Nothing between the stages reshapes a buffer: each `plrun`'s output \
    struct field feeds the next argument's input struct field by name, and the shapes are \
    this — stage 1 over `capacity · dim = 8` lanes, reading `k` and `v` at 8 and `nk` and `nv` \
    at **2**, writing `k` and `v` at 8; stage 2 over `capacity = 4` lanes, reading `cached.k` \
    at 8 and `q` at **2**, writing 4; stages 3a and 3b over 4 lanes each, reading and writing \
    4; stage 4 over `dim = 2` lanes, reading `weights.y` at 4 and `cached.v` at 8, writing 2. \
    **Two of those rows are the whole cache, and both are the rule \
    `examples/kvcache.lichen` measures.** Stage 2 reads all eight key slots (`i · kdim + \
    s(0) - 1` over `i` in `0 … 3`) and stage 4 reads all eight value slots **including the \
    two unfilled positions**, which is why the appended cache cannot be handed on as its \
    filled prefix `[1 2 3 4]`: the device used to refuse a short host input and the host \
    zero-filled it silently; the rule was removed (the owner's decision), so both answer `0` past \
    the prefix now. The chain still rewrites the whole buffer rather than one row, because a write \
    past the count is dropped in silence. **Only the query is short, and only by `dim`.**\n\n\
    **The two config spellings coexist, and which one a kernel can use is decided by its leaf \
    count.** Stages 3a and 3b are `examples/softmax.lichen`'s two kernels **verbatim**, and \
    they carry `compute.P (compute.KT Float)(.I Row, .O Maxs)` — a config holding **one** \
    leaf, `.n` (`crates/lichen-compute/src/compute.lichen:5`). Stages 1, 2 and 4 need four, \
    two and three leaves, so they spell the config out as a `struct` with `.n` first and \
    `.in`/`.out` last, the way `examples/gemv.lichen` and `examples/kvcache.lichen` do. Both \
    spellings compile in the same file and either may be used at any hand-off: what decides \
    it is only how many leaves the body reads.\n\n\
    **The two positional rules, and one of them is now measurable through a chain.** Leaf \
    **one** is the launch extent whatever it is called (`scalar_leaf_classes`, \
    `crates/lichen-compute/src/compute.rs:1849`); leaf **two** is what `compute.row` and \
    `compute.col` divide by (`crates/lichen-compute/src/compute/body.rs:788-815`) and is \
    reached only from a coordinate, so it is free in a body that takes none. Every multi-leaf \
    config here keeps the extent first and puts a new leaf after the divisor: `ParK` is `.n, \
    .cap, .dim, .t` and `ParW` is `.n, .dim, .keys`. **Stage 2 measures the rule through the \
    chain:** putting `.kdim` **first** in `ParS` launches the score kernel over **2** lanes with \
    `n = 4` as the stride, the scores buffer holds 4, the row maximum is taken over four slots \
    of which two read past the buffer and answer `0` (`compute.rs:6234`), and the chain still \
    answers a plausible row — `[7.425879, 8.748409]`, from scores `[1.0, 3.0]` — **with no \
    refusal anywhere**. The rule bites through a chain, and a chain hides it *worse* than a \
    single kernel does: the wrong scores are still a softmax, and the wrong softmax is still a \
    weighted sum. **Stage 4 cannot show it, and that is the shape rather than the data:** its \
    extent and its divisor are both `2`, because a one-query decode step's output extent **is** \
    the head dimension, so no reordering of `.n` and `.dim` is observable here. A decode step \
    with two queries would not have that luck.\n\n\
    **What chaining cost.** Stages 3a and 3b are `examples/softmax.lichen`'s two kernels \
    **character for character**, measured by diffing each body out of the two files. Stages 1 \
    and 2 are their example's kernel with the binder and its parameter renamed and **nothing \
    else** — `append_raw` over `ParK` for `append_raw` over `Par`, `score_raw` over `ParS` for \
    `gemv_raw` over `Par`, the binder because two `gemv_raw`s in one file would be one \
    binding. **Only stage 4 changed**, and it changed by losing two of \
    `examples/attention.lichen`'s three nests. `attend` carries `top`, the row maximum, `den`, \
    the denominator, and `acc`, the weighted sum, because it is the **only** kernel there. In \
    a chain the softmax is already a stage, so `weigh` keeps `acc` alone and reads the \
    precomputed weight at `a[s(0) - 1]` where `attend` read `exp (s - m)`. **That change is not \
    free, and the cost is in the last bit: `[8.888384, 10.773098]` against `[8.888383, \
    10.773099]`** — this file's stage 4 beside `examples/attention.lichen`'s `attend` run \
    verbatim over the same scores and the same appended `V'`, measured on `cpu`. The reason is \
    **where the division sits**: hoisted, each of the `keys` weights is divided once and the \
    weighted sum under it is exact; inline, one division at the write divides an accumulated \
    sum. Neither is wrong, and a chain has to **choose** — so this file's declared row is the \
    hoisted spelling's, and a step `n + 1` built on the other would not reproduce it. That is \
    the first measurement on the ladder of a chain **choosing between two correct answerings \
    of the same operator**.\n\n\
    **The claim the values half now makes is the one attention's floor measurement names.** \
    With the softmax hoisted there is no `exp` left to mark, and this node's written value \
    **is** the fold — which is exactly the shape `docs/notes/llm-operators.md` measures \
    refusing the fusion floor as `ElementWithoutProducer` and `BeyondANode`, *here `element` is \
    the whole fold, so no element expression is left for a producer's value to flow into*. \
    This file marks `element` on the fold result for attention's reason. **That refusal is \
    cited, not re-measured here**, and with no floor installed nothing in this file consults \
    one.\n\n\
    **What a decode loop still needs, beyond the walls the two examples before this one \
    name.** The known ones: the language has **no iteration form at all** — no `for`, no \
    `while`, no loop keyword in any example — `@loop` is a fold **inside one lane**, \
    `compute.call` inside a parallel body is refused with a `NodeId` (`code-audit.md` P1-39, \
    `todo`), and nothing advances a position. Four more, and only the first is a wall. (a) **A \
    position to write into** — `t` is a leaf of stage 1's argument, written out as the literal \
    `2`, and step `n + 1` needs `t + 1`, which the language cannot produce because it holds \
    no mutable state between calls. Nothing in this file's operator has to change for it to \
    land, which is why the chain is worth landing first. (b) **Carried state is expressible \
    inside one program and nowhere else.** A `plrun`'s output is a buffer a later `plrun` \
    names as an input — this file proves it five times over, and `examples/silu.lichen` does \
    it across two kernels — but a name that survives the step is not the same thing: `cached` \
    is a binding in *this* term, so an `N`-step spelling repeats this program's shape `N` \
    times with the cache literal and the incoming-row literal written out `N` times. \
    **Chaining inside one program is free; chaining across programs is where the missing \
    state is felt**, and that is what this file adds to the two slices before it: they \
    measured the wall, this one measures where it bites. (c) **Nothing is reused** — every step \
    allocates its own score, weight and output buffers and `collect` hands back every element \
    of each, so `N` steps is `3N` buffers and none of them can be named again. (d) **`dim` is \
    one number carried as three unrelated literals**: stage 1's `.dim`, stage 2's `.kdim` and \
    stage 4's `.dim`, and nothing compares them. Stage 2's `kdim` is the *stride* `i · kdim + \
    s(0) - 1`, so a `kdim` that is not stage 1's `dim` reads the cache at the wrong pitch and \
    the score is wrong **silently**; a real decode step carries one number and the language \
    has no spelling for a number that is one number.\n\n\
    **The device leg, which this file cannot reach, and what it would meet.** The CLI installs \
    no compute backend, so flipping every kernel in this program to `gpu` answers by name — \
    *this parallel kernel was compiled for the `gpu` backend, but no compute backend is \
    installed* — **measured, this program**. The walls the two predecessors measured are both \
    **gone**: the length rule was removed (the owner's decision) and the shape \
    `examples/gemv.lichen` measured is no longer refused, so **this chain now runs on the device** \
    — 1 of the 2 declared elements differs by **1 ulp**, the `exp`-and-division family §7 of \
    `docs/notes/device-operator-sweep.md` attributes, not a length. The hypothesis below that a \
    chained step might reach a device before a lone GEMV is moot: both do.\n\n\
    **The graph leg, which used to refuse this chain at two more places.** `RaggedInputs` \
    (`crates/lichen-graph-ir/src/run.rs`, **removed with the length rule**) compared \
    **payload bytes** across a node's inputs, and **no two stages of this chain have two \
    inputs of one length**: stage 1 is 8 against 2, stage 2 is 8 against 2, stage 4 is 4 \
    against 8. What stays is the compile-order class defect \
    (`compute.rs:2424-2430`'s `unwrap_or(ScalarClass::Int)`), measured on attention's \
    two-kernel chain over a **recorded** graph, which is this chain's shape with a second \
    query, so it is not a sixth wall here but **the same one**. What this chain adds to the \
    ladder is the number of *distinct* shapes a real step has to satisfy at once — a \
    four-input append, a two-input GEMV and a two-input weighted sum in one program.\n\n\
    **What is exact and what is modelled.** **Exact, by hand and in `f32`:** the append, every \
    element copied or replaced with integers below `2^24` and no `exp` and no `sqrt` in it; \
    the four dot products `1`, `3`, `1`, `0`, every term a product of integers under 30; the \
    row maximum `3.0`; the offsets `score - max`, which are the **exact** integers `-2, 0, -2, \
    -3`; `exp(0) = 1.0` **exactly**, because the polynomial at a zero argument is `1.0` and \
    seven squarings of `1.0` are `1.0`; and the four `weight · V` products and their sum, \
    **given** the weights. **Modelled:** `exp(-2)` and `exp(-3)`, which are the degree-10 \
    Taylor sum of `docs/notes/floating-point.md` §6 — clamped at `-128`, Horner over `x / \
    2^7`, seven squarings — and **never** a reference `eˣ`; and everything downstream of them, \
    the denominator `1.3204572`, the four weights `[0.10249112, 0.75731343, 0.10249112, \
    0.03770435]`, and the declared row. **The declared output was derived independently**, by \
    re-implementing that published sequence in `f32` outside the workspace, never calling the \
    crate under test, and by reproducing the fold order the bodies actually have: an `@loop` \
    counts **down**, so the denominator accumulates `exp(-3)`, `exp(-2)`, `exp(0)`, \
    `exp(-2)` in that order, and the values fold accumulates `a[3] · V'[3]`, `a[2] · V'[2]`, \
    `a[1] · V'[1]`, `a[0] · V'[0]` in that order. Every intermediate agrees with what the \
    program prints: `K' = [1.0, 2.0, 3.0, 4.0, 1.0, 1.0, 0.0, 0.0]`, `V' = [5.0, 6.0, 7.0, \
    8.0, 30.0, 40.0, 0.0, 0.0]`, `scores = [1.0, 3.0, 1.0, 0.0]`, `maxima = [3.0, 3.0, 3.0, \
    3.0]`. **For scale, the same chain over a reference `eˣ` in `f64` gives `[8.8883843, \
    10.7731007]`**: the degree-10 Taylor costs about half an ulp in the first element and \
    three in the second, which is its own error and not the fold order's.\n\n\
    **Two workarounds, and neither is a rule.** Stage 4 does **not** reuse \
    `examples/attention.lichen`'s `attend`; it reuses only the `acc` nest, and `top` and \
    `den` are gone because stage 3 already computed what they computed. It could have kept \
    them and still been a chain, at the cost of the same softmax twice and the last-bit \
    difference measured above. **A preference**: the honest reason is that a chain should not \
    compute a value twice, and the measured cost of the preference is that the choice is \
    **visible in the answer**, which is why it has to be written down. Stage 4 also takes its \
    column from `compute.col k` rather than from `compute.range k.n`, which for one query is \
    the same number. **A preference too, cutting the other way**: the lane already knows its \
    column, so the coordinate buys an instruction — but it is what makes `.keys` a legal \
    **third** leaf, and it is attention's spelling. The rule underneath both is D17's, that a \
    `compute.write` is **the value of an output position and not an effect** (`code-audit.md` \
    P1-42, `wontfix:D17`), which is why every stage here is one lane per output with exactly \
    one write, and which is also why the append rewrites the whole cache rather than one row. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`). The append's freshness test is an **arithmetic select** rather than a conditional over a read: the surface refuses a conditional whose arm reads a buffer, so the old value and the new value are read unconditionally and the test picks between them with a factor. The fold's trip count is the launch's extent read as `k.n + 0`: the extent's own node cannot carry a loop's count (`docs/notes/loop-conversion.md`), and the arithmetic leaves it the same number."
  output = "[8.888384, 10.773098]: array{Float, 2}"
---
KVIn  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2))), .nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
KVOut = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
ParK  = struct{.n Int, .cap Int, .dim Int, .t Int, .in KVIn}
ArgK  = struct{.n Int, .cap Int, .dim Int, .t Int, .I KVIn}
append_raw = (k : ParK) => {
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  row_k = (compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)
  row_v = (compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)
  fresh = position == k.t
  new_k = old_k + ((row_k - old_k) * (if fresh then 1.0 else 0.0))
  new_v = old_v + ((row_v - old_v) * (if fresh then 1.0 else 0.0))
  b_k = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  b_v = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((KVOut)(.k (compute.write ((compute.Write _)(.to b_k, .at slot, .value (new_k ~ "element")))), .v (compute.write ((compute.Write _)(.to b_v, .at slot, .value (new_v ~ "element"))))))
}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "cpu" KVOut
Gin  = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
Gout = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS = struct{.n Int, .kdim Int, .in Gin}
ArgS = struct{.n Int, .kdim Int, .I Gin}
score_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Gout)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (k.kdim, 0.0 ~ "identity")))))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Gout
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Maxs = struct{.m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParM = struct{.n Int, .in Row}
rowmax_raw = (k : ParM) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float)) ~ "combine"))
  b_m = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Maxs)(.m (compute.write ((compute.Write _)(.to b_m, .at i, .value fold (k.n + 0, ((0.0 - 1.0) / 0.0) ~ "identity"))))))
}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity)") "cpu" Maxs
Soft = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParY = struct{.n Int, .in Soft}
softmax_raw = (k : ParY) => {
  i = compute.range k.n
  m = (compute.read ((compute.Read _)(.from k.in.m, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) - m) ~ "element")) ~ "combine"))
  total = fold (k.n + 0, 0.0 ~ "identity")
  here = exp ((compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float) - m)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (here / total))))))
}
softmax = compute.parallel (softmax_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutY
AV   = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
OutW = struct{.o (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
ParW = struct{.n Int, .dim Int, .keys Int, .in AV}
ArgW = struct{.n Int, .dim Int, .keys Int, .I AV}
weigh_raw = (k : ParW) => {
  i = compute.range k.n
  column = compute.col k
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.a, .at (s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from k.in.v, .at ((s(0) - 1) * k.dim + column))) : Float))) ~ "combine"))
  b_o = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutW)(.o (compute.write ((compute.Write _)(.to b_o, .at i, .value ((acc (k.keys, 0.0 ~ "identity")) ~ "element"))))))
}
weigh = compute.parallel (weigh_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutW
Fill = struct{.n Int}
kbufBuf = struct{.kbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_kbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((kbufBuf)(.kbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_kbuf = compute.parallel fill_kbuf_raw "cpu" kbufBuf
kbuf = (compute.plrun fill_kbuf ((Fill)(.n 8)) : kbufBuf).kbuf
vbufBuf = struct{.vbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_vbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 5.0 else 0.0) + (if i == 1 then 6.0 else 0.0) + (if i == 2 then 7.0 else 0.0) + (if i == 3 then 8.0 else 0.0)
  ((vbufBuf)(.vbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_vbuf = compute.parallel fill_vbuf_raw "cpu" vbufBuf
vbuf = (compute.plrun fill_vbuf ((Fill)(.n 8)) : vbufBuf).vbuf
nkBuf = struct{.nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nk_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 1.0 else 0.0)
  ((nkBuf)(.nk (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nk = compute.parallel fill_nk_raw "cpu" nkBuf
nk = (compute.plrun fill_nk ((Fill)(.n 2)) : nkBuf).nk
nvBuf = struct{.nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nv_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 30.0 else 0.0) + (if i == 1 then 40.0 else 0.0)
  ((nvBuf)(.nv (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nv = compute.parallel fill_nv_raw "cpu" nvBuf
nv = (compute.plrun fill_nv ((Fill)(.n 2)) : nvBuf).nv
qBuf = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_q_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 1.0 else 0.0)
  ((qBuf)(.q (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_q = compute.parallel fill_q_raw "cpu" qBuf
q = (compute.plrun fill_q ((Fill)(.n 2)) : qBuf).q
cached = (compute.plrun append ((ArgK)(.n 8, .cap 4, .dim 2, .t 2, .I KVIn(.k kbuf, .v vbuf, .nk nk, .nv nv))) : KVOut)
scored = (compute.plrun score ((ArgS)(.n 4, .kdim 2, .I Gin(.w cached.k, .x q))) : Gout)
topped = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x scored.y))) : Maxs)
weights = (compute.plrun softmax ((compute.A Soft)(.n 4, .I Soft(.x scored.y, .m topped.m))) : OutY)
out = (compute.plrun weigh ((ArgW)(.n 2, .dim 2, .keys 4, .I AV(.a weights.y, .v cached.v))) : OutW)
compute.collect out.o : array{Float, 2}
```

### `decode_loop.lichen`

```text
---
  order = "25"
  compute = import "compute.lichen"
  doc = "Two decode steps, not one: `examples/decode.lichen`'s whole chain run twice by a \
    recursive `drive`, with the cache, the transcript and the step's own output threaded \
    through the recursion's argument. **The declared output has three entries, and each \
    entry is one of the three things the loop is asked for.** The first is the \
    **transcript**: two steps' outputs laid end to end, step zero's at slots 0 and 1 and \
    step one's at slots 2 and 3. The second is **the last step's output alone**, \
    `[11.235222, 13.9678135]`, which is the transcript's second row and the only step \
    output anything outside the recursion can name. The third is **the trip count**, `2`.\n\n\
    **The data is `decode.lichen`'s, unchanged, and that is the point of it.** The cache is \
    `K = 1 2 / 3 4 / 0 0 / 0 0` and `V = 5 6 / 7 8 / 0 0 / 0 0`, the new row is `nk = 1 1` \
    and `nv = 30 40`, the query is `q = 1 0`, `capacity = 4`, `dim = 2`, and the loop starts \
    at `t = 2` — the first unfilled position, the same one `examples/decode.lichen` writes. \
    **So step zero answers exactly what that file declares**, `[8.888384, 10.773098]`, \
    because it *is* that file's program with the step inside a recursion, and step one \
    answers a different row because the cache grew. A loop whose first step reproduces the \
    single-step example bit for bit is a loop whose second step means something.\n\n\
    **The state is the argument tuple, and the trip count is element zero of it.** `drive` \
    takes four arguments and they arrive as `s(0)` through `s(3)`: the step index, the cache \
    (`s(1)`, the `KVOut` of the two rewritten buffers), the transcript (`s(2)`), and the \
    step's own output (`s(3)`). **The step is written once.** The guard is `s(0) == 2` and \
    the base arm is `then s` — the last thing the recursion does is hand back the state it \
    was given — so line 93 is the only place the chain appears. \
    `docs/notes/wall-claims-audit.md` §3 spells the same loop with the step written **twice**, \
    because its base case runs its own `plrun`; returning the state instead is shorter, and \
    it is a **preference**, not a rule: a block is allowed as a `then` arm, measured.\n\n\
    **A launch follows a loop-carried value — a buffer and a scalar alike — and that \
    answers §4's open question.** The append's position leaf is `.t (s(0) + 2)` on line 87: \
    written **inside** the recursion and computed from the function's own state, never handed \
    in. The cache arrives as `s(1).k` and the transcript as `s(2)`. Measured on this machine \
    at `dev@01da5a49`, three spellings, each its own program: a loop-carried **buffer** as \
    an input answers `[1.0, 2.0, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0]`, position 0 then position \
    1; the same loop with the trip count **itself** as `.t s(0)` answers that same row; and \
    with the position **computed** as `.t (s(0) + 1)` it answers \
    `[0.0, 0.0, 1.0, 2.0, 1.0, 2.0, 0.0, 0.0]`, position 1 then position 2. **Each of the \
    two launches got its own value**, which a static read of the cell could not produce. So \
    the shape the ladder has been planning around — *nothing advances a position* — is \
    **narrower than written**: a launch follows a loop-carried scalar today, and \
    `examples/decode.lichen`'s literal `2` is a choice rather than a wall.\n\n\
    **What does not generalise is anything one level removed.** Move the same launch into a \
    helper the recursion calls and it stops following, **silently**: a helper whose parameter \
    is a buffer and whose **every leaf is a literal** collects `raw none` — no error, no \
    refusal, no report line — and the same helper with a scalar leaf is refused loudly, \
    *input buffer 0 holds Float elements, but this kernel reads that position as Int*. A \
    `@gc` block over the same bindings answers `raw none` too, and a `@gc` block **inside** \
    the recursion panics with *invalid SlotMap key used*. **So `raw none` is not the \
    recursion's own symptom**, which is how `docs/notes/wall-claims-audit.md` §4 records \
    it: it is the symptom of a launch whose buffer or scalar has crossed a function \
    boundary or a block scope, and the recursion's own body is exactly where the launch does \
    follow.\n\n\
    **The extent follows the recursion too, so the paragraph that said otherwise is corrected \
    rather than deleted.** It once read *one leaf does not follow even that — the extent*, and \
    gave the measurement: the **first** scalar leaf, the launch extent whatever it is called \
    (`crates/lichen-compute/src/compute.rs:1850`), computed from the state as `.n (s(0) * 8)` \
    collecting an **empty** buffer with no error and no report line. That was measured at \
    `dev@01da5a49` and it **does not reproduce** at `dev@81e45151`. Measured now, on this \
    machine and **host-only**: the extent computed from the state as `.n (s(0) * 2 + 2)` runs, \
    and with a body that writes its own lane index it answers `[0.0, 1.0, 2.0, 3.0]` — step \
    zero asked for two lanes and step one for four, so the length is the extent the recursion \
    asked for and not a literal. **A scalar read back out of a buffer is an extent too**, at \
    the price of a host round trip per step: `n = float2int (compute.collect stated.g : \
    array{Float, 1})[0]` launches three lanes and answers `[3.0, 7.0, 0.0]`. **Every leaf of \
    a launch follows the recursion**, and the one place a count may not is a **recorded** \
    dispatch inside `compute.graph`, where it must be a literal or one of the graph function's \
    own arguments (`crates/lichen-compute/src/compute.rs:4885`). **What is left is a rule and \
    a ceiling, not a wall**: the extent is still read as an `Int`, and a count past `2^20` is \
    refused before allocation.\n\n\
    **The trip count is not hidden, which §3's stated cost says it is.** \
    `docs/notes/wall-claims-audit.md` §3 records the cost of the loop as *the trip count is \
    a function's own state, so nothing else in the program can see it*. **The third entry \
    of this file's declared output is `2`, read as `final(0)` outside the recursion.** An \
    argument tuple is ordinary data and the recursion's answer is an ordinary value, so what \
    is actually true is narrower and the other way round: the trip count is **decided by a \
    literal in the recursion's own body**, and nothing outside can read or move it *while \
    the loop runs*. Replacing that literal with a top-level `Int` binding is refused — \
    *table lookup missed — no entry for this key* — against a control whose only difference \
    is the literal, which runs. **What the loop can be asked afterwards is how far it got, \
    not what it should have done.**\n\n\
    **Collecting every step's output is the one thing the loop does not give for free, and \
    the cost is the extent rule.** `weigh` launches over `dim = 2` and writes two slots, so \
    it cannot also write a four-slot transcript: a `compute.write` names a slot of an output \
    buffer **exactly as long as the launch's extent** \
    (`crates/lichen-compute/src/compute.rs:5979`), and a write past that span is dropped in \
    silence — the rule `examples/kvcache.lichen` measures, cited here rather than \
    re-measured. **So the sixth kernel, `keep`, is that rule's spelling and nothing else.** \
    It launches over the transcript's four slots and every lane writes **the value of its \
    own slot** (`code-audit.md` P1-42, `wontfix:D17`): the step's output where \
    `base <= lane < base + dim`, the old transcript everywhere else. Only the last step's \
    `out` is reachable by name; **step zero's survives solely because a buffer was threaded \
    for it**, and a loop that threaded none would answer one row.\n\n\
    **The hand-offs are `decode.lichen`'s, plus one new row.** Stage 1 over \
    `capacity · dim = 8`, stage 2 over 4, stages 3a and 3b over 4 each, stage 4 over 2 — the \
    same five shapes, the same position-major cache, the same two address forms, the same \
    whole-cache rule and the same two config spellings, all cited there rather than restated. \
    **The sixth row is new**: `keep` launches over the transcript's four slots, reads `x` at \
    4 and `o` at **2**, and writes 4 — the first kernel in this ladder whose output is \
    longer than either input. Its leaves are `.n, .j, .dim`: the extent first, and because \
    the body takes no coordinate the second leaf is an ordinary runtime scalar wherever it \
    sits, which `examples/gemv.lichen` measures rather than this file. **Its device leg used to \
    be expected to refuse**, on the predecessors' `InputShorterThanCount` reading of the \
    two-element step buffer; **that rule was removed and the leg is measured now: the program \
    runs on the device and 3 of its 7 elements differ by 1 ulp** — the `exp`-and-division family, \
    not a length.\n\n\
    **What a real decode loop still lacks, and which of it is a wall.** (a) **Sampling — no \
    longer `ln`.** Turning a step's output row into a token needs `ln` over the logits, and it \
    is a **named operator** now (`82640443`, `examples/ln.lichen`): two of the three blockers \
    once recorded here are gone — `floor` is a `KernelInstr`, and the reduction needs neither a \
    shift nor a bitcast, being the power-of-two fold `sqrt` already uses. So the wall a sampler \
    operator a sampler needs is not. (b) **A stop condition — not a wall**, corrected by measurement \
    (`docs/notes/llm-operators.md`): a recursive function stops on any value the language computes, \
    including a token a kernel sampled, and what bounds a generated loop is the checker's budget — \
    `500` trip counts unmarked and `2000` marked. (c) **A growing cache — work, and the wall \
    inside it has been measured away.** Carrying a longer cache is a longer buffer and a larger \
    `.n`, both writable today; so is the third thing the old paragraph called unwritable, since \
    an **extent follows the state** — the paragraph above. A fixed large extent plus a mask is \
    therefore **a spelling rather than the only one**, and it costs what it says: on the \
    growing cache `docs/notes/llm-operators.md` measures, it answers the same first three \
    elements over **six** lanes where three were wanted — three times the lanes over the loop — \
    and its `collect` hands back `capacity` elements rather than `filled`. (d) **Per-token \
    rows — work.** `nk`, `nv` and `q` are top-level constants here, so both steps append the \
    same row and ask the same question; making them vary is the launch-follows-a-loop-carried-\
    buffer measurement read the other way, since they would be loop-carried buffers.\n\n\
    **What is exact and what is modelled.** **Exact, in `f32`:** the append, every element \
    copied or replaced being an integer below `2^24` and no `exp` or `sqrt` in it; the dot \
    products `1, 3, 1, 0` then `1, 3, 1, 1`, every term a product of integers under 40; the \
    row maximum `3.0`; the offsets `score - max`, which are the **exact** integers \
    `-2, 0, -2, -3` and then `-2, 0, -2, -2`; `exp(0) = 1.0` **exactly**, because the \
    polynomial at a zero argument is `1.0` and seven squarings of `1.0` are `1.0`; and \
    every `weight · V` product and sum, **given** the weights. **Modelled:** `exp(-2)` and \
    `exp(-3)` by the degree-10 Taylor sum of `docs/notes/floating-point.md` §6 — the \
    argument clamped at `-128`, Horner over `x / 2^7`, seven squarings — and **never** a \
    reference `e^x`; and everything downstream of them. **The declared output was derived \
    independently**, by re-implementing that published sequence in `f32` outside the \
    workspace, never calling the crate under test, and by reproducing the fold order the \
    bodies have: an `@loop` fold counts **down**, so step zero's denominator accumulates \
    `exp(-3), exp(-2), exp(0), exp(-2)` and its values fold accumulates `a[3]·V'[3]`, \
    `a[2]·V'[2]`, `a[1]·V'[1]`, `a[0]·V'[0]`, while step one's denominator accumulates \
    `exp(-2)` three times and then `1.0`. Every intermediate agrees with what the program \
    prints: step zero's `scores = [1.0, 3.0, 1.0, 0.0]` and \
    `maxima = [3.0, 3.0, 3.0, 3.0]`, step one's `[1.0, 3.0, 1.0, 1.0]` and the same \
    maxima, and the weights are `[0.10249112, 0.75731343, 0.10249112, 0.03770435]` then \
    `[0.09625506, 0.7112348, 0.09625506, 0.09625506]`. **For scale, the same arithmetic over \
    a reference `e^x` in `f64` gives `[8.8883843, 10.7731007]` for step zero** — the \
    degree-10 Taylor costs about half an ulp in the first element and three in the second, \
    which is its own error and not the fold order's — and step one's weights as \
    `[0.09625513525746869, 0.7112345942275937, ...]` against the modelled row above.\n\n\
    **Three workarounds and two of them are rules.** (1) **The transcript is a sixth kernel \
    rather than a wider `weigh`** — a **rule**, the extent rule named above. (2) **`keep` \
    reads the step's output at `lane % k.dim`, not at `lane - base`** — a **rule**: on the \
    lanes where `lane - base` leaves the two-element buffer the address would be out of \
    range altogether, which answers `0` past a host buffer's end \
    (`crates/lichen-compute/src/compute.rs:6234`), which the device now answers the same way \
    (the length refusal is gone), so the address is made total instead. (3) **The chain lives in the recursion's body and is not \
    factored into a named `step` function** — a **rule**, measured above: the helper collects \
    `raw none` in silence, which is the worst of the three failures because nothing says so. \
    **Two things in this file are preferences and are named as such because neither is \
    forced:** the base arm returns the state rather than repeating the step, and the incoming \
    row and query are the same every step because a varying one would need buffers threaded \
    for a reason this file does not otherwise exercise. The rule underneath all of it is \
    D17's, that a `compute.write` is **the value of an output position and not an effect**, \
    which is why `keep` writes every one of the transcript's four slots every step and takes \
    the old value back where the step does not reach. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`). The append's freshness test is an **arithmetic select** rather than a conditional over a read: the surface refuses a conditional whose arm reads a buffer, so the old value and the new value are read unconditionally and the test picks between them with a factor. The fold's trip count is the launch's extent read as `k.n + 0` (`docs/notes/loop-conversion.md`), and the lane's column is `lane - (lane / k.dim) * k.dim` rather than `lane % k.dim`, which cannot be placed."
  output = "([8.888384, 10.773098, 11.235222, 13.9678135], [11.235222, 13.9678135], 2): tuple{array{Float, 4}, array{Float, 2}, Int}"
---
KVIn  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2))), .nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
KVOut = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
ParK  = struct{.n Int, .cap Int, .dim Int, .t Int, .in KVIn}
ArgK  = struct{.n Int, .cap Int, .dim Int, .t Int, .I KVIn}
append_raw = (k : ParK) => {
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  row_k = (compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)
  row_v = (compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)
  fresh = position == k.t
  new_k = old_k + ((row_k - old_k) * (if fresh then 1.0 else 0.0))
  new_v = old_v + ((row_v - old_v) * (if fresh then 1.0 else 0.0))
  b_k = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  b_v = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((KVOut)(.k (compute.write ((compute.Write _)(.to b_k, .at slot, .value (new_k ~ "element")))), .v (compute.write ((compute.Write _)(.to b_v, .at slot, .value (new_v ~ "element"))))))
}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "cpu" KVOut
Gin  = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 8))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
Gout = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS = struct{.n Int, .kdim Int, .in Gin}
ArgS = struct{.n Int, .kdim Int, .I Gin}
score_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Gout)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (k.kdim, 0.0 ~ "identity")))))))
}
score = compute.parallel (score_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Gout
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Maxs = struct{.m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParM = struct{.n Int, .in Row}
rowmax_raw = (k : ParM) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float)) ~ "combine"))
  b_m = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Maxs)(.m (compute.write ((compute.Write _)(.to b_m, .at i, .value fold (k.n + 0, ((0.0 - 1.0) / 0.0) ~ "identity"))))))
}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity)") "cpu" Maxs
Soft = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .m (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
OutY = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParY = struct{.n Int, .in Soft}
softmax_raw = (k : ParY) => {
  i = compute.range k.n
  m = (compute.read ((compute.Read _)(.from k.in.m, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (exp ((compute.read ((compute.Read _)(.from k.in.x, .at s(0) - 1)) : Float) - m) ~ "element")) ~ "combine"))
  total = fold (k.n + 0, 0.0 ~ "identity")
  here = exp ((compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float) - m)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutY)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (here / total))))))
}
softmax = compute.parallel (softmax_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutY
AV   = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 4))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
OutW = struct{.o (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
ParW = struct{.n Int, .dim Int, .keys Int, .in AV}
ArgW = struct{.n Int, .dim Int, .keys Int, .I AV}
weigh_raw = (k : ParW) => {
  i = compute.range k.n
  column = compute.col k
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.a, .at (s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from k.in.v, .at ((s(0) - 1) * k.dim + column))) : Float))) ~ "combine"))
  b_o = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutW)(.o (compute.write ((compute.Write _)(.to b_o, .at i, .value ((acc (k.keys, 0.0 ~ "identity")) ~ "element"))))))
}
weigh = compute.parallel (weigh_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutW
KTIn  = struct{.o (compute.Buf ((compute.BufParam _)(.type Float, .length 2))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
KTOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParT  = struct{.n Int, .j Int, .dim Int, .in KTIn}
ArgT  = struct{.n Int, .j Int, .dim Int, .I KTIn}
keep_raw = (k : ParT) => {
  lane = compute.range k.n
  base = k.j * k.dim
  held = (compute.read ((compute.Read _)(.from k.in.x, .at lane)) : Float)
  here = (compute.read ((compute.Read _)(.from k.in.o, .at (lane - (lane / k.dim) * k.dim))) : Float)
  fresh = (lane >= base) & (lane - base < k.dim)
  mine = held + ((here - held) * (if fresh then 1.0 else 0.0))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((KTOut)(.y (compute.write ((compute.Write _)(.to b_y, .at lane, .value (mine ~ "element"))))))
}
keep = compute.parallel (keep_raw ~ "claim:Reduction(element)") "cpu" KTOut
Fill = struct{.n Int}
kbufBuf = struct{.kbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_kbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((kbufBuf)(.kbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_kbuf = compute.parallel fill_kbuf_raw "cpu" kbufBuf
kbuf = (compute.plrun fill_kbuf ((Fill)(.n 8)) : kbufBuf).kbuf
vbufBuf = struct{.vbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 8)))}
fill_vbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 8))
  v = (if i == 0 then 5.0 else 0.0) + (if i == 1 then 6.0 else 0.0) + (if i == 2 then 7.0 else 0.0) + (if i == 3 then 8.0 else 0.0)
  ((vbufBuf)(.vbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_vbuf = compute.parallel fill_vbuf_raw "cpu" vbufBuf
vbuf = (compute.plrun fill_vbuf ((Fill)(.n 8)) : vbufBuf).vbuf
nkBuf = struct{.nk (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nk_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 1.0 else 0.0)
  ((nkBuf)(.nk (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nk = compute.parallel fill_nk_raw "cpu" nkBuf
nk = (compute.plrun fill_nk ((Fill)(.n 2)) : nkBuf).nk
nvBuf = struct{.nv (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_nv_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 30.0 else 0.0) + (if i == 1 then 40.0 else 0.0)
  ((nvBuf)(.nv (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nv = compute.parallel fill_nv_raw "cpu" nvBuf
nv = (compute.plrun fill_nv ((Fill)(.n 2)) : nvBuf).nv
qBuf = struct{.q (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_q_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = (if i == 0 then 1.0 else 0.0)
  ((qBuf)(.q (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_q = compute.parallel fill_q_raw "cpu" qBuf
q = (compute.plrun fill_q ((Fill)(.n 2)) : qBuf).q
logBuf = struct{.log (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_log_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = 0.0
  ((logBuf)(.log (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_log = compute.parallel fill_log_raw "cpu" logBuf
log = (compute.plrun fill_log ((Fill)(.n 4)) : logBuf).log
spareBuf = struct{.spare (compute.Buf ((compute.BufParam _)(.type Float, .length 2)))}
fill_spare_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 2))
  v = 0.0
  ((spareBuf)(.spare (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_spare = compute.parallel fill_spare_raw "cpu" spareBuf
spare = (compute.plrun fill_spare ((Fill)(.n 2)) : spareBuf).spare
drive = (s : tuple{Int, KVOut, compute.Buf ((compute.BufParam _)(.type Float, .length 4)), OutW}) => if s(0) == 2 then s else { cached = (compute.plrun append ((ArgK)(.n 8, .cap 4, .dim 2, .t (s(0) + 2), .I KVIn(.k s(1).k, .v s(1).v, .nk nk, .nv nv))) : KVOut)
  scored = (compute.plrun score ((ArgS)(.n 4, .kdim 2, .I Gin(.w cached.k, .x q))) : Gout)
  topped = (compute.plrun rowmax ((compute.A Row)(.n 4, .I Row(.x scored.y))) : Maxs)
  weights = (compute.plrun softmax ((compute.A Soft)(.n 4, .I Soft(.x scored.y, .m topped.m))) : OutY)
  out = (compute.plrun weigh ((ArgW)(.n 2, .dim 2, .keys 4, .I AV(.a weights.y, .v cached.v))) : OutW)
  logged = (compute.plrun keep ((ArgT)(.n 4, .j s(0), .dim 2, .I KTIn(.o out.o, .x s(2)))) : KTOut)
  drive (s(0) + 1, cached, logged.y, out) }
final = (drive (0, KVOut(.k kbuf, .v vbuf), log, OutW(.o spare)) : tuple{Int, KVOut, compute.Buf ((compute.BufParam _)(.type Float, .length 4)), OutW})
(compute.collect final(2) : array{Float, 4}, compute.collect final(3).o : array{Float, 2}, final(0))
```

### `mathlib.lichen`

```text
---
  order = "26"
  compute = import "compute.lichen"
  doc = "The elementwise basics, as a library rather than one operator: `scale` (`y = k · x`), \
    `lerp` (`y = a + t · (b − a)`) and `clamp` (`y = max lo (min hi x)`), each one lane per \
    element and each taking its constant as a **`Float` leaf** rather than a literal. Every \
    operator above this file in the ladder is a fold, a coordinate or an address form; these \
    three are the arithmetic they are built out of, and they are what a caller reaches for when \
    the answer is a per-element formula and there is nothing to reduce.\n\n\
    **One lane per element, and the write address is the lane's own index.** A `compute.write` \
    names a slot of an output buffer exactly as long as the launch's extent \
    (`crates/lichen-compute/src/compute.rs:5979`), so `i = compute.range k.n` with `compute.write \
    (... .at i)` is a bijection and the only spelling whose answer does not depend on a schedule. \
    That is also why these operators need **no coordinates**: unlike `examples/matmul.lichen` and \
    `examples/attention.lichen` they never read `compute.row` or `compute.col`, so the second \
    scalar leaf is an ordinary runtime scalar and a `Float` one at that.\n\n\
    **A constant is an argument, and `Float` is a leaf class of its own** (`e24d00fb`): the \
    recorded config carries the first scalar leaf as the launch extent and **every other leaf for \
    what it is**, and the graph's value table holds a float at its own class. So `.k`, `.t`, `.lo` \
    and `.hi` here are pushed as `Float` words beside the extent, and a caller can vary them \
    without recompiling a kernel — which is the whole difference between a library and a set of \
    hard-coded constants. The rule that bites is **positional**: the **second** leaf is what \
    `compute.row`/`compute.col` divide by (`crates/lichen-compute/src/compute/body.rs:788-815`), \
    and these bodies have no coordinates, so the second leaf is free; a new leaf goes after it \
    anyway, as `examples/gemv.lichen` states for the case where it is not.\n\n\
    **Three `plrun`s, and the chain is the library's own composition.** `scale` twice (up and \
    down), `lerp` between them, then `clamp` on the result: four kernels, four buffers, no \
    buffer reused. They are **elementwise**, and this is worth stating because an elementwise \
    chain is the one shape the fusion floor **refuses by name**: `InterfacesAreNotProved`, \
    because an `Elementwise` region carries no licence and has no reduction chain for the floor's \
    own proof to hold (`docs/notes/llm-operators.md`, the SiLU row). These four therefore never \
    merge, and the answer is four dispatches rather than one — correct, and slower than a \
    hand-written fuse would be. That is a measured refusal and not a missing feature.\n\n\
    **The arithmetic is exact, and the declared output is worked out by hand.** `x` is `1 2 3 4`, \
    so `scale` with `k = 2.0` answers `2 4 6 8` and with `k = −1.0` answers `−1 −2 −3 −4`; \
    `lerp` with `t = 0.25` gives `2 + 0.25 · (−1 − 2) = 1.25`, `4 + 0.25 · (−2 − 4) = 2.5`, \
    `6 + 0.25 · (−3 − 6) = 3.75` and `8 + 0.25 · (−4 − 8) = 5`; `clamp` with `lo 2.0`, `hi 5.0` \
    lifts `1.25` to `2` and leaves the rest, so the answer is `2, 2.5, 3.75, 5`. Every value is a \
    quarter or a half below `2^24` and `f32` represents each exactly, so **nothing here is \
    modelled**: there is no `exp`, no `sqrt`, no `ln` and no division, unlike SiLU, softmax, \
    LayerNorm and attention. The declared output is that arithmetic, not a transcription of what \
    the program printed.\n\n\
    **`clamp` is two builtins and no branch.** `max lo (min hi x)` is one instruction each and a \
    `Select`'s two arms are not involved, which matters because a `Select` inside a fold's step \
    is refused (`examples/topk.lichen` records that sentence) and an `if` would have put a branch \
    in a body whose lanes are otherwise identical. `abs` is **not** here for that reason: it is \
    `if x < 0 then -x else x`, a `Select`, and the surface's `Select` refuses when its two arms \
    are computed in different classes (`attention.lichen`'s `scores` refusal, fixed at \
    `b6dea46c`) — so a `Float`-only body would be fine today and an `Int` one would not, and one \
    file should not be the place to find that out. Nothing else in these three operators is \
    conditional.\n\n\
    **The declaration is `cpu`, and the device agrees with it bit for bit.** The sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` runs every example that dispatches a \
    kernel twice, on both backends, and this one answers `agrees, 10 of 10 element(s)` on the RTX \
    3060 — both chains, the fold included. Exact arithmetic is what that should say: every value is \
    a quarter or a half, there is no transcendental and no division, so there is no ulp for the two \
    backends to disagree about.\n\n\
    **The reduction layer: `rowsum`, one lane per row.** A fold cannot be elementwise and cannot be \
    faked by one — `@loop fold = s => ...` counts down **inside a single lane**, so `rowsum` \
    launches over the **rows** (`r = compute.range c.n`) and walks `kdim` elements itself: \
    `y[r] = Sigma_k x[r * kdim + k]`. Its read address is therefore **computed** (`r * kdim + s(0) \
    - 1`) while its write address is still the lane's own index, which is the same split \
    `examples/gemv.lichen` has and the reason that operator needs no coordinates either. The \
    `combine` and `element` marks and the `claim:Reduction(combine,identity,element)` are \
    what the fusion floor reads when a graph records the kernel (`docs/notes/llm-operators.md`); \
    on the `cpu` path they are carried and not consulted. `mean` is then `scale` with \
    `k = 0.25` over the sums and `clamp` bounds the result, because a buffer is a buffer: the \
    reduction layer composes with the elementwise one and neither knows about the other.\n\n\
    **Two more reductions, and the identity is where the modelling creeps in.** `rowmax` is the same \
    fold with `max` for its combine, and `dot` is the same fold reading **two** buffers at the same \
    computed address (`a[...] * b[...]`). The difference between them and `rowsum` is the seed: a sum \
    has `0.0` as a true identity, while a max has **no finite identity**, so `rowmax` takes its seed \
    as a leaf — `.least` — and the caller passes a value below everything in the data \
    (`(0.0 - 100.0)` here). That is the one place this file models something rather than computing it, \
    and it models it *outside* the kernel: a seed too high would silently clip the answer, and the \
    kernel cannot know. `dot`'s fold adds a product per step and its `0.0` seed is honest. Note also \
    that **`floor` is a reserved builtin name**, so the field is `.least`; a struct field spelled \
    `.floor` does not parse, which cost a round to find.\n\n\
    **The third chain, worked out by hand.** `bumps` is `1 -2 3 4 -5 6 7 -8` and `weights` is \
    `1 2 3 4 4 3 2 1`, both two rows of four. `rowmax` answers `4` for the first row (the largest of \
    `1, -2, 3, 4`) and `7` for the second (the largest of `-5, 6, 7, -8`), and the seed `-100` is \
    below every element so it never wins. `dot` answers `1*1 + (-2)*2 + 3*3 + 4*4 = 22` and \
    `(-5)*4 + 6*3 + 7*2 + (-8)*1 = 4`. Every value is an integer below `2^24`, so the arithmetic is \
    exact and the declaration is hand-computed, not transcribed.\n\n\
    **The second chain, worked out by hand.** `grid` is `1` through `8`, `scale` with `k = 2.0` \
    answers `2 4 6 8 10 12 14 16`, `rowsum` over two rows of four gives `2 + 4 + 6 + 8 = 20` and \
    `10 + 12 + 14 + 16 = 52`, `scale` with `0.25` gives `5` and `13`, and `clamp` with `lo 6.0`, \
    `hi 12.0` lifts `5` to `6` and brings `13` down to `12`. Every value is exact for the reason \
    above — quarters and halves, no division — and the two chains are the root as a **tuple**, so \
    both are performed: an unread chain would not be, which is the plain-run gate \
    `docs/notes/graph-recording-rounds.md` records. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`). The buffer types here carry an \
    **undecided length**: one struct type serves buffers of several lengths — `scale` runs over \
    four, eight and two elements — so the length is the launch's count rather than a stated number."
  output = "([2.0, 2.5, 3.75, 5.0], [6.0, 12.0], [4.0, 7.0], [22.0, 4.0]): tuple{array{Float, 4}, array{Float, 2}, array{Float, 2}, array{Float, 2}}"
---
SIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
SOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
SPar = struct{.n Int, .k Float, .in SIn}
SArg = struct{.n Int, .k Float, .I SIn}
scale_raw = (c : SPar) => {
  i = compute.range c.n
  x = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((SOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (x * c.k))))))
}
scale = compute.parallel scale_raw "cpu" SOut
LIn  = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .b (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
LOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
LPar = struct{.n Int, .t Float, .in LIn}
LArg = struct{.n Int, .t Float, .I LIn}
lerp_raw = (c : LPar) => {
  i = compute.range c.n
  a = (compute.read ((compute.Read _)(.from c.in.a, .at i)) : Float)
  b = (compute.read ((compute.Read _)(.from c.in.b, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((LOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (a + c.t * (b - a)))))))
}
lerp = compute.parallel lerp_raw "cpu" LOut
CIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
COut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
CPar = struct{.n Int, .lo Float, .hi Float, .in CIn}
CArg = struct{.n Int, .lo Float, .hi Float, .I CIn}
clamp_raw = (c : CPar) => {
  i = compute.range c.n
  x = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((COut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (max c.lo (min c.hi x)))))))
}
clamp = compute.parallel clamp_raw "cpu" COut
RIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
ROut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
RPar = struct{.n Int, .kdim Int, .in RIn}
RArg = struct{.n Int, .kdim Int, .I RIn}
rowsum_raw = (c : RPar) => {
  r = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.x, .at (r * c.kdim + (s(0) - 1)))) : Float)) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((ROut)(.y (compute.write ((compute.Write _)(.to b_y, .at r, .value (fold (c.kdim, 0.0 ~ "identity")))))))
}
rowsum = compute.parallel (rowsum_raw ~ "claim:Reduction(combine,identity,element)") "cpu" ROut
MIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
MOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
MPar = struct{.n Int, .kdim Int, .least Float, .in MIn}
MArg = struct{.n Int, .kdim Int, .least Float, .I MIn}
rowmax_raw = (c : MPar) => {
  r = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from c.in.x, .at (r * c.kdim + (s(0) - 1)))) : Float)) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((MOut)(.y (compute.write ((compute.Write _)(.to b_y, .at r, .value (fold (c.kdim, c.least ~ "identity")))))))
}
rowmax = compute.parallel (rowmax_raw ~ "claim:Reduction(combine,identity,element)") "cpu" MOut
DIn  = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .b (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
DOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
DPar = struct{.n Int, .kdim Int, .in DIn}
DArg = struct{.n Int, .kdim Int, .I DIn}
dot_raw = (c : DPar) => {
  r = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from c.in.a, .at (r * c.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from c.in.b, .at (r * c.kdim + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((DOut)(.y (compute.write ((compute.Write _)(.to b_y, .at r, .value (fold (c.kdim, 0.0 ~ "identity")))))))
}
dot = compute.parallel (dot_raw ~ "claim:Reduction(combine,identity,element)") "cpu" DOut
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length _))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 4)) : xBuf).x
up = (compute.plrun scale ((SArg)(.n 4, .k 2.0, .I SIn(.x x))) : SOut)
down = (compute.plrun scale ((SArg)(.n 4, .k (0.0 - 1.0), .I SIn(.x x))) : SOut)
mixed = (compute.plrun lerp ((LArg)(.n 4, .t 0.25, .I LIn(.a up.y, .b down.y))) : LOut)
bounded = (compute.plrun clamp ((CArg)(.n 4, .lo 2.0, .hi 5.0, .I CIn(.x mixed.y))) : COut)
gridBuf = struct{.grid (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
fill_grid_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length _))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0) + (if i == 6 then 7.0 else 0.0) + (if i == 7 then 8.0 else 0.0)
  ((gridBuf)(.grid (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_grid = compute.parallel fill_grid_raw "cpu" gridBuf
grid = (compute.plrun fill_grid ((Fill)(.n 8)) : gridBuf).grid
doubled = (compute.plrun scale ((SArg)(.n 8, .k 2.0, .I SIn(.x grid))) : SOut)
rows = (compute.plrun rowsum ((RArg)(.n 2, .kdim 4, .I RIn(.x doubled.y))) : ROut)
means = (compute.plrun scale ((SArg)(.n 2, .k 0.25, .I SIn(.x rows.y))) : SOut)
bounded_rows = (compute.plrun clamp ((CArg)(.n 2, .lo 6.0, .hi 12.0, .I CIn(.x means.y))) : COut)
bumpsBuf = struct{.bumps (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
fill_bumps_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length _))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then (0.0 - 2.0) else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then (0.0 - 5.0) else 0.0) + (if i == 5 then 6.0 else 0.0) + (if i == 6 then 7.0 else 0.0) + (if i == 7 then (0.0 - 8.0) else 0.0)
  ((bumpsBuf)(.bumps (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_bumps = compute.parallel fill_bumps_raw "cpu" bumpsBuf
bumps = (compute.plrun fill_bumps ((Fill)(.n 8)) : bumpsBuf).bumps
weightsBuf = struct{.weights (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
fill_weights_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length _))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 4.0 else 0.0) + (if i == 5 then 3.0 else 0.0) + (if i == 6 then 2.0 else 0.0) + (if i == 7 then 1.0 else 0.0)
  ((weightsBuf)(.weights (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_weights = compute.parallel fill_weights_raw "cpu" weightsBuf
weights = (compute.plrun fill_weights ((Fill)(.n 8)) : weightsBuf).weights
peaks = (compute.plrun rowmax ((MArg)(.n 2, .kdim 4, .least (0.0 - 100.0), .I MIn(.x bumps))) : MOut)
products = (compute.plrun dot ((DArg)(.n 2, .kdim 4, .I DIn(.a bumps, .b weights))) : DOut)
(compute.collect bounded.y : array{Float, 4}, compute.collect bounded_rows.y : array{Float, 2}, compute.collect peaks.y : array{Float, 2}, compute.collect products.y : array{Float, 2})
```

### `sample.lichen`

```text
---
  order = "27"
  compute = import "compute.lichen"
  doc = "Greedy sampling: `pick[r]` is the position of the largest score in row `r` — the rung that \
    turns a decode step's output row into a token. It is **one kernel over one lane per row**, and \
    **nothing in it is modelled**: every score is an integer below `2^24`, the arithmetic is \
    comparisons, additions and multiplications of integers under `4`, and there is no `exp`, no \
    `sqrt`, no `ln` and no division anywhere in the fold. The scores are `3 9 4 9 / 5 1 5 2 / 7 7 7 \
    7 / 8 6 4 2 / 1 2 3 4` and the answer is `[3, 2, 3, 0, 3]`, derived by hand at the end.\n\n\
    **The fold carries a pair, and the pair is not the answer.** The state is three slots — the \
    counter, the best value seen, the position of that value — so the pair travels through the \
    recursion exactly as `examples/attention.lichen`'s `acc` carries its row maximum in `s(2)`. \
    **What this ladder had not had is a fold whose result is a selection**, and here is what the \
    surface allows about it: **the base arm may return one slot of the state, and one only.** A base \
    arm returning the whole state, or a two-slot tuple, is refused — verbatim, *`a kernel body's \
    index cannot be placed: its target is a computation and its index is the constant 1 … the \
    domain is NodeId(135v1) (class NodeId(166v1)) with leaves [NodeId(135v1)]`*. The clause that \
    carries the rule is the last one: **the `@loop`'s domain has one leaf**. That is the \
    conversion's own fact — `crates/lichen-lowlevel/src/loop_conversion.rs:131` builds it from the \
    base arm's value, and `lower_loop_exit` (`crates/lichen-compute/src/compute/body.rs:1195`) \
    emits that one value. So this fold returns the position and **the maximum is unobservable \
    afterwards**; a body that needs both wants two kernels, not a wider tuple.\n\n\
    **A comparison cannot select inside a fold's step, and that is a rule.** The natural spelling — \
    `if (read > s(1)) then (s(0) - 1) else s(2)` — is refused whatever the arms are: a literal \
    arm, a state-read arm, an `Int` arm and a `Float` arm all answer the same way, *`compute.parallel: \
    a kernel body reached a node with neither a value nor an operation, so there is nothing to emit \
    for it (node=NodeId(1707v1))`*, and **that sentence names a cause this is not** — it talks \
    about a `let` alias and a binding the body would fill in at run time. **It is not the mark**: \
    the same body with the `~` mark moved off the selection onto a `max` and a pass-through slot \
    compiles and runs. So the refusal is the language's `Select` in a `@loop` \
    step, and `examples/attention.lichen`'s `if` is therefore **not** a precedent for this: its \
    `attend` folds `max`, `+` and `*`, and its only `if` is the base test that ends the fold.\n\n\
    **So the selection is written as arithmetic on the comparison's `0`/`1`.** A comparison is the \
    language's only boolean and leaves `0` or `1` (`examples/operators.lichen`), so \
    `s(2) + (1 * flag) * ((s(0) - 1) - s(2))` is a lerp: a `flag` of `1` moves the position to \
    `s(0) - 1`, a `flag` of `0` leaves it where it was. **The `1 *` is not decoration — it is what \
    makes the fragment lower.** The same expression without it, `s(2) + (flag * ((s(0) - 1) - \
    s(2)))`, is refused: *`instruction 15 mixed an integer and a float in one operation. Int and \
    Float do not convert in either direction, so nothing here can make the two operands meet: \
    this is a malformed fragment rather than an unsupported shape`*. The two-term blend also runs, \
    `s(2) * (1 - flag) + (s(0) - 1) * flag`, and answers the same row; it is the shape where \
    `1 - flag` happens to put an `Int` operand beside the comparison. **Which operand decides the \
    class is not written down anywhere this file could point at**, so the rule as measured is only \
    the negative one — *a product with the comparison on its left is refused, and the same product \
    with an integer literal on its left is not* — and the honest spelling of it is the `1 *`.\n\n\
    **Ties: the higher position wins, and one character chooses it.** The comparison is `>` and an \
    `@loop` fold counts **down** (`examples/matmul.lichen` and `examples/layernorm.lichen` say so \
    too), so a later visit never displaces an equal value and the **highest** position wins. Write \
    `>=` in both places instead — `>=` is in the grammar (`parse.rs:711`, `KernelBin::Geq`) — and \
    the same program on the same data answers `[1, 0, 0, 0, 3]`, the **lowest** position. Both are \
    measured here. **So it is a choice and not a rule, and it is deterministic for this program \
    while incidental as a property of the operator**: nothing in the language says an argmax prefers \
    the last equal element. What decides it is the conjunction of two independently motivated \
    choices — the countdown order of `@loop` and a strict comparison — and a sampler wanting the \
    lowest position writes `>=` and means something else. **Rows 0, 1 and 2 are tie rows on \
    purpose** — a pair at `1` and `3`, a pair at `0` and `2`, four equal values — and row 3 \
    (`8 6 4 2`) is the control: its maximum is at position `0`, which the fold visits **last**, \
    and it still stands.\n\n\
    **One kernel, one launch, and the extent rule fixes both shapes.** The launch is over the \
    **rows** (`n = 5`), not over the row's width, so every lane writes exactly one position and \
    the output buffer is as long as the launch's extent — `compute.write`'s own rule \
    (`crates/lichen-compute/src/compute.rs:5903-5910`: *the output count is the fragment's \
    `outputs` — a static fact, not a discovery of which slots were written*). **Nothing here needs \
    `examples/decode_loop.lichen`'s `keep`**, because the choice already has one slot per lane. \
    The other spelling — one lane per **element** of the row, the same fold over the whole row, \
    the write aimed at slot `0` — also **runs** and puts the answer in slot `0` (measured: \
    `[3.0, 0.0, 0.0, … ]` over twenty lanes), so the extent rule does not refuse it; it is twenty \
    lanes of the same fold for one number. What the extent rule does forbid is a launch writing \
    **more** slots than its extent, and that is the top-k question below.\n\n\
    **Chaining, measured both ways.** This kernel takes `examples/decode.lichen`'s row verbatim — \
    `.n 1, .w 2` over `[8.888384, 10.773098]` — and answers `[1.0]`, so a decode step's output feeds \
    it with nothing changed, and nothing about the operator is tied to the integer data above. The \
    pair answer also agrees with the **two-`plrun`** spelling — `examples/softmax.lichen`'s \
    `rowmax` kernel, per row, then a position kernel reading the maximum — **but not by itself**. \
    `>=` alone makes every maximum overwrite the last one and answers `[1.0, 0.0, 0.0, 0.0, 3.0]`, \
    the lowest position. Add the guard `(read >= max) * (p == 0)` — `first writer wins`, written \
    without `&` — and it answers `[3.0, 2.0, 3.0, 0.0, 3.0]`, this file's row. **So the tie rule \
    is a property of the body, not of the operator**: two honest spellings of greedy sampling \
    disagree on every tie row until one of them is given an explicit conjunct.\n\n\
    **What top-2 would additionally need, and it is work rather than a wall.** A second rank needs \
    a second fold in the same lane, and that fold must **exclude** the first rank's position — \
    which is where the arithmetic above stops being free. The guard \
    `(read >= s(1)) & ((s(0) - 1) != first)` is refused twice over: `&` answers *`BitAnd has no \
    float form: a kernel's float operators are + - * / and the four order comparisons, not % or \
    the bitwise operators`*, and writing the `and` in `Float` answers the **type checker**, not \
    the kernel, with *`expected Float, found Int`*. **What does run** is the second fold wholly in \
    `Int`: `float2int` on the read, the guard \
    `(1 * (ri >= s(1))) * (1 * ((s(0) - 1) != first))`, and the same lerp for the best. On \
    `[3, 9, 4, 6]` it answers `([1.0], [3.0])` — rank one at `1`, the `9`, rank two at `3`, the `6` \
    — and the two writes must be returned as a **tuple in output-struct order**, or it refuses by \
    name: *`output 0 of the parallel index function is a compute.write to output 1: a write fills \
    the output its .to names, and the codomain's order is not that name`* (`examples/kvcache.lichen` \
    is that spelling). **Three costs are named rather than paid.** The second fold's seed is a \
    finite `0` where the first fold's is `((0.0 - 1.0) / 0.0)`, which is exactly the finite \
    stand-in `examples/softmax.lichen` warns a `max` seed is. The second fold is a **whole extra \
    pass** over the row, so `k` ranks is `k` passes and `k` output buffers rather than one. And \
    `float2int` on the data makes the second rank right only for scores inside the exactly \
    representable integer range. **What was not reached is the one-buffer answer**: a launch of `n` \
    lanes cannot write `k` slots per lane, and nothing concatenates `k` `n`-long buffers. \
    `examples/decode_loop.lichen`'s `keep` is the gather — it launches over the wider extent and \
    reads the step's buffer at `lane % k` — but it needs the ranks already addressable, and \
    getting them there is the step not taken. **Wall or work: work**, with a measured spelling for \
    `k = 2` and an open spelling for the gather.\n\n\
    **What is exact and what is modelled.** **Everything here is exact; nothing is modelled.** The \
    scores are integers below `2^24`, the comparisons are `f32` order comparisons of exactly \
    represented integers, `s(0) - 1`, `(s(0) - 1) - s(2)` and the `0`/`1` products are exact \
    because every value involved is an integer under `4`, `((0.0 - 1.0) / 0.0)` is the signed \
    infinity `BinaryOp`'s invariant gives it rather than a large number, and `int2float` of a \
    position under `4` is exact. The declared row was derived **by hand** before the program was \
    run: row 0 is `3 9 4 9`, the fold visits `3, 2, 1, 0`, `9` beats `-inf` at position `3` and \
    nothing displaces it, so `3`; row 1 is `5 1 5 2`, the `5` at `2` stands and position `0` only \
    ties it, so `2`; row 2 is `7 7 7 7`, position `3` stands, so `3`; row 3 is `8 6 4 2`, position \
    `0` is visited last and still holds `8`, so `0`; row 4 is `1 2 3 4`, every element beats the \
    one before it, so `3`. Two spellings of the tie rule and two of the two-`plrun` chain were run \
    over the same data as the cross-check, and the numbers above are the ones they printed.\n\n\
    **Two leaves, and the second one is free here.** `.n` is the launch extent whatever it is \
    called (`scalar_leaf_classes`, `crates/lichen-compute/src/compute.rs:1850`, and \
    `run_parallel_kernel`'s own *the ABI's first leaf is the launch extent* at `:5917`). `.w` is \
    the **second** leaf, which `compute.row` and `compute.col` divide by \
    (`crates/lichen-compute/src/compute/body.rs:771-795`) and which is reached **only** from a \
    coordinate; this body takes no coordinate, so `.w` is an ordinary runtime scalar wherever it \
    sits — `examples/gemv.lichen` measures that, and it is cited rather than re-measured here. \
    **The rows are row-major flattened, as `examples/decode.lichen` records**: nothing here is a \
    two-dimensional array, `row[0]` starts at `0 · 4 + 0` and `row[4]` at `4 · 4 + 0`, and \
    transposing the input would change every answer.\n\n\
    **Three workarounds, and which of them is a rule.** (1) **The selection is arithmetic rather \
    than a `Select`, and the `1 *` anchor is part of the same refusal** — a **rule**. (2) **The \
    fold's result is the position and the maximum is dropped** — a **rule**, the one-leaf domain. \
    (3) **The launch is over rows rather than over the row** — a **preference**: the wide spelling \
    runs and answers the same number, so nothing forbids it, and one lane per row is chosen \
    because it is one slot per lane with no redundancy. **The rule underneath all three** is \
    `code-audit.md` P1-42's `wontfix:D17`, that a `compute.write` is **the value of an output \
    position and not an effect** — which is why this body is one lane per output with exactly one \ The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[3.0, 2.0, 3.0, 0.0, 3.0]: array{Float, 5}"
---
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 20)))}
Pick = struct{.p (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
Par  = struct{.n Int, .w Int, .in Row}
Arg  = struct{.n Int, .w Int, .I Row}
greedy_raw = (k : Par) => {
  lane = compute.range k.n
  base = lane * k.w
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float)) ~ "combine"), (s(2) + ((1 * (((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  b_p = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Pick)(.p (compute.write ((compute.Write _)(.to b_p, .at lane, .value (int2float (fold (k.w, ((0.0 - 1.0) / 0.0) ~ "identity", 0))))))))
}
greedy = compute.parallel (greedy_raw ~ "claim:Reduction(combine,identity)") "cpu" Pick
Fill = struct{.n Int}
scoresBuf = struct{.scores (compute.Buf ((compute.BufParam _)(.type Float, .length 20)))}
fill_scores_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 20))
  v = (if i == 0 then 3.0 else 0.0) + (if i == 1 then 9.0 else 0.0) + (if i == 2 then 4.0 else 0.0) + (if i == 3 then 9.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 1.0 else 0.0) + (if i == 6 then 5.0 else 0.0) + (if i == 7 then 2.0 else 0.0) + (if i == 8 then 7.0 else 0.0) + (if i == 9 then 7.0 else 0.0) + (if i == 10 then 7.0 else 0.0) + (if i == 11 then 7.0 else 0.0) + (if i == 12 then 8.0 else 0.0) + (if i == 13 then 6.0 else 0.0) + (if i == 14 then 4.0 else 0.0) + (if i == 15 then 2.0 else 0.0) + (if i == 16 then 1.0 else 0.0) + (if i == 17 then 2.0 else 0.0) + (if i == 18 then 3.0 else 0.0) + (if i == 19 then 4.0 else 0.0)
  ((scoresBuf)(.scores (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_scores = compute.parallel fill_scores_raw "cpu" scoresBuf
scores = (compute.plrun fill_scores ((Fill)(.n 20)) : scoresBuf).scores
out = (compute.plrun greedy ((Arg)(.n 5, .w 4, .I Row(.x scores))) : Pick)
compute.collect out.p : array{Float, 5}
```

### `topk.lichen`

```text
---
  order = "28"
  compute = import "compute.lichen"
  doc = "Top-`k`: three ranks per row, gathered into one buffer. The rung after \
    `examples/sample.lichen`, which is greedy sampling — one lane per row and one position out. Here \
    the same row yields its **three** best positions, and the step that was **not taken** there — a \
    buffer every rank is addressable in — is taken here. The scores are that file's five rows \
    unchanged, `3 9 4 9 / 5 1 5 2 / 7 7 7 7 / 8 6 4 2 / 1 2 3 4`, so **kernel one is its \
    `greedy_raw` verbatim** and answers that file's declared row `[3, 2, 3, 0, 3]` bit for bit.\n\n\
    **The declared output has two entries and each is a different address layout of the same three \
    ranks.** `.p` is **rank-major**: rank zero's five, then rank one's five, then rank two's five, \
    end to end in one `15`-slot buffer — `[3, 2, 3, 0, 3, 1, 0, 0, 1, 2, 2, 3, 1, 2, 1]`. `.t` is \
    **row-major**: each row's three ranks side by side, so entry `row * ranks + rank` is row `row`'s \
    rank `rank` — `[3, 1, 2, 2, 0, 3, 3, 0, 1, 0, 1, 2, 3, 2, 1]`. **Both land from the same launch, \
    in the same body, into two different outputs**, which is what `docs/notes/gather-spelling.md` \
    §2.1 measured and what `.t` is for: **`.t` is the layout `examples/decode_loop.lichen`'s `keep` \
    can gather from**, because there `lane % k.dim` is a coordinate a consumer can compute without \
    knowing a rank's row. `.p` is the cheaper one and it is free for a reason worth stating: it is \
    written at **`.at lane`**, because `rank * rows + row` **is** the lane index whenever `rank = \
    lane / rows` and `row = lane % rows`. There is nothing to concatenate; a launch of `n * k` lanes \
    writing at its own index *is* `k` `n`-long buffers laid end to end.\n\n\
    **What three ranks cost: `k` launches, one buffer, and `6` folds per row where `3` would do.** The \
    launches are `.n 5`, then `.n 10`, then `.n 15` — each one widening the same buffer by one rank \
    and re-reading the ranks it already has. **The reason is the one wall \
    `docs/notes/gather-spelling.md` §2.4 found beside `keep`**: a rank that exists only as a \
    `@loop` result inside a lane cannot exclude anything in a following `@loop`, so rank two's fold \
    may not read rank one's position. Putting it in a buffer is what makes it a read, and that costs \
    one launch per rank. **Counted from the extents and not timed**: the three launches run `1 + 2 + \
    3 = 6` folds per row — every lane of a gather launch carries the deepest fold in its body and \
    the `Select` picks between two values rather than guarding one — where a one-lane-per-row \
    spelling would run `3`. **Nothing here shortens `k` passes to one.** A rank is a **guard factor \
    and a `Select` arm**, not a new form: ranks one and two differ from each other by one more \
    conjunct, `(1 * (i != mid))`, and from rank zero by being in `Int`.\n\n\
    **The `k`-tuple spelling runs too, and it is the cheaper one on launches.** Three `@loop` \
    nests in **one** body, each result bound at lane level — `first = f0 (k.w, ((0.0 - 1.0) / 0.0) ~ \
    `identity`, 0)`, `second = f1 (k.w, 0, 0)`, `third = f2 (k.w, 0, 0)` — the next nest's step \
    reading `first` in `(1 * (i != first))`, and three writes returned as a **tuple in output-field \
    order**. Measured here at `k = 3`: it **runs**, and answers `([3, 2, 3, 0, 3], [1, 0, 0, 1, 2], \
    [2, 3, 1, 2, 1])` — the same three ranks, in **one launch and three buffers**. So the \
    comparison is a straight trade and neither side wins: **the tuple is `1` launch where the gather \
    is `3`, and `3` buffers where the gather is `1`**, the tuple pays no address arithmetic and the \
    gather pays `rank`, `row` and one write per layout. **This file ships the gather anyway, for the \
    one thing the tuple does not have**: its ranks are **addressable by arithmetic**, which is what a \
    consumer like `keep` needs and what `gather-spelling.md` §3 says the one-buffer answer buys. \
    `gather-spelling.md` §2.4's condition was wrong and is corrected there: a cross-read lowers, and the \
    refusal needs **two** things at once — that read plus a `Bin` outside every nest over a nest result \
    whose value a `compute.write` takes. This file is that second half's evidence: three nests whose \
    steps read the previous rank all run.\n\n\
    **Ties: the three ranks do not share one rule, and that is a choice with three measured \
    answers.** Rank zero is `examples/sample.lichen`'s `Float` fold with `>`, and a fold counts \
    **down**, so a later visit never displaces an equal value and the **highest** position wins. \
    Ranks one and two are `Int` folds with `>=`, where a later visit **does** displace an equal \
    value and the **lowest** wins. On the tie row `7 7 7 7` this file answers rank zero `3`, rank one \
    `0`, rank two `1`; **with `>` written in all three places the same program answers `3`, `2`, \
    `1`**, and with `>=` in all three it would answer `0`, `1`, `2`. Three correct top-3 answers to \
    one row, chosen by one character. **The same position is in two ranks never, on this data, and \
    the reason is structural rather than lucky**: the exclusion is a **conjunct of the guard**, so a \
    fold cannot select an excluded position even when it holds the row's largest value — which is \
    why a tie row is the case that exercises it. **The one way a position repeats is the seed, and \
    the next paragraph is that case.**\n\n\
    **The finite seed's consequence is a refusal, not a wrong answer.** Ranks one and two seed their \
    folds with `(k.w, 0, 0)` where rank zero seeds with `((0.0 - 1.0) / 0.0)`, and **a row whose \
    values go below that seed never gets that far**: `examples/convert.lichen` records that \
    `float2int` refuses a float with no `Int` to truncate toward — `NaN`, an infinity, **a \
    negative** — as `operator.out_of_range`, and inside a kernel body that refusal arrives as \
    `compute.parallel: integer overflow`. Measured here by adding a sixth row of negatives below \
    the seed: **rank zero still answers it** (`3` for `-5 -2 -9 -1`, its fold having no conversion \
    anywhere) and **the program stops**, rather than answering rank one and rank two from the seed. \
    So the reach of the second and later ranks is **exactly the non-negative integers**, which is a \
    narrower statement than the range `examples/sample.lichen` names, and it is why these two \
    kernels claim `Reduction(combine)` and **not** `Reduction(combine,identity)`: the claim is \
    refused by name — *`the kernel claims Reduction and its body carries no mark for its \
    identity role`* — and the honest claim is that **this fold has no identity**. A sampler over \
    signed logits therefore cannot be written in this spelling at all, and that is the wall the \
    ladder actually meets at `k >= 2`.\n\n\
    **Everything here is exact; nothing is modelled.** The scores are integers under `2^24` and every \
    one of them non-negative, so `float2int` on each is exact; the comparisons are `Int` order \
    comparisons; `i`, `base + i` and every product of a `0`/`1` guard by an integer under `4` are \
    exact; `int2float` of a position under `4` is exact; and rank zero's only non-integer is \
    `((0.0 - 1.0) / 0.0)`, which is the signed infinity `BinaryOp`'s invariant gives it rather than \
    a large number. **The declared row was derived by hand before the program was run**, and the \
    derivation is at the end.\n\n\
    **Six workarounds, and which of them is a rule.** (1) **Ranks one and two are wholly in \
    `Int`** — a **rule**: `&` is refused by the kernel (*`BitAnd has no float form`*) and writing \
    `and` in `Float` is refused by the type checker (*`expected Float, found Int`*), both measured \
    and recorded in `examples/sample.lichen`, cited rather than re-measured. (2) **The guard is \
    arithmetic and carries the `1 *` anchor** — a **rule**, the same refusal: a product with the \
    comparison on its left does not lower. (3) **The claim drops the `identity` role** — a \
    **rule**, by name, quoted above. (4) **Three writes come back as a tuple in output-struct \
    order** — a **rule** (*`output 0 of the parallel index function is a compute.write to output \
    1`*). (5) **`.rows` and `.ranks` are ordinary runtime leaves placed after `.w`** — a \
    **preference**: only the **first** leaf is bound by a rule (it is the launch extent), and \
    `examples/gemv.lichen` measures that a body taking no coordinate may read its second leaf \
    wherever it likes. (6) **Kernel one is left verbatim rather than unified with the other two** \
    — a **preference**, chosen so this file's first five entries are `examples/sample.lichen`'s \
    declared row by construction rather than by agreement. **The rule underneath all six** is \
    `docs/notes/code-audit.md` P1-42's `wontfix:D17`, that a `compute.write` is **the value of an \
    output position and not an effect** — which is why every lane writes exactly one slot per \
    output, and why the ranks have to be in a buffer before a later launch may read them.\n\n\
    **The hand derivation, before the run.** Five rows of width four. **Rank zero**, the \
    `examples/sample.lichen` fold over each row counting `3, 2, 1, 0` with `9 > -inf` at position \
    `3` standing and nothing displacing it — row `3 9 4 9` gives `3`; `5 1 5 2` gives `2`, its \
    leading `5` only tied at the end; `7 7 7 7` gives `3`, the highest of four equals; `8 6 4 2` \
    gives `0`, whose `8` is visited **last** and stands; `1 2 3 4` gives `3`. So rank zero is \
    `[3, 2, 3, 0, 3]`. **Rank one**, the `Int` fold with `>=` excluding rank zero's position and \
    seeded at `0`: row 0 loses position `3` and counts down over `9, 4, 3`, so `1`; row 1 loses \
    position `2` and counts down over `2, 1, 5`, so `0`; row 2 loses position `3` and every \
    survivor is `7`, so the **lowest** survivor `0` wins; row 3 loses position `0` and counts down \
    over `2, 4, 6`, so `1`; row 4 loses position `3` and counts down over `3, 2, 1`, so `2`. Rank \
    one is `[1, 0, 0, 1, 2]`. **Rank two**, the same fold with one more conjunct, excluding both: \
    row 0 loses `3` **and** `1`, leaving `{3, 4}` at `0` and `2`, so `2` — the other `9` is **not** \
    still there, and reading it as though it were gives `3`; row 1 loses `2` and `0`, leaving `1` at \
    `1` and `2` at `3`, so `3`; row 2 loses `3` and `0`, leaving two `7`s at `1` and `2`, so the \
    lower `1`; row 3 loses `0` and `1`, leaving `2` at `3` and `4` at `2`, so `2`; row 4 loses `3` \
    and `2`, leaving `1` at `0` and `2` at `1`, so `1`. Rank two is `[2, 3, 1, 2, 1]`. Lay the three \
    rank-major for `.p` and row-major for `.t` and the two rows above are what the program prints. \
    The host's data is built by **fill kernels**: the surface has no array-to-buffer conversion \
    (`docs/notes/kernel-buffer-declarations.md`), so the scores are a fourth kernel and this file \
    now runs four."
  output = "([3.0, 2.0, 3.0, 0.0, 3.0, 1.0, 0.0, 0.0, 1.0, 2.0, 2.0, 3.0, 1.0, 2.0, 1.0], [3.0, 1.0, 2.0, 2.0, 0.0, 3.0, 3.0, 0.0, 1.0, 0.0, 1.0, 2.0, 3.0, 2.0, 1.0]): tuple{array{Float, 15}, array{Float, 15}}"
---
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 20)))}
Pick = struct{.p (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
Par0 = struct{.n Int, .w Int, .in Row}
Arg0 = struct{.n Int, .w Int, .I Row}
rank0_raw = (k : Par0) => {
  lane = compute.range k.n
  base = lane * k.w
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float)) ~ "combine"), (s(2) + ((1 * (((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  b_p = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Pick)(.p (compute.write ((compute.Write _)(.to b_p, .at lane, .value (int2float (fold (k.w, ((0.0 - 1.0) / 0.0) ~ "identity", 0))))))))
}
rank0 = compute.parallel (rank0_raw ~ "claim:Reduction(combine,identity)") "cpu" Pick
Sel1 = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 20))), .q (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
Cat1 = struct{.p (compute.Buf ((compute.BufParam _)(.type Float, .length 10)))}
Par1 = struct{.n Int, .w Int, .rows Int, .ranks Int, .in Sel1}
Arg1 = struct{.n Int, .w Int, .rows Int, .ranks Int, .I Sel1}
rank1_raw = (k : Par1) => {
  lane = compute.range k.n
  rank = lane / k.rows
  row = lane % k.rows
  base = row * k.w
  low = float2int ((compute.read ((compute.Read _)(.from k.in.q, .at row)) : Float))
  @loop fold = s => if s(0) == 0 then s(2) else { i = s(0) - 1
    ri = float2int ((compute.read ((compute.Read _)(.from k.in.x, .at (base + i))) : Float))
    hit = (1 * (ri >= s(1))) * (1 * (i != low))
    fold (s(0) - 1, ((s(1) + (hit * (ri - s(1)))) ~ "combine"), ((s(2) + (hit * (i - s(2)))) ~ "combine")) }
  mine = if rank == 0 then low else (fold (k.w, 0, 0))
  b_p = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Cat1)(.p (compute.write ((compute.Write _)(.to b_p, .at lane, .value (int2float mine ~ "element"))))))
}
rank1 = compute.parallel (rank1_raw ~ "claim:Reduction(combine)") "cpu" Cat1
Sel2 = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 20))), .q (compute.Buf ((compute.BufParam _)(.type Float, .length 10)))}
Cat2 = struct{.p (compute.Buf ((compute.BufParam _)(.type Float, .length 15))), .t (compute.Buf ((compute.BufParam _)(.type Float, .length 15)))}
Par2 = struct{.n Int, .w Int, .rows Int, .ranks Int, .in Sel2}
Arg2 = struct{.n Int, .w Int, .rows Int, .ranks Int, .I Sel2}
rank2_raw = (k : Par2) => {
  lane = compute.range k.n
  rank = lane / k.rows
  row = lane % k.rows
  base = row * k.w
  low = float2int ((compute.read ((compute.Read _)(.from k.in.q, .at row)) : Float))
  mid = float2int ((compute.read ((compute.Read _)(.from k.in.q, .at (k.rows + row))) : Float))
  @loop fold = s => if s(0) == 0 then s(2) else { i = s(0) - 1
    ri = float2int ((compute.read ((compute.Read _)(.from k.in.x, .at (base + i))) : Float))
    hit = (1 * (ri >= s(1))) * (1 * (i != low)) * (1 * (i != mid))
    fold (s(0) - 1, ((s(1) + (hit * (ri - s(1)))) ~ "combine"), ((s(2) + (hit * (i - s(2)))) ~ "combine")) }
  mine = if rank == 0 then low else (if rank == 1 then mid else (fold (k.w, 0, 0)))
  b_p = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  b_t = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Cat2)(.p (compute.write ((compute.Write _)(.to b_p, .at lane, .value (int2float mine ~ "element")))), .t (compute.write ((compute.Write _)(.to b_t, .at (row * k.ranks + rank), .value (int2float mine ~ "element"))))))
}
rank2 = compute.parallel (rank2_raw ~ "claim:Reduction(combine)") "cpu" Cat2
Fill = struct{.n Int}
fill_scores_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 20))
  v = (if i == 0 then 3.0 else 0.0) + (if i == 1 then 9.0 else 0.0) + (if i == 2 then 4.0 else 0.0) + (if i == 3 then 9.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 1.0 else 0.0) + (if i == 6 then 5.0 else 0.0) + (if i == 7 then 2.0 else 0.0) + (if i == 8 then 7.0 else 0.0) + (if i == 9 then 7.0 else 0.0) + (if i == 10 then 7.0 else 0.0) + (if i == 11 then 7.0 else 0.0) + (if i == 12 then 8.0 else 0.0) + (if i == 13 then 6.0 else 0.0) + (if i == 14 then 4.0 else 0.0) + (if i == 15 then 2.0 else 0.0) + (if i == 16 then 1.0 else 0.0) + (if i == 17 then 2.0 else 0.0) + (if i == 18 then 3.0 else 0.0) + (if i == 19 then 4.0 else 0.0)
  ((Row)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_scores = compute.parallel fill_scores_raw "cpu" Row
scores = (compute.plrun fill_scores ((Fill)(.n 20)) : Row).x
r0 = (compute.plrun rank0 ((Arg0)(.n 5, .w 4, .I Row(.x scores))) : Pick)
r1 = (compute.plrun rank1 ((Arg1)(.n 10, .w 4, .rows 5, .ranks 2, .I Sel1(.x scores, .q r0.p))) : Cat1)
r2 = (compute.plrun rank2 ((Arg2)(.n 15, .w 4, .rows 5, .ranks 3, .I Sel2(.x scores, .q r1.p))) : Cat2)
(compute.collect r2.p : array{Float, 15}, compute.collect r2.t : array{Float, 15})
```

### `generate.lichen`

```text
---
  order = "29"
  compute = import "compute.lichen"
  doc = "Autoregressive generation, the ladder's last rung, as **one program**: a model step, \
    `examples/sample.lichen`'s greedy sampler, `examples/kvcache.lichen`'s append, and a \
    recursive `generate` that steps **while the sampled token is not the sentinel**. It runs \
    three steps, and **no trip count is written anywhere in it** — the length is the guard's \
    own decision. `docs/notes/llm-operators.md`'s `generation` row records this shape as measured \
    on `dev@4b9bd7a9`; this file is it landing as an example rather than as a probe.\n\n\
    **The declared output has three entries and each answers a different question.** The first \
    is the **sequence**, `collect final(1).k`, which is `[1.0, 2.0, 2.0, 0.0, 3.0]`: the prompt \
    `1 2` at positions `0` and `1`, then the three tokens this loop generated, `2`, `0`, `3`, in \
    that order. The second is the **trip count**, `final(0)`, which is `3` — the loop's length, \
    read from outside the recursion. The third is the **logit row the loop stopped on**, \
    `collect final(2).y`, which is `[-1.0, -4.0, -7.0, 3.0]`: the four scores the sampler saw on \
    its last step, whose largest is the sentinel at position `3`. **So the answer states the \
    sequence, its length, and the row the length came from**, and the third is checkable by hand \
    in one line of the derivation at the end.\n\n\
    **Three of the eight kernels are another example's body, unchanged.** `append_raw` is \
    `examples/kvcache.lichen`'s **verbatim**, all fifteen lines, including the column-major lane \
    enumeration and the `position == k.t` freshness test. `greedy_raw` is \
    `examples/sample.lichen`'s **verbatim**, with its `(counter, best value, best position)` \
    fold and its `1 *` anchor. `logits_raw` is `examples/decode_loop.lichen`'s `score_raw` \
    **verbatim** — the same GEMV, the same `i * k.kdim + (s(0) - 1)` address, the same `0.0` \
    identity — with **only the binder renamed**, because this file already uses the name. What \
    changed is the **launch configuration, not the bodies**: `dim` is `1` rather than `2`, the \
    sampler runs `.n 1 .w 4` over one row rather than `.n 5 .w 4` over five, and the GEMV is \
    `.n 4 .kdim 5`. **The model is the smallest thing that makes the loop a feedback loop**: a \
    fixed integer weight matrix of `4 * 5`, row-major, token by token, read against the cache, \
    so each step's logits are a function of what every earlier step wrote. `examples/decode.lichen` \
    is where a real model's score stage lives; this file's is `decode_loop`'s GEMV over integer \
    data, so that nothing here is modelled.\n\n\
    **The guard reads a token a kernel sampled.** `bit = (compute.collect tok.p : \
    array{Float, 1})[0]` collects the sampler's own one-slot output and reads it, and the test \
    is `if bit == sentinel then ...`, with `sentinel = 3.0` a top-level binding and no literal \
    of it in the body. **Neither the sentinel nor a trip count is in the recursion's shape**: \
    `generate`'s spine is a test and a step, and the step is `generate (s(0) + 1, cached, \
    scored)`. **The base arm returns the state after the append**, so the sampled token is **in** \
    the sequence and the last slot is the sentinel itself — which is what an end-of-sequence \
    token is. `docs/notes/llm-operators.md` measured that a guard may read a top-level binding or \
    an element collected from a buffer; this file uses the third form it names, a token a kernel \
    sampled, and the loop's length is the number of tokens before one matched.\n\n\
    **The `@loop` marker is what makes the trip count a number rather than a `raw`, and that is \
    measured here.** The same program both ways, one character apart: unmarked, the second entry \
    prints `raw 3` and the tuple's second slot has no class; marked, it prints `3` and `Int`. \
    Nothing else differs. `examples/decode_loop.lichen`'s trip count does print as an `Int`, and \
    the reason is visible in its own body: its guard compares `s(0)` against a **literal**, so \
    the count is decided and the recursion expands, and a decided count has a class. **Here the \
    count is undecided**, and the measured shape of the conversion \
    (`crates/lichen-highlevel/tests/highlevel/loop_marker.rs`) carries **one slot per element of \
    the state tuple**, with the counter at slot zero — so the marker is where an undecided trip \
    count is given its type. **Without it the second declared entry would not be a fact about \
    the loop at all, only a `raw` cell**, and this file would have no observable length.\n\n\
    **What bounds the loop, measured, and it is not the pair of constants the ladder records.** \
    The checker's budget is a tuned pair — `apply_depth_limit = 500` and `apply_total_limit = \
    2_000` (`crates/lichen-highlevel/src/checker.rs:384-389`) — and both refusals are worded as \
    **non-termination** rather than as a stop condition that was not met. Four minimal recursions \
    with a buffers-derived guard that never fires, each its own program, at `dev@c2c04546`: with \
    a body that **dispatches nothing**, the unmarked spelling reports `nested applications \
    exceeded 500 levels` and the `@loop` spelling reports `it applied a function more than 2000 \
    times`; with **one `compute.plrun` in the body**, **both spellings report `it applied a \
    function more than 2000 times`**. **So the marker is what moves the bound, and it does not \
    move it for a recursion that dispatches kernels** — this file's body dispatches three, so its \
    bound is the cumulative one whichever way it is written. That is what `sentinel = 4.0`, a \
    value no sampled row answers, produces here, in about two seconds: `this binding never \
    terminates — it applied a function more than 2000 times (non-terminating recursion)`. **It is \
    a non-termination report and not a refusal**: the program is not told its stop condition \
    failed, it is told it ran too long, and nothing distinguishes the two at the call site. The \
    mechanism behind the first row is **not traced here** — the guard reads the applied node's \
    own depth (`crates/lichen-lowlevel/src/apply.rs:44`) — and the four runs are the whole of the \
    claim.\n\n\
    **Why a missed sentinel does not merely keep going, measured.** The cache has `capacity = 5` \
    and the loop takes three steps, so a fourth would ask for `.t = 5` — and `position` is \
    `lane % capacity`, so it is never `5`. **No lane is fresh, the append rewrites the whole \
    cache with its own contents, and the loop samples the same token forever.** Measured by \
    starting the recursion at step `3` with `sentinel = 2.0`, which takes exactly one step and \
    hands back `[1.0, 2.0, -1.0, -1.0, -1.0]`: **the cache unchanged, with no error, no \
    refusal and no report line**. That is `examples/kvcache.lichen`'s write-side rule meeting a \
    fixed extent, and it is why the constraint \
    `docs/notes/llm-operators.md` paragraph (c) records — an extent that follows the state is \
    not writable, so a fixed extent plus masking is the only spelling available — is a **rule \
    here and not a preference**. This loop cannot overrun into a refusal; it silently stops \
    advancing.\n\n\
    **Ties: `>` is written in both places, deliberately, and the data has a tie row on \
    purpose.** The fold counts **down** and the comparison is strict, so among equal values the \
    **highest** position wins — `examples/sample.lichen`'s rule, cited rather than re-measured. \
    Step zero's row is `[5.0, 8.0, 8.0, 4.0]`: positions `1` and `2` both hold the maximum, \
    position `2` is visited first and `>` does not displace an equal value, so the first token \
    is `2`. **The counterfactual is measured rather than argued**: the same program with `>=` in \
    that one place answers `[1.0, 2.0, 1.0, 0.0, 3.0]`, the same trip count `3`, and a last \
    logit row of `[2.0, 1.0, -1.0, 6.0]`. **So the tie rule is the body's, it changed the \
    sequence, and it did not change the loop's length here** — a stop condition built on a \
    sampled token inherits whatever the fold's comparison says, and a tie that straddled the \
    sentinel would change the length as well.\n\n\
    **The seed's reach, and where this file sits against `examples/topk.lichen`.** Rank zero's \
    fold seeds with `((0.0 - 1.0) / 0.0)`, the signed infinity `BinaryOp`'s invariant gives it, \
    so its comparison is against every representable `Float` — and step one's logit row is \
    `[-4.0, -7.0, -10.0, -5.0]`, **all four negative**, so that seed is load-bearing rather than \
    decorative. `examples/topk.lichen` measured the narrower reach of the ranks that seed with \
    `0`: an `Int` fold seeded at `0` covers the non-negative integers only and **refuses** below \
    it rather than answering. **So a greedy sampler reaches negative logits and a top-`k` of rank \
    two or more does not**, which is that file's own wall at `k >= 2` and is unchanged by \
    anything here.\n\n\
    **What is exact and what is modelled. Nothing is modelled.** Every cache slot, every weight \
    and every logit is an integer below `2^24`; the largest magnitude anywhere is `36`, and each \
    dot product sums exactly representable `f32` values whose partial sums are integers, so the \
    fold order reproduces the hand derivation term for term. The arithmetic is `+`, `-`, `*` and \
    the four order comparisons: there is **no `exp`, no `sqrt`, no `ln`, no division and no \
    `float2int` on data** anywhere in the program, and `int2float` of a position under `4` is \
    exact. The only non-integer values in the whole file are the `-inf` seed and the type \
    annotations. **The declared line above is host-only**, because the CLI installs no compute \
    backend and this file's own literal is `cpu` — **and the same program answers the same on a \
    device**, with that one literal `gpu` and a backend installed: an RTX 3060 Laptop GPU returns \
    `([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0])`, **every entry bit-identical**, and \
    nothing was loosened to get there. All three kernels are on the device, each emitted module \
    accepted by `spirv-val`. **The guard is still a host `compute.collect`**, so the stop \
    condition is decided on the host three times over and the device answers the arithmetic only. \
    `docs/notes/device-leg-of-the-ladder.md` carries the measurement.\n\n\
    **The hand derivation, before the run.** The cache starts at `1 2 -1 -1 -1`: a two-token \
    prompt and three spare slots, holding `-1` because that is outside the vocabulary `0 1 2 3` \
    and a spare slot must not be confusable with a generated `0`. Negative numbers are spelled \
    `(0.0 - n)` throughout, because the grammar has no negative literal \
    (`docs/language-spec.md`: *non-negative decimal literals*). **Step zero**, cache `1 2 -1 -1 \
    -1`: `2 + 4 + 3 - 3 - 1 = 5`, `4 + 4 + 5 - 3 - 2 = 8`, `0 + 8 + 6 - 3 - 3 = 8`, `5 + 8 + 3 - \
    8 - 4 = 4` — so `[5, 8, 8, 4]`, a tie for the maximum between positions `1` and `2`, \
    resolved to `2`. Token `2` is appended at position `2`. **Step one**, cache `1 2 2 -1 -1`: \
    `2 + 4 - 6 - 3 - 1 = -4`, `4 + 4 - 10 - 3 - 2 = -7`, `0 + 8 - 12 - 3 - 3 = -10`, `5 + 8 - 6 \
    - 8 - 4 = -5` — so `[-4, -7, -10, -5]`, every entry negative, maximum `-4` at position `0`. \
    Token `0` is appended at position `3`. **Step two**, cache `1 2 2 0 -1`: `2 + 4 - 6 + 0 - 1 \
    = -1`, `4 + 4 - 10 + 0 - 2 = -4`, `0 + 8 - 12 + 0 - 3 = -7`, `5 + 8 - 6 + 0 - 4 = 3` — so \
    `[-1, -4, -7, 3]`, maximum `3` at position `3`, **which is the sentinel**. Token `3` is \
    appended at position `4` and the recursion returns. Three steps, so the trip count is `3` and \
    the cache is `1 2 2 0 3`.\n\n\
    **Five decisions, and which of them is a rule.** (1) **`dim = 1`** — a **preference**: the \
    cache holds one token per position, which is what a generated sequence is, and with `dim = \
    1` the append's write address is the identity on the lane index. `examples/kvcache.lichen` \
    is where the computed address is measured and this file gives it up — and the consequence is \
    that **the cache is also the transcript, so no gather kernel is needed at all** where \
    `examples/decode_loop.lichen` needed `keep` as a sixth; `docs/notes/gather-spelling.md` §2.2 \
    and §2.4 are where the gather shapes are measured, and neither is reached here. (2) **The \
    incoming key row is the sampler's own output buffer**, `.nk tok.p` — a **preference** that \
    costs nothing: a `plrun`'s output named as the next one's input is the chaining \
    `examples/decode.lichen` measures, and there is no copy anywhere in the file. (3) **The \
    value half is written and never read** — a **consequence** of (1): `column` is `0` at every \
    lane when `dim = 1`, so `nv` is read at index `0` alone and every lane writes the same number \
    into `cached.v`, **and `nv` is as wide as the launch: five elements, not one.** That used to be \
    the device's own length rule — `InputShorterThanCount` compared a **host** input's length \
    against the **launch count** rather than against the addresses the body reads, so a \
    one-element `compute.buffer` beside a five-index append was refused by name. **The rule was \
    removed (the owner's decision), so the one-element `nv` runs now**: the device answers this \
    program element for element. It is written rather than dropped because \
    `append_raw` is reused verbatim and a \
    `compute.write` is **the value of an output position and not an effect** \
    (`docs/notes/code-audit.md` P1-42, `wontfix:D17`), so dropping the second write would mean \
    editing a measured body. (4) **The trip count is a state slot rather than a second \
    launch** — a **preference**, and it is `examples/decode_loop.lichen`'s own third entry read \
    the same way. (5) **Only the last step's logit row is observable** — a **preference**: the \
    state carries `scored`, so the third entry is the row the loop stopped on rather than every \
    row it ever saw. **The rule underneath all five is D17's** — one value per output position, \
    every position written on every lane — which is why the append rewrites the whole cache \
    instead of one row, why the sampler's single lane writes one slot, and why a launch cannot \
    grow with the loop.\n\n\
    **What this file does not reach.** The **graph** leg is not reached, and \
    `examples/decode.lichen` records the two-input refusal that would meet it. And **a growing \
    cache remains unwritten**: the extent is a literal `5`, which is the one wall paragraph (c) \
    names.\n\n\
    **The device leg, and a prediction this file's own text had to withdraw.** It used to say the \
    device leg was not reached, and that the append's narrow incoming row would refuse first \
    **because the sampler here is one element long by construction** — reasoned from \
    `examples/kvcache.lichen`'s measurement, not measured here. **Measured instead, on an RTX 3060 \
    Laptop GPU, the leg runs and answers the declared line element for element.** The prediction \
    was half right for the wrong reason, and **the reason is the finding**: the one-element input \
    that refuses is **not** the sampler's pick. `.nk tok.p` is the greedy launch's **own output**, \
    and a device input stages nothing and so has no length to check; the input that refused is \
    `nv`, the **host-built** value row, and giving it the launch count's width is the whole repair. \
    The other two shapes are worse and were measured against: **launching over what the buffer \
    holds** leaves four cache slots unwritten, which `wontfix:D17`'s every-position rule forbids and \
    the GEMV would then read stale keys through, and **rebuilding the row on the host from the \
    collected token** gives up decision (2) — the sampler's own output named as the next input — and \
    adds a host round trip per step for nothing. \
    `docs/notes/device-leg-of-the-ladder.md` carries the rows. The host's data is built by **fill \
    kernels**: the surface has no array-to-buffer conversion \
    (`docs/notes/kernel-buffer-declarations.md`), so the weights, the cache, the value row and the \
    blank log are five more kernels and this file now runs eight, and the recursion's state is \
    annotated `tuple{Int, KVOut, Gout}` because a read inside a `@loop` body needs the state's type \
    decided."
  output = "([1.0, 2.0, 2.0, 0.0, 3.0], 3, [-1.0, -4.0, -7.0, 3.0]): tuple{array{Float, 5}, Int, array{Float, 4}}"
---
KVIn  = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 5))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 5))), .nk (compute.Buf ((compute.BufParam _)(.type Float, .length 1))), .nv (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
KVOut = struct{.k (compute.Buf ((compute.BufParam _)(.type Float, .length 5))), .v (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
ParK  = struct{.n Int, .cap Int, .dim Int, .t Int, .in KVIn}
ArgK  = struct{.n Int, .cap Int, .dim Int, .t Int, .I KVIn}
append_raw = (k : ParK) => {
  column = compute.row k
  position = compute.col k
  slot = position * k.dim + column
  old_k = (compute.read ((compute.Read _)(.from k.in.k, .at slot)) : Float)
  old_v = (compute.read ((compute.Read _)(.from k.in.v, .at slot)) : Float)
  row_k = (compute.read ((compute.Read _)(.from k.in.nk, .at column)) : Float)
  row_v = (compute.read ((compute.Read _)(.from k.in.nv, .at column)) : Float)
  fresh = position == k.t
  new_k = old_k + ((row_k - old_k) * (if fresh then 1.0 else 0.0))
  new_v = old_v + ((row_v - old_v) * (if fresh then 1.0 else 0.0))
  b_k = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  b_v = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((KVOut)(.k (compute.write ((compute.Write _)(.to b_k, .at slot, .value (new_k ~ "element")))), .v (compute.write ((compute.Write _)(.to b_v, .at slot, .value (new_v ~ "element"))))))
}
append = compute.parallel (append_raw ~ "claim:Reduction(element)") "cpu" KVOut
Gin  = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 20))), .x (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
Gout = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
ParS = struct{.n Int, .kdim Int, .in Gin}
ArgS = struct{.n Int, .kdim Int, .I Gin}
logits_raw = (k : ParS) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (i * k.kdim + (s(0) - 1)))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at (s(0) - 1))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Gout)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (k.kdim, 0.0 ~ "identity")))))))
}
logits = compute.parallel (logits_raw ~ "claim:Reduction(combine,identity,element)") "cpu" Gout
Row  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
Pick = struct{.p (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}
Par  = struct{.n Int, .w Int, .in Row}
Arg  = struct{.n Int, .w Int, .I Row}
greedy_raw = (k : Par) => {
  lane = compute.range k.n
  base = lane * k.w
  @loop fold = s => if s(0) == 0 then s(2) else fold (s(0) - 1, (max s(1) ((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float)) ~ "combine"), (s(2) + ((1 * (((compute.read ((compute.Read _)(.from k.in.x, .at (base + s(0) - 1))) : Float) > s(1))) * ((s(0) - 1) - s(2)))) ~ "combine"))
  b_p = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Pick)(.p (compute.write ((compute.Write _)(.to b_p, .at lane, .value (int2float (fold (k.w, ((0.0 - 1.0) / 0.0) ~ "identity", 0))))))))
}
greedy = compute.parallel (greedy_raw ~ "claim:Reduction(combine,identity)") "cpu" Pick
Fill = struct{.n Int}
weightsBuf = struct{.weights (compute.Buf ((compute.BufParam _)(.type Float, .length 20)))}
fill_weights_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 20))
  v = (if i == 0 then 2.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then (0.0 - 3.0) else 0.0) + (if i == 3 then 3.0 else 0.0) + (if i == 4 then 1.0 else 0.0) + (if i == 5 then 4.0 else 0.0) + (if i == 6 then 2.0 else 0.0) + (if i == 7 then (0.0 - 5.0) else 0.0) + (if i == 8 then 3.0 else 0.0) + (if i == 9 then 2.0 else 0.0) + (if i == 10 then 0.0 else 0.0) + (if i == 11 then 4.0 else 0.0) + (if i == 12 then (0.0 - 6.0) else 0.0) + (if i == 13 then 3.0 else 0.0) + (if i == 14 then 3.0 else 0.0) + (if i == 15 then 5.0 else 0.0) + (if i == 16 then 4.0 else 0.0) + (if i == 17 then (0.0 - 3.0) else 0.0) + (if i == 18 then 8.0 else 0.0) + (if i == 19 then 4.0 else 0.0)
  ((weightsBuf)(.weights (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_weights = compute.parallel fill_weights_raw "cpu" weightsBuf
weights = (compute.plrun fill_weights ((Fill)(.n 20)) : weightsBuf).weights
kbufBuf = struct{.kbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
fill_kbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 5))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then (0.0 - 1.0) else 0.0) + (if i == 3 then (0.0 - 1.0) else 0.0) + (if i == 4 then (0.0 - 1.0) else 0.0)
  ((kbufBuf)(.kbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_kbuf = compute.parallel fill_kbuf_raw "cpu" kbufBuf
kbuf = (compute.plrun fill_kbuf ((Fill)(.n 5)) : kbufBuf).kbuf
vbufBuf = struct{.vbuf (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
fill_vbuf_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 5))
  v = 0.0
  ((vbufBuf)(.vbuf (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_vbuf = compute.parallel fill_vbuf_raw "cpu" vbufBuf
vbuf = (compute.plrun fill_vbuf ((Fill)(.n 5)) : vbufBuf).vbuf
nvBuf = struct{.nv (compute.Buf ((compute.BufParam _)(.type Float, .length 5)))}
fill_nv_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 5))
  v = 0.0
  ((nvBuf)(.nv (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_nv = compute.parallel fill_nv_raw "cpu" nvBuf
nv = (compute.plrun fill_nv ((Fill)(.n 5)) : nvBuf).nv
blankBuf = struct{.blank (compute.Buf ((compute.BufParam _)(.type Float, .length 4)))}
fill_blank_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 4))
  v = 0.0
  ((blankBuf)(.blank (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_blank = compute.parallel fill_blank_raw "cpu" blankBuf
blank = (compute.plrun fill_blank ((Fill)(.n 4)) : blankBuf).blank
sentinel = 3.0
@loop generate = (s : tuple{Int, KVOut, Gout}) => {
  scored = (compute.plrun logits ((ArgS)(.n 4, .kdim 5, .I Gin(.w weights, .x s(1).k))) : Gout)
  tok = (compute.plrun greedy ((Arg)(.n 1, .w 4, .I Row(.x scored.y))) : Pick)
  cached = (compute.plrun append ((ArgK)(.n 5, .cap 5, .dim 1, .t (s(0) + 2), .I KVIn(.k s(1).k, .v s(1).v, .nk tok.p, .nv nv))) : KVOut)
  bit = (compute.collect tok.p : array{Float, 1})[0]
  if bit == sentinel then (s(0) + 1, cached, scored) else generate (s(0) + 1, cached, scored) }
final = (generate (0, KVOut(.k kbuf, .v vbuf), Gout(.y blank)) : tuple{Int, KVOut, Gout})
(compute.collect final(1).k : array{Float, 5}, final(0), compute.collect final(2).y : array{Float, 4})
```

### `stencil.lichen`

```text
---
  order = "30"
  compute = import "compute.lichen"
  doc = "One explicit step of the 1D heat equation: `y[i] = x[i] + alpha * (x[i - 1] - 2 * x[i] + \
    x[i + 1])`, the three-point Laplacian, one lane per element. It is the ladder's first \
    **physics** operator and its first whose **read set is wider than its lane**: every operator \
    below it reads `x[i]` or a slice from `0`, this one reads `i - 1` and `i + 1` as well. Its \
    two-dimensional half is `examples/torus.lichen`, which is where the horizontal boundary stops \
    being the padding.\n\n\
    **The boundary condition is the padding, not a special case.** At `i = 0` the left index is \
    `i - 1` and at `i = n - 1` the right one is `i + 1`, and both are **outside the buffer** — an \
    out-of-buffer read answers `0` (`crates/lichen-compute/src/compute.rs:6234`; the same rule that \
    made `examples/gemv.lichen` answer `0` for `x`'s fourth element). So both ends are \
    **Dirichlet-zero** with no branch, no `Select` and no clamp, and the **Dirichlet** operator has \
    no case analysis in it at all. That is a choice of operator rather than a limitation of the \
    surface: a different boundary is a different operator, because a boundary that is *a parameter* \
    would need a `Select` whose arms are data, and the surface has none.\n\n\
    **The write address is the lane's own index.** A `compute.write` names a slot of an output \
    buffer exactly as long as the launch's extent \
    (`crates/lichen-compute/src/compute.rs:5979`), so `compute.range c.n` with `.at i` is a \
    bijection and the answer does not depend on a schedule. The **read** address is the interesting \
    one: `[i - 1, i + 1]` is not a prefix of the buffer and not a slice of it, so a body that \
    reaches outside its own lane has to be checked against the device's tail-lane rule rather than \
    assumed — the guard at the entry point is what makes a surplus lane write nothing \
    (`docs/notes/lichen-compute-gpu.md`, the tail-lane obligation).\n\n\
    **`alpha` is a `Float` leaf, and the diffusion number is the operator's argument.** The first \
    scalar leaf is the extent and every other leaf is carried for what it is (`e24d00fb`), so the \
    number that decides **stability** is a caller's to choose: this step is stable only for \
    `alpha <= 1/2`, which is a fact about the physics and not something the operator can enforce, \
    because `alpha` is a runtime value by design. The rule that bites is positional: the **second** \
    leaf is what `compute.row`/`compute.col` divide by \
    (`crates/lichen-compute/src/compute/body.rs:788-815`), and this body has no coordinates, so the \
    second leaf is free.\n\n\
    **The arithmetic is exact, and the declared output is worked out by hand.** `x` is a bump at \
    both ends, `4 0 0 0 0 4`, and `alpha` is `0.25`. Lane `0` reads the padding as `0`, so \
    `4 + 0.25 * (0 + 0 - 8) = 2`; lane `1` is `0 + 0.25 * (4 + 0 - 0) = 1`; lanes `2` and `3` have \
    zeroes on all three taps and stay `0`; lane `4` is `0 + 0.25 * (0 + 4) = 1`; and lane `5`, \
    with the right padding as its right neighbour, is `4 + 0.25 * (0 + 0 - 8) = 2`. The answer is \
    `2 1 0 0 1 2` — every value a quarter or a half below `2^24`, so **nothing is modelled**: no \
    `exp`, no `sqrt`, no `ln`, no division.\n\n\
    **The same data under a periodic boundary, which is the wrap done on purpose.** The second \
    chain runs the ring kernel on the *same* `4 0 0 0 0 4`, so the only thing that changes is the \
    boundary: lane `0`'s left tap is `x[5] = 4` instead of the padding's `0`, and lane `5`'s right \
    tap is `x[0] = 4`. The two edges therefore stop leaking and answer `3` where the Dirichlet step \
    answered `2`, so the readings are `3 1 0 0 1 3` against `2 1 0 0 1 2` — and the difference is \
    visible as a **conserved total**: both start at `8`, the ring ends at `8` (the Laplacian sums to \
    zero over a ring, so a step with any `alpha` leaves the total alone) and the Dirichlet step ends \
    at `6`, having lost `2` through the two zero edges. `examples/torus.lichen` shows the same pair \
    in two dimensions, where the masked grid loses three and the torus holds.\n\n\
    **A solver is a loop, and it can be typed.** The third chain does not unroll: it seeds the kernel's \
    buffer with the ring's first step and runs two more through a top-level `@loop`, handing the beam \
    from one iteration to the next as `s(1)`. Two things make that work and neither is obvious. A \
    **recursive function's type follows from its annotation** (`language-spec.md` §3's `f : f` rule; \
    `examples/gcd.lichen` states it), so `march` carries \
    `tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 6))} -> compute.Buf ((compute.BufParam _)(.type Float, .length 6))` — an **array** state without that annotation \
    prints `raw[raw …]: ?a`, a value with no type, which is why a solver that threads an array needs \
    one. And the **dispatch \
    happens inside the fold**: a plain run is one pass, so an operator asked before its operand is \
    decided is never asked again (`docs/notes/graph-recording-rounds.md`), which is why the state is \
    the kernel's **buffer** — `seed = looped.y`, and each iteration hands `s(1)` straight to `.x` — \
    rather than the `XOut` struct the other two chains pass around: a fold carrying that struct never \
    resolves at all (measured: `raw none` at one, two and three iterations), while a buffer threads \
    through and needs no `collect` inside the step, so the loop pays **one** readback for the whole run \
    instead of one per step (measured: 3.29 ms against 7.21 per dispatch at 1024 lanes on the device, the median of five runs). \
    The answer is the same `2.1875 1.3125 0.5 0.5 1.3125 2.1875` \
    the unrolled form gives, and the total is still `8` at every step, which is what makes three \
    steps checkable by hand rather than by comparing two runs.\n\n\
    **The lane count is six, which matters for one reason.** `LOCAL_SIZE_X` is 64, so each of these \
    runs has 58 surplus lanes, and a surplus lane's fabricated coordinates would put its write at \
    `i < 6` if nothing guarded it — which is exactly the class of silent wrong answer measured on \
    `examples/kvcache.lichen`'s append before the guard landed. A six-element stencil is therefore a \
    device check of the tail-lane guard, not only a physics demonstration, and the sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` answers `agrees, 18 of 18 element(s)` \
    on the RTX 3060 for all three chains: the padding is the same boundary on both backends, and the \
    surplus lanes leave the answer untouched. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`). The ring's boundary is an **arithmetic select** rather than a conditional over a read: the surface refuses a conditional whose arm reads a buffer, so both addresses are read and the test picks between them with a factor. The arithmetic is exact — every element is a quarter or a half — so each neighbour is the copy or the padding and not a rounding of it."
  output = "([2.0, 1.0, 0.0, 0.0, 1.0, 2.0], [3.0, 1.0, 0.0, 0.0, 1.0, 3.0], [2.1875, 1.3125, 0.5, 0.5, 1.3125, 2.1875]): tuple{array{Float, 6}, array{Float, 6}, array{Float, 6}}"
---
XIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
XOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
XPar = struct{.n Int, .alpha Float, .in XIn}
XArg = struct{.n Int, .alpha Float, .I XIn}
stencil_raw = (c : XPar) => {
  i = compute.range c.n
  centre = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  left = (compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))) : Float)
  right = (compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((XOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre)))))))
}
stencil = compute.parallel stencil_raw "cpu" XOut
ring_raw = (c : XPar) => {
  i = compute.range c.n
  centre = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  left_base = (compute.read ((compute.Read _)(.from c.in.x, .at ((max i 1) - 1))) : Float)
  left_wrap = (compute.read ((compute.Read _)(.from c.in.x, .at (c.n - 1))) : Float)
  left = left_base + ((left_wrap - left_base) * (if i == 0 then 1.0 else 0.0))
  right_base = (compute.read ((compute.Read _)(.from c.in.x, .at ((min i (c.n - 2)) + 1))) : Float)
  right_wrap = (compute.read ((compute.Read _)(.from c.in.x, .at 0)) : Float)
  right = right_base + ((right_wrap - right_base) * (if i == (c.n - 1) then 1.0 else 0.0))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((XOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre)))))))
}
ring = compute.parallel ring_raw "cpu" XOut
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  v = (if i == 0 then 4.0 else 0.0) + (if i == 5 then 4.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 6)) : xBuf).x
out = (compute.plrun stencil ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
looped = (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x x))) : XOut)
seed = looped.y
@loop march = (((s : tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 6))}) => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun ring ((XArg)(.n 6, .alpha 0.25, .I XIn(.x s(1)))) : XOut).y)) : tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 6))} -> compute.Buf ((compute.BufParam _)(.type Float, .length 6)))
solved = march (2, seed)
(compute.collect out.y : array{Float, 6}, compute.collect looped.y : array{Float, 6}, compute.collect solved : array{Float, 6})
```

### `torus.lichen`

```text
---
  order = "31"
  compute = import "compute.lichen"
  doc = "The heat equation's three-point Laplacian in **two** dimensions, one lane per element of a \
    row-major grid, and then the same step wrapped into a torus and looped. It is the half of the \
    stencil family that `examples/stencil.lichen` does not cover: that file is one-dimensional and \
    its boundaries are the padding, this one is two-dimensional and its **horizontal** boundaries \
    are not.\n\n\
    **The vertical taps are the padding and the horizontal ones wrap.** With a row-major \n\
    flattening, `(row - 1) * cols + col` is negative on the first row and past the end on the last, \
    so both answer `0` — the same rule `examples/stencil.lichen` states. But `i - 1` at `col = 0` is \
    the *previous row's last element* and `i + 1` at `col = cols - 1` is the *next row's first*, so a \
    flat neighbour walk crosses rows instead of vanishing. The first chain masks them, \
    `if col == 0 then 0.0 else read (i - 1)` and the same at `cols - 1`, and a `Select` whose two \
    arms are both `Float` is **accepted** — the refusal recorded on `examples/attention.lichen`'s \
    `scores` was about arms computed in *different* classes (`b6dea46c`).\n\n\
    **`cols` is the second scalar leaf, and that is where it must go.** A `compute.range` launch's \
    first leaf is the extent, and the **second** is the divisor `compute.row` and `compute.col` use \
    (`crates/lichen-compute/src/compute/body.rs:788-815`); a stride anywhere else silently answers a \
    different grid, which is the trap `examples/matmul.lichen` records for its own `k`. So the \
    parameter shape is `(.n Int, .cols Int, .alpha Float, ...)` and `rows` is computed in the body as \
    `c.n / c.cols` — integer division, which is exact here because the launch is `rows * cols`.\n\n\
    **The first chain's answer, by hand.** `cols = 3`, `alpha = 0.25`, and the plane is `0 0 4 / 4 0 \
    0 / 0 0 0` — the two `4`s sit at the cells that make the wrap **visible**, because `(0, 2)`'s \
    unmasked right neighbour is `(1, 0)` and `(1, 0)`'s unmasked left is `(0, 2)`. Masked, both taps \
    are `0` and the two `4`s simply decay: `4 + 0.25 * (0 - 16) = 0`. Unmasked, each would instead \
    add `0.25 * 4 = 1` from the other row's edge cell and answer `1`, which is the difference the \
    mask buys. Every other cell takes `0.25 * 4 = 1` from the one `4` that is a real neighbour — \
    `(0, 0)` from below, `(0, 1)` from the right, `(1, 1)` from the left, `(1, 2)` and `(2, 0)` from \
    above — and `(2, 1)`, `(2, 2)` have no `4` at any tap and stay `0`. The answer is `1 1 0 / 0 1 1 \
    / 1 0 0`.\n\n\
    **The second chain wraps all four taps, and then the total cannot move.** `above` and `below` \
    select the last and first rows, `left` and `right` the last and first columns, so every element \
    contributes to exactly two taps and the Laplacian sums to zero over the torus. The plane sums to \
    `8`; the masked chain's step sums to `5`, having lost three through its four zero edges; the \
    wrapped step sums to `8`, and so does the wrapped answer after one more step through the loop. \
    That pair is what makes a boundary a *choice*, and it is the closest this language comes to a \
    conserved quantity — `docs/notes/physics-operators.md` records that a conservation can be read \
    in the declared output but not stated as an invariant.\n\n\
    **The loop is the same shape `examples/stencil.lichen` uses, one dimension up.** The state is the \
    kernel's **buffer**: `seed = wrapped.y`, and each iteration hands `s(1)` straight to `.x`, because \
    a plain run is one pass and an operator asked before its operand is decided is never asked again \
    (`docs/notes/graph-recording-rounds.md`). A fold carrying the `GOut` **struct** never resolves at \
    all (`raw none`); a buffer threads through and needs no `collect` inside the step, so the loop \
    pays one readback for the whole run instead of one per step. `march` still carries \
    `tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 9))} -> compute.Buf \
    ((compute.BufParam _)(.type Float, .length 9))`: a recursive function's type follows from its \
    annotation (`language-spec.md` §3's `f : f` rule), and stating it is what an example should do — \
    though a buffer state is the one shape that also resolves without it, unlike an array state, which \
    prints `raw[raw …]: ?a` unannotated.\n\n\
    **Nine lanes leave 55 surplus.** `LOCAL_SIZE_X` is 64, so a surplus lane's fabricated \
    coordinates would put its write inside `[0, 9)` if nothing guarded it — the class of silent \
    wrong answer measured on `examples/kvcache.lichen`'s append before the guard landed. The sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` answers `agrees, 18 of 18 element(s)` \
    on the RTX 3060 for both chains, so the four `Select`s, the computed `rows` and the guard are \
    all right on the device. The host's data is built by **fill kernels**: the surface has no \
    array-to-buffer conversion (`docs/notes/kernel-buffer-declarations.md`), so the plane is a third \
    kernel and this file now runs three."
  output = "([1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0], [0.5, 0.75, 1.5, 1.5, 0.75, 0.5, 0.75, 1.0, 0.75]): tuple{array{Float, 9}, array{Float, 9}}"
---
GIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 9)))}
GOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 9)))}
GPar = struct{.n Int, .cols Int, .alpha Float, .in GIn}
GArg = struct{.n Int, .cols Int, .alpha Float, .I GIn}
grid_raw = (c : GPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  centre = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  above = (compute.read ((compute.Read _)(.from c.in.x, .at ((row - 1) * c.cols + col))) : Float)
  below = (compute.read ((compute.Read _)(.from c.in.x, .at ((row + 1) * c.cols + col))) : Float)
  left = if col == 0 then 0.0 else ((compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))) : Float))
  right = if col == (c.cols - 1) then 0.0 else ((compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))) : Float))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((GOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (centre + c.alpha * ((above + below + left + right) - 4.0 * centre)))))))
}
grid = compute.parallel grid_raw "cpu" GOut
torus_raw = (c : GPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  centre = (compute.read ((compute.Read _)(.from c.in.x, .at i)) : Float)
  above = if row == 0 then ((compute.read ((compute.Read _)(.from c.in.x, .at ((rows - 1) * c.cols + col))) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at ((row - 1) * c.cols + col))) : Float))
  below = if row == (rows - 1) then ((compute.read ((compute.Read _)(.from c.in.x, .at col)) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at ((row + 1) * c.cols + col))) : Float))
  left = if col == 0 then ((compute.read ((compute.Read _)(.from c.in.x, .at (i + c.cols - 1))) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at (i - 1))) : Float))
  right = if col == (c.cols - 1) then ((compute.read ((compute.Read _)(.from c.in.x, .at (i - c.cols + 1))) : Float)) else ((compute.read ((compute.Read _)(.from c.in.x, .at (i + 1))) : Float))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((GOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (centre + c.alpha * ((above + below + left + right) - 4.0 * centre)))))))
}
torus = compute.parallel torus_raw "cpu" GOut
Fill = struct{.n Int}
fill_plane_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 9))
  v = (if i == 2 then 4.0 else 0.0) + (if i == 3 then 4.0 else 0.0)
  ((GIn)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_plane = compute.parallel fill_plane_raw "cpu" GIn
plane = (compute.plrun fill_plane ((Fill)(.n 9)) : GIn).x
stepped = (compute.plrun grid ((GArg)(.n 9, .cols 3, .alpha 0.25, .I GIn(.x plane))) : GOut)
wrapped = (compute.plrun torus ((GArg)(.n 9, .cols 3, .alpha 0.25, .I GIn(.x plane))) : GOut)
seed = wrapped.y
@loop march = (((s : tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 9))}) => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun torus ((GArg)(.n 9, .cols 3, .alpha 0.25, .I GIn(.x s(1)))) : GOut).y)) : tuple{Int, compute.Buf ((compute.BufParam _)(.type Float, .length 9))} -> compute.Buf ((compute.BufParam _)(.type Float, .length 9)))
solved = march (1, seed)
(compute.collect stepped.y : array{Float, 9}, (compute.collect solved : array{Float, 9}))
```

### `filter.lichen`

```text
---
  order = "32"
  compute = import "compute.lichen"
  doc = "A convolution whose **coefficients are data**: `y[i] = Sigma_k w[k] * x[i - k]`, one lane per \
    output element, with the tap count as a scalar leaf and the weights as an input buffer. Every \
    operator below it carries its constants as leaves; this one carries them as a **buffer**, so the \
    same kernel is a smoothing filter or a difference and a caller changes the filter without a new \
    kernel and without a new leaf.\n\n\
    **The taps are a leaf and the weights are a buffer.** The first scalar leaf is the extent and the \
    **second** is the divisor `compute.row`/`compute.col` use \
    (`crates/lichen-compute/src/compute/body.rs:788-815`) — this body reads no coordinates, so the \
    second is free — and `.taps` sits after it. The loop counts down over the taps, reading `w` at \
    `s(0) - 1` and `x` at `i - (s(0) - 1)`, so the **read set is a window** `[i - taps + 1, i]` rather \
    than a prefix: the first operator here whose read window moves with a runtime value.\n\n\
    **The left edge is the padding, which makes the filter causal.** At `i < k` the address `i - k` is \
    below zero and an out-of-buffer read answers `0` — the same rule `examples/stencil.lichen` states \
    and `examples/gemv.lichen` first measured. So the filter has **zero prehistory** with no branch, \
    no `Select` and no clamp: a causal FIR, deterministic and free. A centred filter would need \
    `(0.0 - 1.0)`-style taps instead, and its left half would discard the same way.\n\n\
    **The write address is still the lane's own index**, so the answer does not depend on a schedule, \
    and the **arithmetic is exact**: integers below `2^24`, multiply and add, no division. `x` is \
    `1 2 3 4 5 6` and the two chains are the same kernel with two coefficient buffers — the smoothing \
    `1 2 1`, which answers `1 4 8 12 16 20` (`1*1`, `1*2 + 2*1`, `1*3 + 2*2 + 1*1 = 8`, and so on), \
    and the difference `1 -1 0`, which answers `1 1 1 1 1 1`. The declared output is that arithmetic, \
    worked out by hand rather than transcribed from a run.\n\n\
    **The reduction is marked the same way as the library's folds.** `claim:Reduction(combine, \
    identity,element)` and the `combine`/`element` marks are what the fusion floor reads when a graph \
    records the kernel (`docs/notes/llm-operators.md`); on the `cpu` path they are carried and not \
    consulted. The identity is `0.0`, which for a sum of products is honest — unlike a maximum, whose \
    identity has to be a caller's leaf (`examples/mathlib.lichen` says why).\n\n\
    **Six lanes leave 58 surplus.** `LOCAL_SIZE_X` is 64, so a surplus lane's fabricated coordinates \
    would put its write inside `[0, 6)` if nothing guarded it — the class of silent wrong answer \
    measured on `examples/kvcache.lichen`'s append before the guard landed. The sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` answers `agrees, 12 of 12 element(s)` on \
    the RTX 3060 for both chains, so the window read, the tap-count leaf and the guard are all right \
    on the device. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "([1.0, 4.0, 8.0, 12.0, 16.0, 20.0], [1.0, 1.0, 1.0, 1.0, 1.0, 1.0]): tuple{array{Float, 6}, array{Float, 6}}"
---
FIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 6))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
FOut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
FPar = struct{.n Int, .taps Int, .in FIn}
FArg = struct{.n Int, .taps Int, .I FIn}
filter_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from c.in.x, .at (i - (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((FOut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (c.taps, 0.0 ~ "identity")))))))
}
filter = compute.parallel (filter_raw ~ "claim:Reduction(combine,identity,element)") "cpu" FOut
Fill = struct{.n Int}
xBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 6)))}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 6))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 3.0 else 0.0) + (if i == 3 then 4.0 else 0.0) + (if i == 4 then 5.0 else 0.0) + (if i == 5 then 6.0 else 0.0)
  ((xBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" xBuf
x = (compute.plrun fill_x ((Fill)(.n 6)) : xBuf).x
smoothBuf = struct{.smooth (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
fill_smooth_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then 2.0 else 0.0) + (if i == 2 then 1.0 else 0.0)
  ((smoothBuf)(.smooth (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_smooth = compute.parallel fill_smooth_raw "cpu" smoothBuf
smooth = (compute.plrun fill_smooth ((Fill)(.n 3)) : smoothBuf).smooth
differenceBuf = struct{.difference (compute.Buf ((compute.BufParam _)(.type Float, .length 3)))}
fill_difference_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 3))
  v = (if i == 0 then 1.0 else 0.0) + (if i == 1 then (0.0 - 1.0) else 0.0)
  ((differenceBuf)(.difference (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_difference = compute.parallel fill_difference_raw "cpu" differenceBuf
difference = (compute.plrun fill_difference ((Fill)(.n 3)) : differenceBuf).difference
smoothed = (compute.plrun filter ((FArg)(.n 6, .taps 3, .I FIn(.x x, .w smooth))) : FOut)
differenced = (compute.plrun filter ((FArg)(.n 6, .taps 3, .I FIn(.x x, .w difference))) : FOut)
(compute.collect smoothed.y : array{Float, 6}, compute.collect differenced.y : array{Float, 6})
```

### `convolution2d.lichen`

```text
---
  order = "33"
  compute = import "compute.lichen"
  doc = "A two-dimensional convolution with a 3x3 coefficient buffer: `y[r,c] = Sigma_(j,k) w[j,k] * \
    x[r-1+j, c-1+k]`, one lane per output cell of a row-major grid. It is the composition of \
    everything the signal and physics samples measured — a two-dimensional layout, **runtime \
    coefficients**, a moving window and a boundary — in one kernel, and its contract names the boundary \
    rather than hiding it. The host's two grids and the coefficient box are three **fill kernels**: the \
    surface has no array-to-buffer conversion (`docs/notes/kernel-buffer-declarations.md`).\n\n\
    **Nine taps are one fold, and the tap index splits into the two offsets.** `@loop` counts down \
    over the nine weights; `(s(0) - 1) / 3` is the row offset and `(s(0) - 1) % 3` the column offset, \
    so the window address is `base + j * cols + k` with `base = (row - 1) * cols + (col - 1)` \
    precomputed **outside** the fold — it does not depend on the tap, and computing it once keeps the \
    fold body one line. `cols` must be the **second** scalar leaf, because that is what \
    `compute.row`/`compute.col` divide by \
    (`crates/lichen-compute/src/compute/body.rs:788-815`); the tap count follows it. The write address \
    stays the lane's own index, so the answer does not depend on a schedule.\n\n\
    **The boundary is where the two chains differ, and the flat address is the trap.** The first \
    chain leaves the window address flat: at `col = 0` its left tap is one below the row's start, which \
    the layout reads as the *previous row's last cell*, not as the padding — measured in \
    `zz_conv2d_probe.rs` with a value at `(1, 4)` that cell `(2, 0)` picks up. Masking that tap needs a \
    `Select` inside the fold, and **that spelling does not compile**: an `if` in a fold body is \
    refused, because the fold is a template compiled before any apply (`zz_masked_conv_probe.rs` \
    measures it with a read in the arm and with a bare `0.0`/`1.0`). The second chain takes the other \
    expressible route and **clamps its taps**, so the window replicates its edge instead of wrapping.\n\n\
    **A clamp has to be spelled shifted, because the index arithmetic is unsigned.** `i - 1` at \
    `i = 0` is not `-1` in a kernel body: it wraps to a huge value, so `max (i - 1) 0` never takes its \
    lower bound and a clamp written that way silently returns the *upper* bound instead. Measured: \
    `min (max (col - 1) 0) 4` answers `4, 0, 1, 2, 3` for `col = 0..4` \
    (`zz_replicate_address_probe.rs`). Shifting before the subtraction fixes it — `(max i 1) - 1` is \
    `max (i - 1, 0)` with no negative intermediate — and `min ((max i 1) - 1) (dim - 1)` is the whole \
    clamp. That is what the second chain writes, and it is the reason a replicate boundary is \
    expressible at all.\n\n\
    **The arithmetic is exact and both answers are blocks.** Both chains use the same `0.125` box, so a \
    cell answers `0.125 * 8.0 = 1.0` once per tap whose window contains an `8.0`. The first chain's grid \
    has its `8.0` at the centre `(2, 2)` behind a zero border, so the cells that contain it are rows \
    `1..3` by columns `1..3` — nine `1.0`s. The second chain's grid puts its `8.0` at `(1, 4)`, on the \
    right edge, so a replicate window sees it once for columns `0..3` and **twice** for column `4` \
    (that column is the edge and repeats), which is why its readings are `0 0 0 1 2` on the three rows \
    that can reach it and zero elsewhere. Both are hand-computed from the window, not transcribed. The \
    taps still sum to `1.125`, not `1.0`: this is a box, not a normalized average, so a uniform field \
    would come back scaled.\n\n\
    **Twenty-five lanes per chain leave 39 surplus.** `LOCAL_SIZE_X` is 64, so a surplus lane's fabricated \
    coordinates would put its write inside `[0, 25)` if nothing guarded it — the class of silent wrong \
    answer measured on `examples/kvcache.lichen`'s append before the guard landed, and the reason a \
    window read needs the guard as much as a flat one does. The sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` answers `agrees, 50 of 50 element(s)` on \
    the RTX 3060 for both chains: the window arithmetic, the coefficient buffer, the clamped addresses and the guard are all right there."
  output = "([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]): tuple{array{Float, 25}, array{Float, 25}}"
---
CIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 25))), .w (compute.Buf ((compute.BufParam _)(.type Float, .length 9)))}
COut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 25)))}
CPar = struct{.n Int, .cols Int, .taps Int, .in CIn}
CArg = struct{.n Int, .cols Int, .taps Int, .I CIn}
convolve_raw = (c : CPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  base = ((row - 1) * c.cols + (col - 1))
  y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from c.in.x, .at (base + (s(0) - 1) / 3 * c.cols + (s(0) - 1) % 3))) : Float))) ~ "combine" ~ "element"))
  ((COut)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (fold (c.taps, 0.0 ~ "identity")))))))
}
replicate_raw = (c : CPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  rows = (c.n / c.cols)
  y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) : Float) * (compute.read ((compute.Read _)(.from c.in.x, .at (min ((max (row + (s(0) - 1) / 3) 1) - 1) (rows - 1) * c.cols + (min ((max (col + (s(0) - 1) % 3) 1) - 1) (c.cols - 1))))) : Float))) ~ "combine" ~ "element"))
  ((COut)(.y (compute.write ((compute.Write _)(.to y, .at i, .value (fold (c.taps, 0.0 ~ "identity")))))))
}
convolve = compute.parallel (convolve_raw ~ "claim:Reduction(combine,identity,element)") "cpu" COut
replicate = compute.parallel (replicate_raw ~ "claim:Reduction(combine,identity,element)") "cpu" COut
XBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 25)))}
WBuf = struct{.w (compute.Buf ((compute.BufParam _)(.type Float, .length 9)))}
EBuf = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 25)))}
Fill = struct{.n Int}
fill_x_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 25))
  v = (if i == 12 then 8.0 else 0.0)
  ((XBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_w_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 9))
  v = 0.125
  ((WBuf)(.w (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_e_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 25))
  v = (if i == 9 then 8.0 else 0.0)
  ((EBuf)(.x (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_x = compute.parallel fill_x_raw "cpu" XBuf
fill_w = compute.parallel fill_w_raw "cpu" WBuf
fill_e = compute.parallel fill_e_raw "cpu" EBuf
x = (compute.plrun fill_x ((Fill)(.n 25)) : XBuf)
weights = (compute.plrun fill_w ((Fill)(.n 9)) : WBuf)
edge = (compute.plrun fill_e ((Fill)(.n 25)) : EBuf)
blurred = (compute.plrun convolve ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x x.x, .w weights.w))) : COut)
replicated = (compute.plrun replicate ((CArg)(.n 25, .cols 5, .taps 9, .I CIn(.x edge.x, .w weights.w))) : COut)
(compute.collect blurred.y : array{Float, 25}, compute.collect replicated.y : array{Float, 25})
```

### `pooling.lichen`

```text
---
  order = "34"
  compute = import "compute.lichen"
  doc = "A 3x3 max-pooling over a row-major grid: `y[r, c]` is the maximum of the nine cells around \
    `(r, c)`, one lane per output cell. It is the smallest operator whose **identity is not a \
    constant**, and the one where the boundary has to be chosen with that in mind rather than after \
    it.\n\n\
    **A maximum has no finite neutral element.** A sum starts at `0.0` honestly; a maximum does not, so \
    the seed is a **leaf** the caller supplies — `.least` here, and this file passes \
    `(0.0 - 100.0)`, below every datum (`examples/mathlib.lichen` says the same for `rowmax`). A seed \
    that is not below the data silently clips the answer, and the kernel cannot detect it.\n\n\
    **The padding rule is right for a sum and wrong for a maximum.** An out-of-buffer read answers \
    `0.0`, which for a max is not neutral: it is *larger* than any negative datum and would win the \
    window. Both failures are measured in \
    `crates/lichen-compute/tests/lichen_compute/zz_pool_probe.rs` — with `0.0` as the seed every cell \
    outside the centre answers `0.0`, and even with a proper seed the cells whose window leaves the \
    buffer answer `0.0`. So this operator takes its boundary from **the caller**, the way \
    `examples/convolution2d.lichen` takes its zero border: the input below is a `7x7` grid whose outer \
    ring is `-1000000.0`, and the five-by-five interior is the actual field. Every tap then lands on a \
    real cell, no zero can enter the maximum, and the window arithmetic needs no clamp and no \
    `Select` — the offsets are `(row + j)` and `(col + k)` into the padded grid, and `.stride` carries \
    its row length.\n\n\
    **The answer is a block over a floor.** The interior is `-1.0` everywhere except `4.0` at \
    `(2, 2)`, and the outer ring is `-1000000.0`. An output cell's window contains the `4.0` exactly \
    when the cell is in rows `1..3` by columns `1..3` — those answer `4.0`. Every other window holds \
    only `-1.0` and ring values, so it answers `-1.0`. Twenty-five numbers, worked out from the window \
    rather than read off a run.\n\n\
    **Twenty-five lanes leave 39 surplus.** `LOCAL_SIZE_X` is 64, so a surplus lane's fabricated \
    coordinates would put its write inside `[0, 25)` if nothing guarded it — the class of silent wrong \
    answer measured on `examples/kvcache.lichen`'s append before the guard landed. The sweep \
    `crates/lichen-tools/examples/operators_on_device.rs` answers `agrees, 25 of 25 element(s)` on \
    the RTX 3060: the padded addressing, the seed leaf and the guard are all right there. The host's data is built by **fill kernels**: the surface has no array-to-buffer \
    conversion (`docs/notes/kernel-buffer-declarations.md`)."
  output = "[-1.0, -1.0, -1.0, -1.0, -1.0, -1.0, 4.0, 4.0, 4.0, -1.0, -1.0, 4.0, 4.0, 4.0, -1.0, -1.0, 4.0, 4.0, 4.0, -1.0, -1.0, -1.0, -1.0, -1.0, -1.0]: array{Float, 25}"
---
PIn  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length 49)))}
POut = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length 25)))}
PPar = struct{.n Int, .cols Int, .stride Int, .taps Int, .least Float, .in PIn}
PArg = struct{.n Int, .cols Int, .stride Int, .taps Int, .least Float, .I PIn}
pool_raw = (c : PPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((max s(1) ((compute.read ((compute.Read _)(.from c.in.x, .at ((row + (s(0) - 1) / 3) * c.stride + (col + (s(0) - 1) % 3)))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length c.n))
  ((POut)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value (fold (c.taps, c.least ~ "identity")))))))
}
pool = compute.parallel (pool_raw ~ "claim:Reduction(combine,identity,element)") "cpu" POut
Fill = struct{.n Int}
paddedBuf = struct{.padded (compute.Buf ((compute.BufParam _)(.type Float, .length 49)))}
fill_padded_raw = (c : Fill) => {
  i = compute.range c.n
  d = compute.buf ((compute.BufParam _)(.type Float, .length 49))
  v = (if i == 0 then (0.0 - 1000000.0) else 0.0) + (if i == 1 then (0.0 - 1000000.0) else 0.0) + (if i == 2 then (0.0 - 1000000.0) else 0.0) + (if i == 3 then (0.0 - 1000000.0) else 0.0) + (if i == 4 then (0.0 - 1000000.0) else 0.0) + (if i == 5 then (0.0 - 1000000.0) else 0.0) + (if i == 6 then (0.0 - 1000000.0) else 0.0) + (if i == 7 then (0.0 - 1000000.0) else 0.0) + (if i == 8 then (0.0 - 1.0) else 0.0) + (if i == 9 then (0.0 - 1.0) else 0.0) + (if i == 10 then (0.0 - 1.0) else 0.0) + (if i == 11 then (0.0 - 1.0) else 0.0) + (if i == 12 then (0.0 - 1.0) else 0.0) + (if i == 13 then (0.0 - 1000000.0) else 0.0) + (if i == 14 then (0.0 - 1000000.0) else 0.0) + (if i == 15 then (0.0 - 1.0) else 0.0) + (if i == 16 then (0.0 - 1.0) else 0.0) + (if i == 17 then (0.0 - 1.0) else 0.0) + (if i == 18 then (0.0 - 1.0) else 0.0) + (if i == 19 then (0.0 - 1.0) else 0.0) + (if i == 20 then (0.0 - 1000000.0) else 0.0) + (if i == 21 then (0.0 - 1000000.0) else 0.0) + (if i == 22 then (0.0 - 1.0) else 0.0) + (if i == 23 then (0.0 - 1.0) else 0.0) + (if i == 24 then 4.0 else 0.0) + (if i == 25 then (0.0 - 1.0) else 0.0) + (if i == 26 then (0.0 - 1.0) else 0.0) + (if i == 27 then (0.0 - 1000000.0) else 0.0) + (if i == 28 then (0.0 - 1000000.0) else 0.0) + (if i == 29 then (0.0 - 1.0) else 0.0) + (if i == 30 then (0.0 - 1.0) else 0.0) + (if i == 31 then (0.0 - 1.0) else 0.0) + (if i == 32 then (0.0 - 1.0) else 0.0) + (if i == 33 then (0.0 - 1.0) else 0.0) + (if i == 34 then (0.0 - 1000000.0) else 0.0) + (if i == 35 then (0.0 - 1000000.0) else 0.0) + (if i == 36 then (0.0 - 1.0) else 0.0) + (if i == 37 then (0.0 - 1.0) else 0.0) + (if i == 38 then (0.0 - 1.0) else 0.0) + (if i == 39 then (0.0 - 1.0) else 0.0) + (if i == 40 then (0.0 - 1.0) else 0.0) + (if i == 41 then (0.0 - 1000000.0) else 0.0) + (if i == 42 then (0.0 - 1000000.0) else 0.0) + (if i == 43 then (0.0 - 1000000.0) else 0.0) + (if i == 44 then (0.0 - 1000000.0) else 0.0) + (if i == 45 then (0.0 - 1000000.0) else 0.0) + (if i == 46 then (0.0 - 1000000.0) else 0.0) + (if i == 47 then (0.0 - 1000000.0) else 0.0) + (if i == 48 then (0.0 - 1000000.0) else 0.0)
  ((paddedBuf)(.padded (compute.write ((compute.Write _)(.to d, .at i, .value v)))))
}
fill_padded = compute.parallel fill_padded_raw "cpu" paddedBuf
padded = (compute.plrun fill_padded ((Fill)(.n 49)) : paddedBuf).padded
pooled = (compute.plrun pool ((PArg)(.n 25, .cols 5, .stride 7, .taps 9, .least (0.0 - 100.0), .I PIn(.x padded))) : POut)
compute.collect pooled.y : array{Float, 25}
```

### `worldgen.lichen`

```text
---
  order = "35"
  compute = import "compute.lichen"
  doc = "A seeded world generator, and the application the heavy-value cache is validated on \
    (`docs/notes/heavy-value-cache.md` §9.17). Three elevation octaves and two moisture octaves \
    of hashed value noise, blended by two `mix` kernels, smoothed twice by one five-tap kernel, \
    classified into five biomes and turned into a tile atlas — twelve `@cache` stages over a \
    `128 × 128` grid of `16 384` cells.\n\n\
    **The noise is the language's own arithmetic, not a modelled function.** A lattice corner \
    answers `((x * 7919) + (y * 104729) + (seed * 1299709)) % 65536 / 65535`, every product \
    staying inside 31 bits, so the integer half is exact on both backends and the float half is \
    a repeatable reading rather than a driver's. The interpolation between four corners is \
    smoothstep-weighted (`fx * fx * (3.0 - 2.0 * fx)`), so an octave's `scale` is a real \
    frequency and not a label.\n\n\
    **The stages are the dependency graph the cache exists for, and each one is a `@cache` binding \
    whose value is a buffer.** The three elevation octaves and the two moisture octaves are \
    independent; `blended` and `elevation` fold the first three, `moisture` folds the other two, \
    `eroded` and `relief` run the same five-tap kernel twice, `biomes` classifies on both fields, \
    and `tiles` adds a per-cell variant. Editing the biome thresholds therefore dirties `biomes` \
    and `tiles`; editing an octave's `scale` dirties that octave and everything downstream of it \
    and nothing on the moisture side; a whitespace edit moves no cell at all. \
    `crates/lichen-compute/examples/worldgen_edits` drives exactly those edits and compares the \
    session's whole collected map against a fresh compile's.\n\n\
    **The declared output samples three cells** — `(0, 0)`, `(63, 127)` and `(127, 127)` — of the \
    relief, of the biome grid and of the atlas. The relief samples are `0.67571974`, \
    `0.25864074` and `0.6503648`; the biome ids are `1`, `0` and `2` — low-and-dry, sea, \
    low-and-wet, under `sea = 0.35` and `ridge = 0.7` — and the atlas entries are `7`, `3` and \
    `11`, which is `biome * 4 + variant` with variant `3` at all three: 7919 ≡ 3 and 104729 ≡ 1 \
    and 1299709 ≡ 1 modulo `4`, so the multipliers take `(0, 0)` to `3`, and the two samples \
    whose coordinates are both `≡ 3` land on the same residue."
  output = "(0.67571974, 0.25864074, 0.6503648, 1, 0, 2, 7, 3, 11): tuple{Float, Float, Float, Int, Int, Int, Int, Int, Int}"
---
InSeed   = struct{.seed Int}
OutField = struct{.f (compute.Buf ((compute.BufParam _)(.type Float, .length 16384)))}
OutIds   = struct{.ids (compute.Buf ((compute.BufParam _)(.type Int, .length 16384)))}
InField  = struct{.f (compute.Buf ((compute.BufParam _)(.type Float, .length 16384)))}
InField2 = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length 16384))), .b (compute.Buf ((compute.BufParam _)(.type Float, .length 16384)))}
InLand   = struct{.height (compute.Buf ((compute.BufParam _)(.type Float, .length 16384))), .moisture (compute.Buf ((compute.BufParam _)(.type Float, .length 16384)))}
InTile   = struct{.biomes (compute.Buf ((compute.BufParam _)(.type Int, .length 16384)))}

PNoise = struct{.n Int, .cols Int, .scale Int, .seed Int, .in InSeed}
ANoise = struct{.n Int, .cols Int, .scale Int, .seed Int, .I InSeed}
noise_raw = (p : PNoise) => {
  i = compute.range p.n
  row = compute.row p
  col = compute.col p
  x = col / p.scale
  y = row / p.scale
  fx = int2float (col % p.scale) / int2float p.scale
  fy = int2float (row % p.scale) / int2float p.scale
  sx = fx * fx * (3.0 - 2.0 * fx)
  sy = fy * fy * (3.0 - 2.0 * fy)
  h00 = int2float (((x * 7919) + (y * 104729) + (p.seed * 1299709)) % 65536) / 65535.0
  h10 = int2float ((((x + 1) * 7919) + (y * 104729) + (p.seed * 1299709)) % 65536) / 65535.0
  h01 = int2float (((x * 7919) + ((y + 1) * 104729) + (p.seed * 1299709)) % 65536) / 65535.0
  h11 = int2float ((((x + 1) * 7919) + ((y + 1) * 104729) + (p.seed * 1299709)) % 65536) / 65535.0
  top = h00 + ((h10 - h00) * sx)
  bottom = h01 + ((h11 - h01) * sx)
  value = top + ((bottom - top) * sy)
  b_f = compute.buf ((compute.BufParam _)(.type Float, .length p.n))
  ((OutField)(.f (compute.write ((compute.Write _)(.to b_f, .at i, .value value)))))
}
noise = compute.parallel noise_raw "cpu" OutField

PMix = struct{.n Int, .cols Int, .w Float, .in InField2}
AMix = struct{.n Int, .cols Int, .w Float, .I InField2}
mix_raw = (p : PMix) => {
  i = compute.range p.n
  a = (compute.read ((compute.Read _)(.from p.in.a, .at i)) : Float)
  b = (compute.read ((compute.Read _)(.from p.in.b, .at i)) : Float)
  b_f = compute.buf ((compute.BufParam _)(.type Float, .length p.n))
  ((OutField)(.f (compute.write ((compute.Write _)(.to b_f, .at i, .value (a + ((b - a) * p.w)))))))
}
mix = compute.parallel mix_raw "cpu" OutField

PSmooth = struct{.n Int, .cols Int, .in InField}
ASmooth = struct{.n Int, .cols Int, .I InField}
smooth_raw = (p : PSmooth) => {
  i = compute.range p.n
  row = compute.row p
  col = compute.col p
  rows = p.n / p.cols
  up = (compute.read ((compute.Read _)(.from p.in.f, .at (((max row 1) - 1) * p.cols + col))) : Float)
  down = (compute.read ((compute.Read _)(.from p.in.f, .at ((min (row + 1) (rows - 1)) * p.cols + col))) : Float)
  left = (compute.read ((compute.Read _)(.from p.in.f, .at ((row * p.cols) + ((max col 1) - 1)))) : Float)
  right = (compute.read ((compute.Read _)(.from p.in.f, .at ((row * p.cols) + (min (col + 1) (p.cols - 1))))) : Float)
  centre = (compute.read ((compute.Read _)(.from p.in.f, .at i)) : Float)
  value = (centre * 0.5) + ((up + down + left + right) * 0.125)
  b_f = compute.buf ((compute.BufParam _)(.type Float, .length p.n))
  ((OutField)(.f (compute.write ((compute.Write _)(.to b_f, .at i, .value value)))))
}
smooth = compute.parallel smooth_raw "cpu" OutField

PBiome = struct{.n Int, .cols Int, .sea Float, .ridge Float, .in InLand}
ABiome = struct{.n Int, .cols Int, .sea Float, .ridge Float, .I InLand}
biome_raw = (p : PBiome) => {
  i = compute.range p.n
  e = (compute.read ((compute.Read _)(.from p.in.height, .at i)) : Float)
  m = (compute.read ((compute.Read _)(.from p.in.moisture, .at i)) : Float)
  b = if e < p.sea then 0 else (if e < p.ridge then (if m < 0.45 then 1 else 2) else (if m < 0.55 then 3 else 4))
  b_ids = compute.buf ((compute.BufParam _)(.type Int, .length p.n))
  ((OutIds)(.ids (compute.write ((compute.Write _)(.to b_ids, .at i, .value b)))))
}
biome = compute.parallel biome_raw "cpu" OutIds

PTile = struct{.n Int, .cols Int, .seed Int, .in InTile}
ATile = struct{.n Int, .cols Int, .seed Int, .I InTile}
tile_raw = (p : PTile) => {
  i = compute.range p.n
  row = compute.row p
  col = compute.col p
  b = (compute.read ((compute.Read _)(.from p.in.biomes, .at i)) : Int)
  v = ((col * 7919) + (row * 104729) + (p.seed * 1299709)) % 4
  b_ids = compute.buf ((compute.BufParam _)(.type Int, .length p.n))
  ((OutIds)(.ids (compute.write ((compute.Write _)(.to b_ids, .at i, .value ((b * 4) + v))))))
}
tile = compute.parallel tile_raw "cpu" OutIds

seed = 7
size = 128
count = size * size

@cache octave_a = (compute.plrun noise ((ANoise)(.n count, .cols size, .scale 32, .seed seed, .I InSeed(.seed seed))) : OutField).f
@cache octave_b = (compute.plrun noise ((ANoise)(.n count, .cols size, .scale 16, .seed (seed + 1), .I InSeed(.seed (seed + 1)))) : OutField).f
@cache octave_c = (compute.plrun noise ((ANoise)(.n count, .cols size, .scale 8, .seed (seed + 2), .I InSeed(.seed (seed + 2)))) : OutField).f

@cache blended = (compute.plrun mix ((AMix)(.n count, .cols size, .w 0.5, .I InField2(.a octave_a, .b octave_b))) : OutField).f
@cache elevation = (compute.plrun mix ((AMix)(.n count, .cols size, .w 0.25, .I InField2(.a blended, .b octave_c))) : OutField).f

@cache damp_a = (compute.plrun noise ((ANoise)(.n count, .cols size, .scale 24, .seed (seed + 3), .I InSeed(.seed (seed + 3)))) : OutField).f
@cache damp_b = (compute.plrun noise ((ANoise)(.n count, .cols size, .scale 12, .seed (seed + 4), .I InSeed(.seed (seed + 4)))) : OutField).f
@cache moisture = (compute.plrun mix ((AMix)(.n count, .cols size, .w 0.5, .I InField2(.a damp_a, .b damp_b))) : OutField).f

@cache eroded = (compute.plrun smooth ((ASmooth)(.n count, .cols size, .I InField(.f elevation))) : OutField).f
@cache relief = (compute.plrun smooth ((ASmooth)(.n count, .cols size, .I InField(.f eroded))) : OutField).f

@cache biomes = (compute.plrun biome ((ABiome)(.n count, .cols size, .sea 0.35, .ridge 0.7, .I InLand(.height relief, .moisture moisture))) : OutIds).ids
@cache tiles = (compute.plrun tile ((ATile)(.n count, .cols size, .seed seed, .I InTile(.biomes biomes))) : OutIds).ids

view = (compute.collect relief : array{Float, 16384})
land = (compute.collect biomes : array{Int, 16384})
atlas = (compute.collect tiles : array{Int, 16384})
(view[0], view[8191], view[16383], land[0], land[8191], land[16383], atlas[0], atlas[8191], atlas[16383])
```

<!-- end: examples -->
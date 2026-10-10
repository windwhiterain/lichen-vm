# `Int` and `Float`: the two numeric classes

> Status: **current — phases 0–2 landed.** A float literal checks, prints,
> round-trips through an artifact, and runs as a kernel on **both backends**; the
> two classes meet only at `int2float` / `float2int`.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowValue::Float`, `LowShape`),
> `crates/lichen-lowlevel/src/codec.rs` (the value tags),
> `crates/lichen-lowlevel/src/equality.rs` (`observed_low_shape`),
> `crates/lichen-highlevel/src/shape.rs` (`for_each_kind_marker!`) and
> `program.rs` (`TypeValue`, `TypeOperator::Int2Float` / `Float2Int`),
> `crates/lichen-language-lex` / `-parser` (the literal and the two keywords),
> `crates/lichen-render` (the round-tripping spelling),
> `crates/lichen-kernel-ir/src/lib.rs` (`ScalarClass`, `BufferSlot`,
> `KernelInstr::Conv`), `crates/lichen-compute` and `crates/lichen-compute-gpu`
> (the two backends). The demand is
> [whiting-scene-document](whiting-scene-document.md); the operator set is
> [operators](operators.md) §7.

## 1. The two classes

`Int` and `Float` are **distinct classes** with no implicit conversion between
them. A number's class is its representation and its type at once:

| | `Int` | `Float` |
|---|---|---|
| lowlevel value | `LowValue::USize` | `LowValue::Float(f32)` |
| low shape | `LowShape::USize` | `LowShape::Float` |
| type constant | `TypeInt` | `TypeFloat`, the ninth kind marker |
| literal | `5` | `1.5` |
| kernel data | `i64`, 64-bit | `f32` |

`f32`, not `f64`: it is what a packed scene buffer component already is, so the
handoff is a copy rather than a conversion, and it is what most GPUs compute
(§4.1).

A float literal is `[0-9]+\.[0-9]+` — a digit is **required** before the dot, so a
leading dot stays field access. Overflowing a literal yields an infinity, not a
lex error (`f32` has one, and the operators produce it too). There is no
scientific notation (`1.5e3` is `Float(1.5)` then the name `e3`), and no prefix
minus, so a negative float is written by subtraction, not by a literal.

`1 == 1.5` **does not check**: the operands are differently-typed values and the
checker unifies their types first. Any answer that made it check would be a
conversion wearing a comparison's clothes.

`==`/`!=` are the **generalized** equality over any two same-typed values, and
they read `ValueExt::value_eq` — the same relation unification, table keys and
frozen-artifact reuse read. For a float that relation is **bit equality**: §3.1.

## 2. Why this is a compiler plugin, not a native one

[plugin-taxonomy](plugin-taxonomy.md) splits a feature by one question: does the
language layer have to change to accommodate it? A kind marker is not a leaf a
native plugin can contribute — it is an entry in a list the highlevel owns
(`for_each_kind_marker!`), whose every consumer is generated from it (the
`TypeValue` variants, the `ValueType` marker constructors, the `Ctx` accessors,
the checker's installed fields, and the artifact codec on both sides). Adding a
marker is therefore a **compiler plugin**, and it is the strongest case in the
workspace for that classification: the codesign site is a single macro invocation
with a comment saying so. `Perspective` and `Doc` are the precedent for the
shape: the semantic core in its own code, the codesign sites in
`lichen-language`.

## 3. The sites

Each subsection below is the one owner of its fact.

### 3.1 The lowlevel value

`LowValue::Float(f32)` beside `USize`. `ValueExt` needs no work: a float is not a
handle and is not traced. `LowShape` gains `Float` beside `USize`, and
`observed_low_shape` needs its arm, or a float value states no shape at all and
§3.8's boundary has nothing to disagree with.

**A float compares by its 32 bits, never by IEEE `==`.** `LowValue`'s derived
`PartialEq` is replaced by a hand-written one: every other variant compares
field-wise as before, and the float compares with `to_bits`. The derived relation
is wrong in both directions — it says `0.0 == -0.0`, which would let a reuse
decision accept one of two distinct artifacts, and `NaN != NaN`, which would make
a cached `NaN` never match its own reload. Every reader of that identity compares
a value against one the codec reproduced **bit for bit**, so "equal but not the
same bits" has no meaning for any of them. The content hash owes *equal keys hash
equal*, so `hash_step` hashes the bits too: a total equality and a matching hash
are one contract.

This is deliberately **not** the relation the language's `==` uses — that is
§3.7's — and the two are kept apart on purpose.

### 3.2 The kind marker — the one list

One entry in `for_each_kind_marker!`:

```rust
/// The `float` type constant — `Float` literals pair with `[float, K]`.
TypeFloat { 9, "float", float_marker, float_marker_node }
```

**The tag is `9`, not `8`, and that is the one thing about this list that is easy
to get wrong.** The kind markers do not have a tag space of their own: each
entry's tag is a tag in the **`TypeValue` codec**, and that codec has one more arm
the registry does not own — `TypeValue::TypeId`, a nominal struct identity, holds
`8` and is spelled by hand. A collision here is **silent**: the registry's arms
expand *before* the hand-spelled `TypeId` arm on both sides, so a marker claiming
`8` makes `8 => TypeValue::TypeId` unreachable, every persisted nominal id decodes
as that marker with a stray `u64` left in the stream, and the compiler reports
`unreachable_patterns` as a **warning** — the build passes. A *gap* in the tag
space, by contrast, hits the catch-all and is a clean `unknown type-value tag N`
error. A guard added here should tighten the collision without loosening the gap.

Everything else follows from the list by construction: the `TypeValue` variant,
the `ValueType::float_marker()` constructor, the `Ctx` accessor, the checker's
installed field and dispatch, and the artifact codec. A marker with no literal is
inert rather than broken, and a `[float, K]` type that reaches `low_type_of`
without a float value behind it falls through to `LowShape::Unknown`.

### 3.3 The literal

`RawToken` gains `[0-9]+\.[0-9]+` beside `[0-9]+`; the lexer takes the longest
match, so `3.5` is one token while `x.5` and `x . y` stay as they are. Three
shapes are decided rather than assumed:

- **A digit is required before the dot**, so `.5` is field access by the pattern
  rather than by token priority.
- **An overflowing literal is an infinity, not a lex error.** `Int` refuses an
  overflow because `usize` has no infinity for the value to become; `f32` has one,
  and the operators produce it too, so refusing it here would make the literal the
  only source of a value the language can otherwise hold.
- **No scientific notation**, a scope decision recorded so it is not
  rediscovered as a gap.

`TokenKind` gains `Float(f32)` beside `Int(usize)`, plus `KwFloat` beside `KwInt`.
There is no prefix minus, and `Eq` comes off `TokenKind`, `Token` and `RawToken`:
nothing folds a token into a key, and `Eq` on an `f32` payload is impossible.
That is the **opposite** of §3.1's decision, on purpose.

### 3.4 The parser and the literal's type

A literal builds a `[value, type]` pair. `Int(5)` builds `[USize(5), [int, K]]`;
`Float(1.5)` builds `[Float(1.5), [float, K]]`, and the parser's literal arm is
the one place that pair is built. The literal's type is its own class, with
nothing to choose.

### 3.5 The printer

`lichen-render`'s `ValuePrinter` spells a `LowValue`; an unhandled variant prints
nothing. A float needs a spelling that **round-trips**: the printer's output is
what the README examples and the sync test compare, and `1.0` printed as `1` reads
back as an `Int` — a different `LowValue`, and by §4.2 a different type — so the
round trip would not merely lose a digit, it would change what the program means.
Rust's own shortest-round-trip formatting is kept, and nothing else may re-spell a
float.

### 3.6 The artifact codec — three tag spaces

All three are append-only and no tag in any of them may ever move. They are
separate encodings: one may be read without the others, and the same value is
written under a different tag in each.

| space | used | next | file and crate |
|---|---|---|---|
| `LowValue` variants | `0`–`8` (`Error` took `7` additively, `Float` is `8`) | **`9`** | `crates/lichen-lowlevel/src/codec.rs` |
| kind markers | the registry holds `0`–`7` and `9` (`TypeString` is `7`, deliberately not its list position; `TypeFloat` is `9`), while `8` is the non-registry `TypeValue::TypeId` | **`10`** | `crates/lichen-highlevel/src/shape.rs` |
| `LowShape` variants | `0`–`6` (`0`–`4` decided, `5` is `Unknown`, the lattice's bottom, `6` is `Float`) | **`7`** | `crates/lichen-language/src/persist/container.rs` |

There is a **fourth** versioned thing, which is not a tag space but behaves like
one: the AST content key, which carries `KEY_FORMAT_VERSION` and whose contract
says adding an `Expr` variant **bumps it** — an `Expr::Float` that does not is a
retained key that silently collides. The key is also a place a float becomes text,
so it owes the printer's discipline: hash the float's **bits**.

Four traps: the kind-marker row is `9`, not `8` (§3.2); the first two rows landing
on the same low number is a coincidence, because they are separate encodings; the
third is easy to miss because `LowShape` is a **lowlevel** enum and its persisted
form lives in `lichen-language`; and the fourth is a counter, not a tag, whose
failure is a wrong answer rather than a rejected read.

### 3.7 The operators

A float takes `+ - * /` and the four order comparisons; it does **not** take `%`,
the bitwise trio, or a conversion operator — the first two because they are not in
its set, the third because §4.2 makes it a contradiction.

**Which class an operation runs over is decided by its operands, and an operand
that names no class leaves the operation where it was.** A concretely `Float`
operand makes the whole operation `Float` and pins the other operand to it, so
`x + 1.5` types `x` as a `Float`; with neither operand naming a class the
operation stays `Int`, so `x + 1` is unchanged. A concrete `Int` operand cannot be
pinned, which is why `1.5 + 1` is a reported `expected Float, found Int` rather
than a coercion. The operators are polymorphic through a refinement contract
([operator-polymorphism](operator-polymorphism.md)).

`==`/`!=` are the one place the decision has a visible edge, and it resolves by
asking the value: `1 == 1.5` does not check, `1.5 == 1.5` is `1`,
`0.0 == -0.0` is `0`, and `NaN == NaN` is `1`. The last is the price of "there is
one relation", and it is the right price: `==`, unification of two concrete
values, table-key comparison and frozen-artifact reuse agree by construction
rather than by discipline. IEEE was considered and declined: it is not an
equivalence relation, so it could not live in `value_eq` without breaking the hash
tables and the marker lookups that read it. One behaviour moved as a consequence
and is not float-specific: `==` now compares a handle-carrying leaf by **content**
rather than by pointer, which is what `value_eq`'s documented contract always
said.

**Division by zero is IEEE for a float, and refused for an `Int`.** `1.0 / 0.0` is
`+inf`, `0.0 / 0.0` is `NaN`, and no divisor is recorded. This is the only answer
consistent with §3.3's value set and with the kernel targets: wasm's float
division by zero yields `inf` without a trap and SPIR-V's is undefined, so an
interpreter refusal beside an IEEE kernel answer would be a divergence.
`operator.divide_by_zero` is therefore the **integer** `Div`/`Rem` category only.

### 3.8 The kernels

`LowShape` gains `Float` beside `USize`, and the join needs no new rule: the
lattice already sends two different decided shapes to `Unknown`, so
`USize ∨ Float` is `Unknown`, the bottom a backend already falls back on.

The rest is not free, because the kernel ABI was `i64` in five places at once and
**could not carry a class at all**: `KernelShape`'s leaf had none, `KernelBin` was
defined as the unsigned-integer operations, and `KernelFragment`'s one class-ish
field was fragment-wide. §4.4 is the five decisions that settled the carrier; the
carrier and the permission have both landed (§5.1).

**Where a float is refused is not a single question, and answering it once is how
a hole gets opened.** Three sites look like the same question and two are not:

| site | question | `Float` |
|---|---|---|
| `kernel_shape` | what *shape* — one value, or a tuple of them | `Scalar` |
| `domain_is_known` | can the ABI lower this to decided locals | **`true`** |
| `flat_arity` | how many locals a domain flattens to | `1` |

All three read **one recursive walk**, so the gate and its message cannot disagree
about a position. The walk recurses through **every** position, and that is where
a hole was: `kernel_domain`'s `Tuple` arm accepts without re-checking elements, so
the walk is the only thing that can see inside. Two positions a first-position-only
walk would miss were admitted before the walk existed — an `Array(Float, 3)`
nested in a tuple, and a `Function(USize, Float)` whose domain is decided and whose
codomain is a float — and neither is reachable now. The uniform walk also makes an
`Unknown` codomain or value position inside a tuple domain refuse at compile time
rather than at launch, which is strictly earlier and clearer.

**A float still cannot reach a `ParallelBackend` unasked**, for reasons
independent of the above: inputs are typed by their class, every producer of a
buffer names its class, and an undecided position is refused by name (§5.1).

## 4. Decisions

### 4.1 `f32`, not `f64`

Most GPUs are `f32`, and the demand is a scene document whose bulk is vertex data.
More usefully, it is the width the consumer's buffer already uses: a packed
component is four little-endian bytes, exactly an `f32`, so the seam is a copy
rather than a conversion. The consumer's in-document float is `f64`, so the
handoff widens `f32 → f64` — exact, and one-directional. An exact rational was
considered and rejected: a buffer has to be four bytes per component, and a
rational has no fixed width.

A second, independent reason came out of measurement: the consumer's document
format does not round-trip every `f64` (its reader refuses an integer literal of
forty or more digits), and `f32::MAX` in that form is thirty-nine digits — just
inside the boundary — while `f64::MAX` walks straight off it. That is luck rather
than design and should not be leaned on; what it buys is that a scene's
coordinates cannot reach the cliff by accident.

### 4.2 `Int` and `Float` do not convert

Not by subtyping, and not by a literal rule either. The language says nothing
about mixing them: `1 + 1.5` does not check, and neither does a `1` that came out
of a `let`. This is the strongest form of the no-subtyping stance in the root
[README](../../README.md), and it is what the consumer's own schema already takes.

**"Do not convert" means do not convert *unasked*.** There is one operator per
direction — `int2float e`, `float2int e` — and nothing inserts it for the author:
each names its own direction's class, checks its operand in the other, and no
implicit rule anywhere in the language reaches across. Both are prefix keywords at
the same level as `!`, so `int2float a + 1` converts `a` and `int2float f x`
converts `f x`. `int2float` never refuses and is exact up to 2²⁴; `float2int`
truncates toward zero and is **partial** off the integer range — a `NaN`, an
infinity, a negative is a runtime refusal naming the value and the reason, not a
check error. In a kernel they lower to one `KernelInstr::Conv { from, to }` each
(wasm `F32ConvertI64U` / `i64.trunc_f32_u`, SPIR-V `OpConvertUToF` /
`OpConvertFToU`; the unsigned source is deliberate, matching wasm). What the two
cost is spelled out in [operators](operators.md) §7.

### 4.3 The boundary does not convert

**Corrected: this decision's mechanism is no longer needed, and nothing in the
language converts at a boundary.** The original question was what a lowering may
do where a class is *not already agreed* — the conservative `Unknown` bottom, or a
declared width that disagrees with the class of the slot it reads or writes. The
answer that landed is the honest one: the class rides on the ABI and every
producer names it (§4.4), an undecided domain is **refused by name**, and the only
conversion the lowering emits is the one the source wrote (the two operators
above), one `KernelInstr::Conv` each. The lowering never inserts a conversion to
square a body with itself.

The invariant that made the original decision dangerous is still the reason to
keep it that way: **the checker never sees a boundary conversion**, so a wrong one
is a wrong number and not a type error — and `i64 → f32` is exact only below 2²⁴,
so a large `Int` that reached a kernel as a float would be a rounded answer that
checked perfectly. That is the "wrong image nobody sees until the render finishes"
failure shape the whiting notes keep returning to, and the answer is a note beside
the value rather than silence.

### 4.4 The class rides in the ABI

Five decisions the code cannot answer for itself, settled before phase 2 was
written because three of them decide what every implementation agent builds:

- **Which positions are permitted.** Every position the recursive `domain_is_known`
  walk reaches, including the two compound ones an earlier phase refused: an
  `Array(Float, _)` nested in a tuple, and a function's codomain. The earlier
  refusal was the ABI having nowhere to put a class, not a float there being
  meaningless.
- **What a compound position is.** Following the existing filler (`flat_arity`
  answers `1` for every non-scalar and non-tuple, as a total function), an
  `array<Float, 3>` parameter is **one local of the element's class**, not three.
  That keeps `flat_arity`'s convention and `KernelShape`'s fold agreeing, and it is
  why a *position* has one class even where a fragment's values do not.
- **The class rides in two places, not one.** The **parameter** class goes on
  `KernelShape`'s leaf, and `fragment_digest` hashes `param_shape` whole, so its
  coverage is free. The **buffer element** class goes on a `KernelFragment` field
  beside `inputs`/`outputs` — `inputs` is already barred from `param_shape` for a
  reason that applies verbatim: a parallel fragment's shape is `(config, index)`
  however many buffers it reads — and it is added to `fragment_digest`. A purely
  fragment-carried or a purely value-carried class cannot cover both, and `fetch`
  forces the read-back's class onto the value because its implementor has no
  fragment at all.
- **`IntWidth` keeps meaning integer width.** A `ScalarClass` beside it says the
  class, and `I32` is still free to take the width slot the field's doc reserves
  for it. Widening `IntWidth` with an `F32` would make `bits()` stop answering its
  question (`I32` and `F32` are both 32) and every existing `bits() != 64` check
  would accept a float fragment as an integer one.
- **Float division is not specified inside a kernel, and that is the answer.**
  SPIR-V's `OpFDiv` with a zero divisor is undefined; wasm's `f32.div` is IEEE.
  The language does not check it and does not promise it: a kernel that divides
  floats by zero returns what its backend returns. This is outside the kernel-safe
  subset — the intersection of what the two backends compute the same way — and it
  is the price of admitting floats at all. §3.7's *interpreter* answer is
  unchanged, because the interpreter is not a kernel.

## 5. What landed

### 5.1 The permission, and what it still refuses

**Landed.** The carrier (§4.4), then both backends, then the width. The wasm
backend admits a float domain and lowers `F32*`; the SPIR-V emitter produces a
module `spirv-val` accepts (`OpTypeFloat 32`, no `Int64` unless needed, `OpFAdd`/
`OpFDiv`, and `Eq`/`Neq` as bitcasts and `IEqual`/`INotEqual` rather than
`OpFOrdEqual`). **The same float program through `"cpu"` and through `"gpu"`
produces the same numbers**, on a real device.

- **The layout**, which is what the two halves once disagreed about:
  `ScalarClass` states `byte_width()` (`Int` 8, `Float` 4) and everything derives
  from it — the host arena packs a float buffer as `f32`s, `spirv::element_stride`
  reads it instead of restating it, `dispatch` derives its stride, padding and
  transfer sizes from the fragment's class, and `DeviceBuffer` carries the class so
  a chained run does not re-upload what it can leave resident. `BufferSlot` became
  `Host(&'a [u8])` and **gave up `Eq`**: eight bytes are one `Int` or two
  `Float`s, so a type answering `true` would claim a fact it cannot check; it
  derives only `PartialEq`. The class-carrying structs stay `Eq`, because a class
  tag is `Eq` and an `f32` payload is not. It is a checked property:
  `a_float_fragment_agrees_across_the_two_backends` and its integer twin run one
  source through both backends at `LOCAL_SIZE_X + 5` elements, so surplus lanes
  run past the bound into the padded tail where a wrong stride corrupts the
  neighbour rather than failing loudly.
- **A float GPU kernel dispatches now.** The run path is class-aware, and
  `GpuContext::fetch` returns `ScalarData` because a packed float payload cannot
  honestly be handed back as words.
- **The class rides with the value, not the fragment.** Each `KernelInstr`
  carries its class, so a body may compute in one class and cross afterwards: the
  index is an `Int`, `int2float i` is the crossing that makes a parallel float
  output vary by lane, and `int2float (x + 1)` over an `Int` parameter runs. Both
  element types are declared in every SPIR-V module, so a module can hold both
  classes and only the 64-bit integer (with its `Int64` capability) is conditional.
- **One class per buffer, and a chain is the one place a mixture is refused.** A
  module declares one buffer-type chain per class it actually uses, and each
  buffer variable is typed with **its own** buffer's class; the host staging is
  per buffer (`reserve`, each upload's `offset`, `data_bytes`, `padded_bytes`, and
  each output's `DeviceBuffer` all derive from that buffer's class). A mixed-class
  fragment runs on both backends
  (`a_fragment_whose_buffers_are_of_two_classes_agrees_across_the_two_backends`).
  A **chain** feeds link `n`'s output into link `n + 1`'s input slot, so a
  fragment whose two ends differ is refused by name
  (`RunError::ChainCrossesClasses`) rather than read at the other width.
- **A crossing is always representable on both targets, and the width is what it
  costs.** Each class's scalar is declared in every module, so only the 64-bit
  integer is conditional. A float fragment's *integer* data is `i64` on the CPU
  and `u32` on the GPU, so it diverges past 2³² — exactly as `int2float`'s
  `i64 → f32` rounding does past 2²⁴ (§4.3). A recorded price, paid knowingly.

**Still refused, and each is the right answer:**

- **A float launch count.** `compute.plrun k (4.0,)` used to be accepted and then
  silently not run, because the count read wanted a `USize` and answered
  `Parameterized` otherwise. It is refused by name now; a dispatch extent is an
  `Int`.
- **An unannotated read of a float buffer.** `WriteOp::build` unifies the count,
  the index and the written value into one element cell, so an undecided operand
  makes `check_binop` pin `a + a` to `Int`, which flows back into that cell: the
  body's own unification closes the loop and the read's fresh cell is not free
  after all. The refusal is truthful on both sides — the language says `Int`, the
  buffer holds `Float` — and the author's fix is the annotation. What closed is
  the case it used to share a cause with: a pure forward
  (`compute.write ((compute.Write _)(.to n, .at i, .value a))`) lowers in the body's class and
  runs on both backends, because each template term's type cell is seeded onto the
  low-type channel ([compute-jit-low-types](compute-jit-low-types.md)).
- **A cross-kernel float chain with no concrete `Float` operand.** A `jit` whose
  body calls a float kernel and adds nothing float decides no class; `+ 0.0` makes
  it run.
- **A genuine mix inside one operation** — `x + 0.5` with no crossing. No
  conversion can serve it, and the refusal is in the shared validator, so neither
  backend can be the quietly permissive one. The wasm backend was once merely
  permissive here and emitted a *valid* module computing `1.0f32 + i_as_f32` where
  the source said `Int + Float` — silently a different number, and no
  single-backend test could have seen it because the GPU half was already right.

The requirement this all rests on, one sentence: **a crossing is representable on
both targets, and a mixture a conversion cannot serve is refused by name.**

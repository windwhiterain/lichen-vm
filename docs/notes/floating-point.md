# Floating point: `Float` as a ninth kind marker

> Status: **proposed.** Nothing here is implemented. This is the shape of the
> change and the decisions taken for it — the width, the relation to `Int`, and
> where a conversion is allowed to happen (§4) — not a description of what the
> code does today. Today the only numeric value the lowlevel has is `USize`.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowValue`, `LowShape`),
> `crates/lichen-lowlevel/src/codec.rs` (the value tags),
> `crates/lichen-highlevel/src/shape.rs` (`for_each_kind_marker!`),
> `crates/lichen-highlevel/src/program.rs` (`TypeValue`, `ValueType`),
> `crates/lichen-language-lex/src/lib.rs` (`RawToken`, `TokenKind`),
> `crates/lichen-language-parser/src/`, `crates/lichen-render/src/render/`
> (`ValuePrinter`), `crates/lichen-kernel-ir/src/lib.rs` (`BufferSlot`,
> `ParallelBackend`, `KernelInstr::Const`, `IntWidth`), and
> [language-spec](../language-spec.md) for the syntax.
> The demand is recorded in [whiting-scene-document](whiting-scene-document.md);
> the decision rule this has to be argued against is
> [plugin-taxonomy](plugin-taxonomy.md).

## 1. What is missing

`LowValue` is the whole of a value's structure:

```rust
// crates/lichen-lowlevel/src/lib.rs:157
pub enum LowValue {
    USize(usize), Str(&'static str), Array(AnyHandle<[ArrayItem]>),
    Table(AnyHandle<[TableItem]>), Function(AnyFunctionId),
    None, Void, Parameterized,
}
```

There is no float, and no byte string either. `USize` is the only number, and
`Str` is a `&'static str` the lowlevel never concatenates, indexes or mutates.
So a program cannot write a real number: `1.5` lexes as `IntLit(1)`, `Dot`,
`IntLit(5)` (`[0-9]+` is the whole number regex,
`crates/lichen-language-lex/src/lib.rs:276`), and `Dot` is field access.

The first consumer that needs this is a scene document for
[whiting-scene-document](whiting-scene-document.md), where every visually
meaningful number — a position, a colour, an intensity, a radius — is a float,
and where a mesh is a flat run of four-byte components. That note is the demand;
this note is the change.

## 2. Why this cannot be a native plugin

[plugin-taxonomy](plugin-taxonomy.md) splits a feature by one question: does the
language layer have to change to accommodate it? A `Float` has to, and the place
it has to change is named in the source:

```rust
// crates/lichen-highlevel/src/shape.rs:44
// THE one list of the 8 kind markers.  Adding or removing a marker means
// editing this list alone: the `TypeValue` variants, the `ValueType` marker
// methods (default bodies), the `Ctx` node accessors, the checker's
// installed marker fields (and its `Ctx::value_node` dispatch and the
// `Build` record), and the `TypeValue` artifact codec (both sides) are all
// macro-derived from it.
macro_rules! for_each_kind_marker { … }
```

That list is **closed**: a native plugin contributes value and operator *leaves*
through `enum_ext!`, and a kind marker is not a leaf — it is an entry in a list
the highlevel owns, whose every consumer is generated from it. Adding a marker is
therefore a **compiler plugin** by the decision rule, and it is the strongest
case in the workspace for that classification: the codesign site is not a
convention but a single macro invocation with a comment saying so.

The precedent for how a compiler plugin is shaped is `Perspective` and `Doc`:
the semantic core lives in its own crate and stays program-generic, while the
codesign sites (a grammar production, an AST field, an IR form, a persist
discriminator) stay in `lichen-language`. A `Float` has the same split, except
that its semantic core is small enough to live where the marker does.

## 3. The sites

Eight groups, in dependency order. The first two are the ones that decide
whether the rest is cheap, and the last is the only one that touches a backend.
§4 records the three decisions this list is shaped by.

### 3.1 The lowlevel value

`LowValue::Float(f32)` beside `USize` — `f32`, for the reasons in §4.1.
`ValueExt` needs no work: a float is not a handle and is not traced.

`LowShape` (`crates/lichen-lowlevel/src/lib.rs:209`) needs `Float` beside
`USize`. `LowShape` is the kernel-safe scalar subset and is the thing a backend
traces without forcing a value, so a float with no shape is a float no backend
can see.

### 3.2 The kind marker — the one list

One entry in `for_each_kind_marker!`:

```rust
/// The `float` type constant — `Float` literals pair with `[float, K]`.
TypeFloat { 8, "float", float_marker, float_marker_node }
```

and everything else follows from the list by construction: the `TypeValue`
variant, the `ValueType::float_marker()` constructor, the `Ctx` accessor, the
checker's installed field and dispatch, and the artifact codec. **This is why
step 3.1 is cheap and the whole change is not:** the derived work is seven
things generated from one line, and the one line is a codesign.

### 3.3 The literal

`RawToken` needs a float pattern that cannot be confused with the existing ones.
`[0-9]+` and `~[0-9]*` are the two numeric patterns; a float adds a third and
the lexer resolves longest-match, so `3.5` must lex as one token while `x.5`
(field read on a name) and `x . y` stay two tokens. Logos's priority, not the
regex alone, is what makes this decidable, and the test that settles it is a
lexer test over the four shapes rather than a parser change.

`TokenKind` gains `Float(f32)` beside `Int(usize)`, plus `KwFloat` beside
`KwInt` — the type constant is a keyword for the same reason `Int` is. Negative
literals are unary minus today and stay that way; `-1.5` is `Minus`, `Float`.

### 3.4 The parser and the literal's type

A literal builds a `[value, type]` pair. `Int(5)` builds
`[USize(5), [int, K]]` today; `Float(1.5)` builds
`[Float(1.5), [float, K]]`, and the parser's literal arm is the one place that
pair is built. §4.2 is what keeps this a single arm rather than a decision: the
literal's type is its own class, with nothing to choose.

### 3.5 The printer

`lichen-render`'s `ValuePrinter` spells a `LowValue`; an unhandled variant is a
diagnostic that prints nothing. A float needs a spelling that **round-trips**,
and that is a constraint rather than a style one: the printer's output is what the
`README` examples and the `readme.rs` sync test compare, and `1.0` printed as `1`
reads back as an `Int` — a different `LowValue`, which §4.2 makes a different
type, so the round trip would not merely lose a digit, it would change what the
program means.

This is not hypothetical: the consumer hits it. Its `Value::Float` `Display` is
`inner.to_string()`, and Rust's float `to_string` prints `1` for `1.0`.

`Str` has no escapes and no concatenation, which is why it round-trips by being
atomic. A float should take the same accident, with the one addition `Str` does
not need: printing is the only place a float becomes text, so that place has to
keep the digits Rust's own shortest-round-trip formatting chooses, and nothing
else may re-spell a float.

### 3.6 The artifact codec — two tag spaces, both full

Both are append-only and neither tag may ever move, so both are stated here
before any code:

| space | used | next | file |
|---|---|---|---|
| `LowValue` variants | `0`–`7` (`Void` took `7` additively) | **`8`** | `crates/lichen-lowlevel/src/codec.rs:244-246` |
| kind markers | `0`–`7` (`TypeString` is `7`, deliberately not its list position) | **`8`** | `crates/lichen-highlevel/src/shape.rs:58-62` |

Both next-tags being `8` is a coincidence, not a shared space. They are separate
encodings and one may be read without the other.

### 3.7 The operators

[operators](operators.md) is the note that owns the operator set, and §4's
second decision is what a float means *to* that set, so the two are read
together. A float needs `+ - * /`; it does **not** need a conversion operator,
and that is a consequence rather than an omission — see §4.2.

`==`/`!=` are the one place the decision has a visible edge. They are the
**generalized** equality over "any two same-typed values" (`operators.md` §1),
and `Int` and `Float` are now two types, so `1 == 1.0` compares two
differently-typed values and does not check. That is the consistent answer
rather than a special case to argue for: any answer that made it true would be a
conversion wearing a comparison's clothes, and would have to be argued twice —
once here and once in the kernel boundary of §3.8.

### 3.8 The kernels

`LowShape` gains `Float` beside `USize`, and the join needs no new rule: the
lattice already sends two different decided shapes to `Unknown`
([lowlevel-low-types](lowlevel-low-types.md)), so `USize ∨ Float` is `Unknown`,
which is the bottom a backend already falls back on. That part is free.

The rest is not free, because the kernel ABI is `i64` in five places at once:

| what | where |
|---|---|
| `BufferSlot::Host(&'a [i64])` | `crates/lichen-kernel-ir/src/lib.rs:64` |
| `ParallelBackend::run(…, inputs: &[BufferSlot], …)` | `:156` |
| `ParallelBackend::fetch(id, count) -> Result<Vec<i64>, String>` | `:192` |
| `KernelInstr::Const(i64)` | `:371` |
| `IntWidth { I64 }` | `:267` |

`fetch` is the one to read twice: it is the **read-back**, so a float result
buffer comes back as `i64` unless the ABI itself moves. That is a larger change
than widening `IntWidth`, and it is why §4.3 is the costliest of the three.

## 4. Decisions

Three forks, all answered. Each was cheaper to decide before the lexer moves
than after.

### 4.1 `f32`, not `f64`

Most GPUs are `f32`, and the demand is a scene document whose bulk is vertex
data. More usefully, it is the width the consumer's buffer already uses: a
whiting packed component is four little endian bytes, which is exactly an `f32`,
so the seam is a copy rather than a conversion.

The consumer's **in-document** float is `f64` (`whiting-definition`
`value.rs:42`), so the handoff widens `f32 → f64` — exact, and one-directional.
A scene whiting authored may carry values this `Float` rounds; that is fine,
because lichen authors scenes here and does not edit them.

An exact rational was considered and rejected on the same grounds: a buffer has
to be four bytes per component, and a rational has no fixed width.

### 4.2 `Int` and `Float` do not convert

Not by subtyping, and not by a literal rule either. The language says nothing
about mixing them: `1 + 1.5` does not check, and neither does a `1` that came
out of a `let`. This is the strongest form of the no-subtyping stance in the root
[README](../../README.md), and it is also what the consumer's own schema already
takes — its `Int` and `Float` are distinct kinds and an `Int` value does not
satisfy a `Float` schema.

### 4.3 The JIT converts across the IR, on demand

**Where it can fire is the whole content of this decision, and it is narrower
than "wherever a float meets an integer".** §4.2 means a fragment is homogeneous
by construction: a kernel body is written in one type or the other, never both,
so there is no intra-expression conversion for the lowering to insert. The
conversion is a **boundary** conversion, at the two places where a class is not
already agreed:

- a buffer of floats reaching a fragment whose `LowShape` is `Unknown` — the
  conservative bottom, where the backend was always going to pick something;
- a fragment whose declared `IntWidth` disagrees with the class of the slot it
  reads or writes.

Both are places where **nothing above the backend decided**, which is the only
place a conversion can honestly be a lowering's choice rather than a type
system's.

**The invariant that has to be written down with this, not after:** the checker
never sees a conversion, so a wrong one is a wrong number and not a type error.
And `i64 → f32` is exact only below 2^24, so a large `Int` that reaches a kernel
as a float is a rounded answer that checked perfectly. This is the same class of
failure the whiting notes keep returning to — a wrong image nobody sees until the
render finishes — and the answer is the one they settled on: a note that says so,
beside the value, not silence.

## 5. What landing this would look like

Phase 0 is §3.1–§3.6: a float literal checks, prints, round-trips through an
artifact, and is **refused by every kernel** rather than silently mis-encoded —
which under §4.3 means the conversion does not exist yet, so phase 0's float is
a value only the interpreter can read.

Phase 1 is §3.7 and the operator set. Phase 2 is §3.8, and it is the only phase
that touches a backend.

The verification for phase 0 is four tests, one per round-trip that can fail
quietly: a literal's pair; the printer's spelling re-lexed and re-checked; an
artifact written and read back through `ArtifactCodec`; and a `cache` cell
holding a float, which is the freeze path and the one place a `Copy` payload with
no handle has to survive being filed under an occurrence path.

## See also

- [whiting-scene-document](whiting-scene-document.md) — the demand for this
- [plugin-taxonomy](plugin-taxonomy.md) — why this is a compiler plugin
- [operators](operators.md) — the operator set §3.7 edits, and `==` over two types
- [lowlevel-low-types](lowlevel-low-types.md) — the `LowShape` lattice §3.8 joins on
- [compute-jit-low-types](compute-jit-low-types.md) — what a backend traces from a shape
- [compute-graph-jit](compute-graph-jit.md) — the `i64` kernel ABI §3.8 moves
- [language-spec](../language-spec.md) — the syntax this changes
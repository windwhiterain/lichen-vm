# Floating point: `Float` as a ninth kind marker

> Status: **proposed.** Nothing here is implemented. This is the shape of the
> change and the list of decisions still open, not a description of what the code
> does today. Today the only numeric value the lowlevel has is `USize`.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowValue`, `LowShape`),
> `crates/lichen-lowlevel/src/codec.rs` (the value tags),
> `crates/lichen-highlevel/src/shape.rs` (`for_each_kind_marker!`),
> `crates/lichen-highlevel/src/program.rs` (`TypeValue`, `ValueType`),
> `crates/lichen-language-lex/src/lib.rs` (`RawToken`, `TokenKind`),
> `crates/lichen-language-parser/src/`, `crates/lichen-render/src/render/`
> (`struct_field_names`), `crates/lichen-kernel-ir/src/lib.rs` (`KernelBin`,
> `IntWidth`), and [language-spec](../language-spec.md) for the syntax.
> The demand is recorded in [whiting-scene-compiler](whiting-scene-compiler.md);
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
[whiting-scene-compiler](whiting-scene-compiler.md), where every visually
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

Seven groups, in dependency order. The first two are the ones that decide
whether the rest is cheap.

### 3.1 The lowlevel value

`LowValue::Float(f64)` beside `USize`. `f64` rather than `f32` because the
consumer's document is float-valued and a scene carries colour and transform
matrices; the four-byte boundary is a packing decision made where the buffer is
built, not a property of the value. `ValueExt` needs no work — a float is not a
handle and is not traced.

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

`TokenKind` gains `Float(f64)` beside `Int(usize)`, plus `KwFloat` beside
`KwInt` — the type constant is a keyword for the same reason `Int` is. Negative
literals are unary minus today and stay that way; `-1.5` is `Minus`, `Float`.

### 3.4 The parser and the literal's type

A literal builds a pair: `Float(f) : [float, K]` for `Float(f)`. The parser's
literal arm is where a `USize` literal is given `[int, K]` today.

### 3.5 The printer

`lichen-render`'s `ValuePrinter` spells a `LowValue`; an unhandled variant is a
diagnostic that prints nothing. `f64` needs a spelling that **round-trips**, and
that is a real constraint rather than a style one: the printer's output is what
`README` examples and the `readme.rs` sync test compare, and `1.0` printed as `1`
would read back as a different `LowValue` in a different language.

### 3.6 The artifact codec — two tag spaces, both full

Both are append-only and neither tag may ever move, so both are stated here
before any code:

| space | used | next | file |
|---|---|---|---|
| `LowValue` variants | `0`–`7` (`Void` took `7` additively) | **`8`** | `crates/lichen-lowlevel/src/codec.rs:244-246` |
| kind markers | `0`–`7` (`TypeString` is `7`, deliberately not its list position) | **`8`** | `crates/lichen-highlevel/src/shape.rs:58-62` |

Both next-tags being `8` is a coincidence, not a shared space. They are separate
encodings and one may be read without the other.

`Str` has no escapes and no concatenation, which is why it round-trips by being
atomic. A float has the same accident available and should take it: printing is
the only place a float becomes text, and a shortest-round-trip format keeps that
one place honest.

### 3.7 The operators

[operators](operators.md) is the note that owns the operator set. A float needs
`+ - * /` and the four comparisons, and each raises the same question the integer
set already answers: whether one operator leaf dispatches on the operand's class
or whether a float leaf exists beside the integer one. The answer is recorded
there and must not be restated here.

## 4. Decisions this note does **not** make

Each is a genuine fork, not an oversight, and each is cheaper to decide before
the lexer changes than after.

**Width and rounding.** `f64` in the value is proposed above. What a float
*rounds to* when an artifact is written, and whether `Float` is a machine float
or an exact rational, is open. An exact rational would be a much better fit for a
language whose thesis is "types are values" and a much worse fit for a buffer
that has to be four little-endian bytes per component.

**Whether `Int` unifies with `Float`.** lichen has no subtyping, and the warning
in the root [README](../../README.md) is explicit that a compound type is typed by
its kind. Three answers, all defensible:

- *distinct, no widening.* `1 + 1.5` does not check. Honest, and it makes every
  mixed expression an explicit conversion — which is a conversion operator that
  does not exist yet.
- *distinct, widening at the literal only.* An `Int` **literal** in a position
  the checker has decided is float-typed widens; an `Int` **value** does not.
  This is not subtyping — it is a literal rule, and the checker already knows a
  literal's provenance because it just built it. It is the answer that makes
  scene documents readable without giving up the no-subtyping stance.
- *widen by unification.* This is subtyping, and it contradicts the README.

The consumer's own schema takes the same position as the first answer — its
`Int` and `Float` are distinct kinds and an `Int` value does not satisfy a
`Float` schema — which is evidence for "distinct" and says nothing about the
literal rule.

**Whether a float reaches a kernel.** `KernelBin` is integer-only and
`IntWidth` has exactly one variant, `I64`; `KernelInstr::Const` carries an `i64`.
So today a float cannot be `jit`-ed, `launch`-ed, or dispatched to a device. A
`LowShape::Float` that no backend traces is a value only the interpreter can
read, which is a narrower feature than it looks and should be described that way
until [lowlevel-low-types](lowlevel-low-types.md) and
[compute-jit-low-types](compute-jit-low-types.md) say otherwise. Whether the
kernels widen is a separate, larger decision: it touches the wasm and SPIR-V
backends and the `i64` scalar ABI that
[compute-graph-jit](compute-graph-jit.md) is built on.

**Whether a float is a `Str`-like opaque scalar or a first-class number.** `Str`
is atomic and `Copy`; a float can be both, and staying `Copy` with no payload
handle is what makes it free in the GC and in the freeze path.

## 5. What landing this would look like

Phase 0 is the whole of §3.1–§3.6 with §4's first three decisions answered, and
nothing else: a float literal checks, prints, round-trips through an artifact,
and is refused by every kernel rather than silently mis-encoded. Phase 1 is
§3.7. Phase 2 is the kernel widening, if it is ever wanted.

The verification for phase 0 is four tests, one per round-trip that can fail
quietly: a literal's pair; the printer's spelling re-lexed and re-checked; an
artifact written and read back through `ArtifactCodec`; and a `cache` cell
holding a float, which is the freeze path and the one place a `Copy` payload with
no handle has to survive being filed under an occurrence path.

## See also

- [whiting-scene-compiler](whiting-scene-compiler.md) — the demand for this
- [plugin-taxonomy](plugin-taxonomy.md) — why this is a compiler plugin
- [operators](operators.md) — the operator set §3.7 edits
- [lowlevel-low-types](lowlevel-low-types.md) — `LowShape` and what a backend traces
- [compute-jit-low-types](compute-jit-low-types.md) — the `i64` kernel ABI
- [language-spec](../language-spec.md) — the syntax this changes
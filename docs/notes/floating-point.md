# Floating point: `Float` as a ninth kind marker

> Status: **phases 0 and 1 landed; phase 2 proposed.** `LowValue::Float(f32)`,
> `LowShape::Float`, the `TypeFloat` kind marker, the `Float` keyword and its
> literal, the printer's round-tripping spelling, and `+ - * /` with the four
> order comparisons are implemented and tested. `==`/`!=` over two floats route
> through [`ValueExt::value_eq`], so they are the same relation unification uses.
> **§3.8 (the kernels) is not**: a float is still refused everywhere a kernel
> could take it, by name and at every position. Today the lowlevel *offered*
> `USize` alone; it now offers both, and they are unrelated types.
>
> Points at: `crates/lichen-lowlevel/src/lib.rs` (`LowValue`, `LowShape`,
> `ValueExt`), `crates/lichen-lowlevel/src/codec.rs` (the value tags),
> `crates/lichen-lowlevel/src/equality.rs` (`observed_low_shape`),
> `crates/lichen-highlevel/src/shape.rs` (`for_each_kind_marker!`),
> `crates/lichen-highlevel/src/program.rs` (`TypeValue`, `ValueType`),
> `crates/lichen-language-lex/src/lib.rs` (`RawToken`, `TokenKind`),
> `crates/lichen-language-parser/src/`, `crates/lichen-render/src/render/`
> (`ValuePrinter`), `crates/lichen-language/src/persist/container.rs` (the shape
> tags), `crates/lichen-kernel-ir/src/lib.rs` (`BufferSlot`,
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
can see. The variant alone is not enough: a value's shape is read through
`observed_low_shape`, so that function needs a `Float` arm or a float value
states no shape at all and §3.8's boundary conversion never has one to disagree
with.

**A float compares by its 32 bits, never by IEEE `==`.** `LowValue`'s derived
`PartialEq` is replaced by a hand-written one: every other variant compares
field-wise exactly as before, and the float compares with `to_bits`. The derived
relation is wrong for this class in both directions — it says `0.0 == -0.0`,
which would let a reuse decision accept one of two distinct artifacts, and
`NaN != NaN`, which would make a cached `NaN` never match its own reload.

Every reader of that identity compares a value against one the codec reproduced
**bit for bit**: two classes unifying on one concrete value, two table keys
comparing as the same content, and a frozen artifact against the value a load
reuses. "Equal but not the same bits" has no meaning for any of them.

That decision carries exactly one obligation, which is paid in the same change:
the content hash owes *equal keys hash equal*, so `hash_step` hashes the bits
too. A total equality and a matching hash are one contract, not two.

Note this is deliberately **not** the relation the language's `==` will use —
that is §3.7's, and the two are kept apart on purpose.

### 3.2 The kind marker — the one list

One entry in `for_each_kind_marker!`:

```rust
/// The `float` type constant — `Float` literals pair with `[float, K]`.
TypeFloat { 9, "float", float_marker, float_marker_node }
```

**The tag is `9`, not `8`, and the reason is the one thing about this list that
is easy to get wrong.** The kind markers do not have a tag space of their own.
Each entry's tag is a tag in the **`TypeValue` codec**, and that codec has one
more arm the registry does not own: `TypeValue::TypeId` — a nominal struct
identity, not a kind marker — holds tag `8`, spelled by hand
(`crates/lichen-highlevel/src/program.rs:564-568` and `:581`). So the next free
tag for a ninth marker is `9`.

**A collision here is silent, and that is worth stating before anyone allocates
one.** The registry's arms are expanded *before* the hand-spelled `TypeId` arm
on both sides, so a marker claiming `8` makes `8 => TypeValue::TypeId`
unreachable and every persisted nominal id decodes as that marker with a stray
`u64` left in the stream. The compiler reports `unreachable_patterns`, which is
a **warning**: the build passes. The failure is a wrong `TypeId` read back as
something else, discovered by running something.

Note the asymmetry, because it is the right shape and worth preserving: a *gap*
in the tag space hits the catch-all and is a clean `unknown type-value tag N`
error, while a *collision* shadows an arm and corrupts silently. Any guard added
here should tighten the second without loosening the first.

and everything else follows from the list by construction: the `TypeValue`
variant, the `ValueType::float_marker()` constructor, the `Ctx` accessor, the
checker's installed field and dispatch, and the artifact codec. **This is why
step 3.1 is cheap and the whole change is not:** the derived work is seven
things generated from one line, and the one line is a codesign.

Nothing above the lowlevel is needed for this step alone: a marker with no
literal is inert rather than broken, and a `[float, K]` type that reaches
`low_type_of` without a float value behind it falls through to `LowShape::Unknown`
— which is §5's phase-0 requirement, arrived at by default.

### 3.3 The literal

`RawToken` gains `[0-9]+\.[0-9]+` beside `[0-9]+`. Logos takes the longest match
within the lexer, so `3.5` is one token while `x.5` and `x . y` stay as they
are. Three shapes had to be decided rather than assumed, and each is stated in
the pattern or at the site rather than left to a reader to infer:

- **A digit is required before the dot.** With `[0-9]*\.[0-9]+`, `.5` alone would
  lex as one float; requiring the leading digit states in the pattern that a
  leading dot is field access, rather than relying on `NameLit` winning by
  priority at an earlier offset.
- **An overflowing literal is an infinity, not a lex error.** `Int` refuses an
  overflow because `usize` has no infinity for the value to become; `f32` has
  one, and the operators produce it too, so refusing it here would make the
  literal the only source of a value the language can otherwise hold.
- **No scientific notation.** `1.5e3` is `Float(1.5)` then the name `e3`. This
  is a scope decision, not an oversight — it should be recorded here so it is not
  rediscovered as a gap.

`TokenKind` gains `Float(f32)` beside `Int(usize)`, plus `KwFloat` beside
`KwInt` — the type constant is a keyword for the same reason `Int` is.

**There is no prefix minus, and so a float literal is never negative.** `Minus`
appears exactly once in the parser, as the binary `BinOp::Sub`; the only prefix
operator is `!`. `-1` is therefore a **parse error today**, which is worth
stating because it is easy to assume otherwise from `-` being a token. A float
literal needs nothing here, and unary minus over floats — which would be the
first negation in the language — belongs to §3.7 if it is ever wanted at all.

**`Eq` comes off `TokenKind`, `Token` and `RawToken`, and no bits stand-in
replaces it.** Nothing folds a token into a key — every consumer keeps tokens in
a `Vec` and compares with `==` — and `Eq` on an `f32` payload is not merely
undesirable but impossible. Note this is the **opposite** of §3.1's decision for
`LowValue`, on purpose: see below.

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

This is not hypothetical: the consumer hits all three. Its `Value::Float` `Display`
is `inner.to_string()`, and Rust's float `to_string` prints `1` for `1.0`. And
its **document format** fails on the same three: `-0.0` reads back as `+0.0`,
`NaN` and `±inf` are written as `null` and read back as `0.0` **with no error**,
and a large magnitude whose shortest form is a bare integer of forty or more
digits does not parse at all.

The second of those is the one to design against. §4.3 lets the lexer produce an
infinity, and the format turns an infinity into `0.0` silently — so a float that
checked perfectly becomes a different float with no message. A printer that
round-trips is therefore not a nicety here; it is the only thing standing between
§4.3 and a wrong scene.

`Str` has no escapes and no concatenation, which is why it round-trips by being
atomic. A float should take the same accident, with the one addition `Str` does
not need: printing is the only place a float becomes text, so that place has to
keep the digits Rust's own shortest-round-trip formatting chooses, and nothing
else may re-spell a float.

### 3.6 The artifact codec — three tag spaces

All three are append-only and no tag in any of them may ever move, so all three
are stated here before any code. They are separate encodings: one may be read
without the others, and the same value is written under a different tag in each.

| space | used | next | file and crate |
|---|---|---|---|
| `LowValue` variants | `0`–`7` (`Void` took `7` additively) | **`8`** | `crates/lichen-lowlevel/src/codec.rs:244-246` |
| kind markers | `0`–`7` (`TypeString` is `7`, deliberately not its list position) | **`9`** | `crates/lichen-highlevel/src/shape.rs:58-62` |
| `LowShape` variants | `0`–`5` (`0`–`4` decided, `5` is `Unknown`, the lattice's bottom) | **`6`** | `crates/lichen-language/src/persist/container.rs:135-164` |

There is a **fourth** versioned thing, which is not a tag space but behaves like
one: the AST content key. `crates/lichen-language/src/resolve/content_key.rs`
carries `KEY_FORMAT_VERSION` (currently `2`) and its own contract says that adding
an `Expr` variant **bumps it** — an `Expr::Float` that does not is a retained key
that silently collides. The key is also a place a float becomes text, so it owes
the same discipline as the printer: hash the float's **bits**, for the same
reason `LowShape`'s table's sibling does and for the same reason `f32` is not
`Eq`.

Four traps in this table, each of which has already cost somebody something:

- **The kind-marker row is `9`, not `8`.** That space is the `TypeValue` codec's,
  and `TypeValue::TypeId` already holds `8` without being a kind marker. See
  §3.2, which also explains why a collision there is a warning rather than an
  error.
- **The first two rows both landing on the same low number is a coincidence.**
  They are separate encodings and one may be read without the others.
- **The third is the one that is easy to miss**, because `LowShape` is a
  **lowlevel** enum and its persisted form lives in `lichen-language`, not
  beside it — so the crate that owns the value owns only one of the two tag
  spaces its change needs.
- **The fourth is a counter, not a tag, and the failure is the same shape**: a
  stale retained key is a wrong answer rather than a rejected read.

### 3.7 The operators

[operators](operators.md) is the note that owns the operator set, and §4's
second decision is what a float means *to* that set, so the two are read
together. A float takes `+ - * /` and the four order comparisons; it does **not**
take `%`, the bitwise trio, or a conversion operator — the first two because
they are not in its set, the third because §4.2 makes it a contradiction.

**Which class an operation runs over is decided by its operands, and an operand
that names no class leaves the operation where it was.** A concretely `Float`
operand makes the whole operation `Float` and pins the other operand to it, so
`x + 1.5` types `x` as a `Float`; with neither operand naming a class the
operation stays `Int`, so `x + 1` is unchanged from what it always was. Pinning
a free cell is this language's ordinary unification, and a concrete `Int`
operand cannot be pinned — which is why `1.5 + 1` is a reported
`expected Float, found Int` rather than a coercion.

**Division by zero is IEEE for a float, and refused for an `Int`.** `1.0 / 0.0`
is `+inf`, `0.0 / 0.0` is `NaN`, and no divisor is recorded. This is the only
answer consistent with §3.3's value set — an overflowing literal is already an
infinity and the printer already spells one — and with `operators.md` §6, which
refused "zero is refused, except when it is not" for the JIT: wasm's float
division by zero yields `inf` without a trap and SPIR-V's is undefined, so an
interpreter refusal beside an IEEE kernel answer is a divergence that becomes
visible the moment §3.8 lands. `operator.divide_by_zero` is therefore the
**integer** `Div`/`Rem` category only.

`==`/`!=` are the one place the decision has a visible edge, and it resolves the
way the rest of the language resolves things: **by asking the value.** They are
the **generalized** equality over "any two same-typed values"
(`operators.md` §1), and the mechanism for "same-typed values are equal" already
exists as [`ValueExt::value_eq`] — which is where a float's equality was decided
in §3.1, at the float's own site. `==` routes through it. So:

- `1 == 1.5` **does not check**, because the operands are differently-typed
  values and the checker unifies their types first (§4.2 forbids conversion in
  either direction). Any answer that made it check would be a conversion wearing
  a comparison's clothes.
- `1.5 == 1.5` is `1`, `0.0 == -0.0` is `0`, and `NaN == NaN` is `1`.

That last line is the price, and it is the right price: it is what "there is one
relation" means. `==`, unification of two concrete values, table-key comparison
and frozen-artifact reuse all read [`ValueExt::value_eq`], so they agree **by
construction rather than by discipline** — and a second relation for the
language would have reintroduced exactly the divergence that one relation
removes. IEEE was considered and declined: it is not an equivalence relation
(`NaN != NaN`), so it could not live in [`ValueExt::value_eq`] without breaking
the hash tables and the marker lookups that read it.

**One behaviour moved as a consequence, and it is not float-specific.** `==`
used the derived `PartialEq`, which compares a handle-carrying leaf by pointer;
`value_eq`'s documented contract is identity **by content**. So two values of a
leaf that answers `is_handle()` — today `ComputeValue::Buffer` is the only one —
now compare by payload rather than by allocation. That is the relation
unification and table keys have always used for the same leaf, and it is what
"generalized equality over any two same-typed values" should have meant, so this
is a correction rather than a regression. It is recorded because a `==` on two
buffers is a thing someone can write and the answer changed.

[`ValueExt::value_eq`]: crates/lichen-lowlevel/src/lib.rs

### 3.8 The kernels

`LowShape` gains `Float` beside `USize`, and the join needs no new rule: the
lattice already sends two different decided shapes to `Unknown`
([lowlevel-low-types](lowlevel-low-types.md)), so `USize ∨ Float` is `Unknown`,
which is the bottom a backend already falls back on. That part is free.

The rest is not free, because the kernel ABI was `i64` in five places at once:

| what | where |
|---|---|
| `BufferSlot::Host(&'a [i64])` | `crates/lichen-kernel-ir/src/lib.rs:64` |
| `ParallelBackend::run(…, inputs: &[BufferSlot], …)` | `:156` |
| `ParallelBackend::fetch(id, count) -> Result<Vec<i64>, String>` | `:192` |
| `KernelInstr::Const(i64)` | `:371` |
| `IntWidth { I64 }` | `:267` |

**That list is accurate and badly incomplete, and saying so is the point.** It is
five sites *in one file*, and it misses that the ABI **cannot carry a class at
all**: `KernelShape`'s leaf had none, `KernelBin` is *defined* as the
unsigned-integer operations, and `KernelFragment`'s one class-ish field was
fragment-wide. So §4.3's "conversion at the boundary" had nothing at the boundary
to convert *to*. The real inventory is roughly seventy sites across five crates —
the ABI's consumers include `lichen-graph-ir`, which the note never mentioned, and
`ParallelBackend` has six implementors — and about thirty of them are load-bearing.

**The carrier has since landed** (§4.4's decisions, in `lichen-kernel-ir` and its
consumers), so of the five rows above: `fetch` returns `ScalarData`, `ResidentBuffer`
carries a `ScalarClass`, `KernelShape::Scalar` carries one, and `KernelFragment`
gained `input_classes` / `output_classes` — all hashed by `fragment_digest`. **A
float is still refused at every position**, because the carrier landed ahead of
the permission; the two refusal tests in `crates/lichen-language/tests/compute.rs`
are the witness that nothing observable moved.

**Two carriers remain open, and both belong to the step that permits floats.**
`BufferSlot` is deliberately still `Host(&[i64])` — no producer of a float slot
exists, and a variant with no constructors would be dead code in every backend's
match. But `run` takes `&[BufferSlot]`, so a float producer forces the decision:
either the host reinterprets `f32` bytes as `i64` words, which makes the slot's
`len()` and the fragment's `count` mean different things, or `BufferSlot` is
widened and gives up `Eq`. And `graph.rs`'s `returned_role` drops the class,
because `Value::device(id, count)` has nowhere to put one — the one path where a
float graph result could come home mislabelled.

**Where a float is refused is not a single question, and answering it once is how
a hole gets opened.** `lichen-compute` has three sites that all match on
`LowShape` and all look like the same question. Two of them are not:

| site | question | `Float` |
|---|---|---|
| `kernel_shape` | what *shape* — one value, or a tuple of them | `Scalar` |
| `domain_is_known` | can the ABI lower this to decided `i64` locals | **`false`** |
| `flat_arity` | how many locals a domain flattens to | `1` |

All three read **one recursive walk**, `domain_obstacle`, which answers not a
bool but *which* obstacle a position holds — `Undecided` for an `Unknown` leaf,
`Float` for a float leaf — and combines them with `max`, so the strongest cause
wins and the one with the derive's variant order to match. `domain_is_known` is
that walk read as a boolean, and `kernel_domain` reads the same walk to name the
cause, so **the gate and the message cannot disagree about a position** — which
is the point, because they did once.

`Float` is the load-bearing answer and it is `false` **for a different reason
than `Unknown`'s**: `Unknown` is undecided, a float is decided and has no local
to be decided into. That distinction is why the walk returns a reason rather
than a bool — a gate alone cannot tell a reader which of the two it hit, and
telling a float domain to "annotate the parameter" asks for something the author
already did.

**The walk recurses through every position, and that is where a hole was.**
`kernel_domain`'s `Tuple` arm accepts without re-checking elements, so the walk
is the only thing that can see inside. Two positions are reached that a
first-position-only walk would miss, and both were **admitted** before the walk
existed: `Array(Float, 3)` nested in a tuple, because the old compound arm asked
only `element.is_known()` and a float is a decided shape; and `Function(USize,
Float)`, whose codomain the old arm skipped on the stated grounds that "those two
positions are decided together or not at all" — which that shape disproves,
because the domain is decided and the codomain is a float. Neither is reachable
now.

The uniform walk has one consequence worth stating: an `Unknown` **codomain or
value** position inside a tuple domain now refuses at compile time rather than
being admitted and failing later at launch. That is a strictly earlier and
strictly clearer refusal — it names the placeholder the author can fill — and no
position flipped from refused to admitted.

A float still cannot reach a `ParallelBackend` even so, for reasons that hold
independently of the above: inputs are `i64` *by type*, every producer of a
buffer is an `i64` source, and `collect_args` refuses a float leaf.

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

**A measurement made afterwards is a second, independent reason for `f32`, and it
is worth keeping next to the first.** The consumer's document format does not
round-trip every `f64`: its writer spells a large float as a bare decimal
integer, and its reader refuses an integer literal of **forty or more digits** —
so `1e38` loads and `1e39` does not. `f32::MAX` is ≈3.4e38, which in that form is
thirty-nine digits and therefore **just inside the boundary**, while `f64::MAX` is
three hundred and nine digits and walks straight off it.

That is luck rather than design, and it should not be leaned on: the width was
chosen for the hardware and for the packed component, and this is what it
happens to buy. What it buys is that a scene's coordinates cannot reach the
cliff by accident, which `f64` would have let them do.

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

### 4.4 The phase-2 decisions, taken before any of it is written

Phase 2 is not one edit. A survey of the real blast radius — the note's original
"five places" was accurate as far as it went but missed four whole classes of
site, and the ABI **cannot carry a class at all** (`KernelShape`'s leaf has none,
`KernelBin` is defined as unsigned-integer operations, and `KernelFragment`'s one
class-ish field is fragment-wide) — found **five** decisions that the code cannot
answer for itself. They are settled here because three of them decide what every
implementation agent has to build, and settling them after the code would mean
rewriting it.

**Which positions are permitted.** Every position `domain_obstacle` walks,
including the two compound ones phase 0 closed deliberately: an `Array(Float, _)`
nested in a tuple, and a function's codomain. Phase 0 refused them because the
ABI had nowhere to put a class, not because a float there was meaningless.

**What a compound position is.** Following the existing filler — `flat_arity`
answers `1` for every non-scalar and non-tuple today, as a total function — an
`array<Float, 3>` parameter is **one local of the element's class**, not three.
That is what keeps `flat_arity`'s convention and `KernelShape`'s fold agreeing,
and it means "one class per fragment" still holds.

**The class rides in two places, not one.** The **parameter** class goes on
`KernelShape`'s leaf, and `fragment_digest` already hashes `param_shape` whole, so
its coverage is free. The **buffer element** class goes on a `KernelFragment`
field beside `inputs`/`outputs` — because `inputs` is already barred from
`param_shape` for a reason that applies verbatim ("a parallel fragment's shape is
`(config, index)` however many buffers it reads": that domain is integers, its
buffers are not) — and it must be added to `fragment_digest`. A
purely-fragment-carried or a purely-value-carried class cannot cover both, and
`fetch` forces the read-back's class onto the value because its implementor has
no fragment at all.

**`IntWidth` keeps meaning integer width.** A `ScalarClass` beside it says the
class, and `I32` is still free to take the width slot the field's own doc
reserves for it. Widening `IntWidth` with an `F32` would make `bits()` stop
answering its question — `I32` and `F32` are both 32 — and every existing
`bits() != 64` check would accept a float fragment as an integer one.

**Float division is not specified inside a kernel, and that is the answer.**
SPIR-V's `OpFDiv` with a zero divisor is undefined; wasm's `f32.div` is IEEE. The
language does not check it and does not promise it: a kernel that divides floats
by zero returns what its backend returns. This is outside the kernel-safe subset
— which [operators](operators.md) defines as the intersection of what the two
backends compute the same way — and it is the price of admitting floats at all.
**§3.7's interpreter answer is unchanged**: `1.0 / 0.0` is `+inf` in lichen,
because the interpreter is not a kernel. The thing this note's §4.3 was shaped
around, a boundary conversion, is what a float **parameter** needs; it says
nothing about a float **operator** whose result a backend computes.

## 5. What landing this would look like

Phase 0 is §3.1–§3.6: a float literal checks, prints, round-trips through an
artifact, and is **refused by every kernel** rather than silently mis-encoded —
which under §4.3 means the conversion does not exist yet, so phase 0's float is
a value only the interpreter can read.

**Phase 0 is not one agent's work, and the reason is §3.6.** The three tag
spaces live in three different crates, and so do the exhaustive matches a new
`LowValue` variant breaks: the value and its shape in `lichen-lowlevel`, the
marker in `lichen-highlevel`, the printer in `lichen-render`, the shape's own
persisted tag and the `LowShape` matches in `lichen-compute` in
`lichen-language`. An agent scoped to one crate will finish its crate and leave
the workspace not compiling, which is expected rather than a failure — the phase
lands when the parts are merged together.

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
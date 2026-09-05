# Extensible attributes (typed perspectives)

> Status: current
> Points at: `crates/lichen-highlevel/src/attr.rs` (the extension point), `ir.rs`
> (the `Schema`), `checker.rs` (`check_unify_relaxed`), and
> `crates/lichen-language/src/program.rs` (the `Perspective` attribute + `GcdOp`).
> Inspired by [the typed-perspectives paper](../reference/perspectives-paper.md).

A program already wraps every expression in a `[value, type]` pair. The attribute
system adds **optional extra slots** whose *shape* is a compile-time `Schema` and whose
*semantics* come from a pluggable extension. It is lichen's first genuinely static idea.

## Schema: a compile-time type

The ordinary "type" is a **runtime** value (the pair's slot 1). The **schema** is a
compile-time (static) type: it names *which* attributes ride on an expression and in
which slots, and is consumed at lowering, then gone — never a runtime node, never
unified, never cloned.

```
Schema<A> { tail: Vec<A> }    // [] => [value, type]; [Perspective] => [value, type, attr]
```

`e # p` stamps the expression's schema with the attribute tail, so the expression is
lowered to a **3-wide pair** instead of a 2-wide one. **Nothing has a fixed arity** —
the highlevel reads the schema and builds the pair at exactly that arity, padding an
absent attribute with the extension's `missing_value` at every unify site so the
positional lowlevel unify just works.

## The extension point (`AttrExt`)

The checker is attribute-agnostic: it only knows the *shape* — "an attribute combines
over its children, an absent occurrence reads its `missing_value`, and two slots unify
by `unify_slots`." Every concrete operation is supplied by an extension in the layer
that defines the attribute; highlevel ships the inert `NoAttr` marker.

```
AttrSpec     marker bound (Copy + PartialEq + Eq + Debug)
AttrExt<P>   slot() / missing_value() / combine() / unify_slots() / is_subtype()
```

## Perspective

`Perspective` (in `lichen-language`) is the first attribute: a plain non-negative
integer whose lattice is **divisibility**, not numeric size. "Uniform over `n` aligned
threads."

| quantity | value | role |
|---|---|---|
| subtype | `a \| b` | `2 ⊑ 4`; `4` and `6` are incomparable |
| meet (combine rule) | `gcd` | `gcd(4,6) = 2` |
| top / missing | `0` | `gcd(n,0) = n`; a value with no `#` is "uniform everywhere" (the top) |
| bottom | `1` | divides everything |

Two traps from the design: **the order is divisibility** (`2` is a *subtype* of `4`),
and **`0` is the top** (not `∞`), because it is the divisibility identity and the GPU
"uniform over all threads" fold.

The combine operator is `GcdOp::Gcd`, an n-ary gcd meet, defined as a **language-layer**
operator — `LangOperator` is a union over `HighProgramOperator` + `GcdOp` — so the
lowlevel/highlevel core never names it. An absent perspective reads `USize(0)`.

## Subtyping: checking is a generalised unify

`check_unify_relaxed(a, b, loc, kind, is_subtype)` attempts an equality unify; on
failure it retries through the attribute's `is_subtype` and suppresses the error if the
partial order holds. `is_subtype` is invoked through the curated [`Ctx`](...)
(`&dyn Ctx<P>`), so the relation reads slot values — never raw lowlevel nodes.
`Perspective` overrides it as **`declared ⊑ value`
(`declared | value`)**: an aligned `n`-group partitions into `q`-groups, so a value
uniform over `n` is usable where `q` is declared iff `q | n`. Since `0` is the top,
`divides(0, sup) ⟺ sup == 0` (a `#4` value is not "uniform everywhere") and
`divides(n, 0)` holds (a uniform-over-all value satisfies any `# n` requirement).

### Behaviour (the acceptance table)

The `perspective.rs` integration tests encode this table.

| program | result |
|---|---|
| `1 # 4` | perspective `4` |
| `((1 # 4) + (2 # 6)) # 2` | slot `gcd(4,6) = 2`, check `2 ≡ 2` ✓ |
| `((1 # 4) + (2 # 6)) # 5` | `2 ≢ 5` ✗ |
| `((1 # 4) + 2) # 4` | `gcd(4,0) = 4` ✓ |
| `id (5 # 4)` | `0 ≢ 4` ✗ (a `#4` value is not uniform over all threads) |
| `f = x # 4 => x; f 5` | `4 \| 0` ✓ (a uniform value satisfies a `#4` requirement) |
| `f = x # 2 => x; f (5 # 4)` | `2 \| 4` ✓ (real subtype relaxation) |
| `f = x # 4 => x; f (5 # 2)` | `4 ∤ 2` ✗ |

### Annotation over an existing attribute (requirements & providers)

An `expr # p ? d` over a value that **already carries** a constraint unifies the
annotation against the value's *existing* attribute and keeps that existing value as
the slot. The annotation is the **requirement** (a subtype); the value's own/existing
attribute is the **provider** (a supertype). So the check is `requirement ⊑ provider`
(`requirement | provider`):

| program | result |
|---|---|
| `(5 # 8) # 4` | provider `8` kept, `4 \| 8` ✓ → slot `8` |
| `(5 # 4) # 8` | `8 ∤ 4` ✗ → "expected 8, found 4" |
| `x = 1 # 8; x # 4` | provider `8` kept, `4 \| 8` ✓ → slot `8` |
| `x = 1 # 8; x # 16` | `16 ∤ 8` ✗ |

The provider is the value's own attribute slot (a value that is itself annotated, or a
bound name carrying an attribute) or, for a compound, the `gcd`-meet of its
sub-expressions' slots. A plain leaf with no attribute of its own has no provider, so
the annotation *is* the slot (`1 # 4` → `4`).

## Syntax

`expr [: expr] [# expr] [? expr]` — `:` fills the type slot, `#` fills the
perspective (constraint) slot, `?` fills the doc (label) slot. A
`x # n => e` parameter (and `x : T # n => e`) is accepted; the frontend desugars a
parameter annotation to a leading body statement (`x => { x # n; e }`) and keeps it as
an optimization on `ExprKind::Function.parameter_attribute`. See the
[language spec](../language-spec.md) for the grammar and `#` precedence.

### Labels (`?`)

`?` is the **label** attribute slot — metadata that attaches to an expression but
carries no constraint (see [`Doc`](doc-attribute-rework-plan.md)). Unlike `#`
(a constraint a compound lives with and that the apply-time check enforces), a label
contributes no apply-time constraint slot, and the attribute's own `is_subtype` (a doc
returns `true`) is what permits `? b` to override an existing `? a` without conflict —
the checker never special-cases a label's *unification*, only its metadata slot. A
label's value is any first-class lichen value (by convention a struct instance); the
renderer reads the value's field *names* from its **type chain** (the label's runtime
slot carries the annotation value's `[value, type]` term pair), so nothing about a
label's shape is hardcoded.

## Non-goals (currently)

- The apply **checks** an attribute (equality + subtype) but does not yet *flow* it out
  through a function (auto-derivation); the return value reads the body's slot.
- A second *constraint* attribute in the same program (only `Perspective` ships;
  `Schema::tail` is a `Vec`, so one could be added). Labels like `Doc` already share
  the tail.
- Observing the perspective slot in the CLI / spec output.

## Decision log

- The ordinary type is a runtime value; only the *schema* is static — lichen's first
  static thing.
- `#`, not `@`: the preprocessor owns `@`.
- The `Gcd` operator lives in the language layer (`LangOperator`), not in the highlevel
  `TypeOperator`, so the core only provides the mechanism.
- Subtype relaxation was originally a stage-2 non-goal; it is now implemented via
  `check_unify_relaxed` + `AttrExt::is_subtype`.
- A label's runtime render slot carries the annotation expression's `[value, type]`
  pair, so the renderer walks the value's whole type chain; a constraint slot keeps its
  bare lifted value (the lattice machinery reads `attr[e]`, never the render slot).

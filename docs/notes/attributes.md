# Extensible attributes (typed perspectives)

> Status: current
> Points at: `crates/lichen-highlevel/src/attr.rs` (the extension point and the
> canonical order), `shape.rs` (the pair layout: `attr_slot`), `ir.rs`
> (the `Schema`), `checker.rs` (`check_ann`), and
> `crates/lichen-language/src/program.rs` (the `attrs` manifest — the one list
> that fixes the order).
> Inspired by
> [Modular GPU Programming with Typed Perspectives](../reference/Modular%20GPU%20Programming%20with%20Typed%20Perspectives.pdf).

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
absent attribute with the extension's `missing_slot` at every unify site so the
positional lowlevel unify just works.

Every attribute's slot is the annotation value's **`[value, type]` term pair** — the
uniform shape. A constraint reads its lattice value from that pair's element 0; a label
uses the whole pair. This is what lets the checker's slot handling be attribute-agnostic
(`# 4` is a real expression `4 : Int`; `? doc` is a real expression `doc : Doc`).

## The canonical attribute order (where a slot comes from)

A pair's attribute slots are a **layout**, and the layout is declared **once**: by the
host composition's `attrs` manifest, in order.

```
lang_compose_vocabulary! {
    attrs = [ Perspective as Perspective; Doc as Doc; ]   // ← the canonical order
    …
}
```

That list *is* the canonical order. The composition derives from it

- `LANG_ATTR_ORDER` — every attribute, in slot order (the `i`-th attribute is at pair
  slot `attr_slot(i)`, i.e. `PAIR_ATTR_BASE + i` in `lichen-highlevel::shape`);
- `AttrSet::order_index` — the index the frontend, the checker and every reader ask;
- a build-time check that each index is its position.

So `Perspective` is the first attribute and `Doc` the second **by declaration, not by a
number anyone typed**: a plugin's marker supplies `AttrSpec` + `AttrExt` (behaviour) and
appears in the manifest, and nothing else. The old `AttrExt::slot()` — with
`Perspective` claiming `2` and `Doc` claiming `3` in their own crates, plus the
checker's sort and the frontend's push order agreeing by hand — is gone; a mismatch
used to be a *silent* mis-unification, and there is now no second list to fall out of
step.

Two consequences worth stating:

- The order is a **compatibility contract**: it is what a compiled artifact's pairs
  encode, so reordering the manifest renumbers persisted layouts (the same rule the
  codec tags follow). It is pinned by a test.
- **No hard limit** on the number of attributes exists in the encoding — a pair's arity
  is whatever its schema tail says. The only guarantee is the one above: indices are
  dense and distinct by construction.

An expression's pair is dense over the attributes it *actually carries*: a `? doc`
alone is `[value, type, doc]`, so "slot 3" is the slot Doc takes in a pair that also
carries a perspective. Readers go through the tail position, never through a hard
number.

## The extension point (`AttrExt`)

The checker is attribute-agnostic: it only knows the *shape* — "an attribute combines
over its children, an absent occurrence reads its `missing_value`, and two slots unify
by `unify_slots`." Every concrete operation is supplied by an extension in the layer
that defines the attribute; highlevel ships the inert `NoAttr` marker.

```
AttrSpec     marker bound (Copy + PartialEq + Eq + Debug)
AttrSet      the composed set: ORDER + order_index() — the slot layout
AttrExt<P>   missing_value() / missing_slot() / combine() / unify_slots() /
             is_subtype() / is_label() / constraint() / label() / render()
```

`AttrExt` carries **no layout**: "which slot" is the set's order, "what the value
means" is the extension's.

Three of the hooks are for a *value* rather than for a slot: `constraint()` is the
condition the attribute imposes on the annotated expression's own value (registered
through the assert channel), `label()` is the **name** it gives that value (a
labelled value *reads* as `?name`), and `render()` spells the attribute in a
signature. `render()` receives the composed extension registry as well as the slot,
because an attribute whose slot holds a **nested pair** — a refinement's slot *is*
its predicate's pair — must look for a name inside it, and which attributes that
pair carries is not in the graph (`attr::pair_label`; see
[operator-polymorphism](operator-polymorphism.md) §8.1).

## Perspective

`Perspective` (in `lichen-perspective`) is the first attribute: a plain non-negative
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

### Nothing consumes it yet

`Perspective` is **live and fully checked** — `#p` reaches the IR from the grammar
(`compile.rs` spells `LangAttr::Perspective`), it is in the persist codec, and the
table below is the acceptance suite — but **no part of the runtime reads it**. It is
an unused asset, not a half-built one, and it is the static half of a GPU execution
model: the certificate for which expressions are uniform across an aligned lane group.

It is worth recording a conclusion that was reached the wrong way first, because the
wrong version is the tempting one. The obvious claim is that a GPU backend *needs*
this, because "the body must be index-free" — that is false. A compute shader indexes
freely, and a dispatch that gives each lane one index maps straight onto
`lichen-compute`'s existing index-based parallel kernel, one lane per index, with no
rewrite of the body. Being index-free is a precondition only for **vectorising across a
lane group**, which is a different (and larger) win than parallelism. What
`Perspective` would buy is the part a dispatch *cannot* give: hoisting a
uniform (`#0`) subexpression out of a lane-varying body, and kernel fusion. So it is a
real future feature and **not** a prerequisite for the GPU backend described in
[compute-jit-low-types](compute-jit-low-types.md). It is the first item on the C
axis of [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md#42-axis-c-lane-width-and-the-consumer-perspective-has-been-waiting-for),
which also records a mismatch this lattice has and that axis would expose:
**divisibility admits a width no device has** — `6` is a legal perspective, and
no lane group is six wide.

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

An `expr # p ? d` over a value that **already carries** an attribute **replaces the
slots it spells and preserves the rest**. A spelled value **replaces** the slot; the
value's existing attribute is the **provider** it is validated against. The annotation
is the **requirement** (a subtype); the provider is a **supertype**. So the check is
`requirement ⊑ provider` (`requirement | provider`), and the slot becomes the
annotation:

| program | result |
|---|---|
| `(5 # 8) # 4` | `4 \| 8` ✓, slot becomes `4` |
| `(5 # 4) # 8` | `8 ∤ 4` ✗ → "expected 8, found 4" |
| `x = 1 # 8; x # 4` | `4 \| 8` ✓, slot becomes `4` |
| `x = 1 # 8; x # 16` | `16 ∤ 8` ✗ |
| `(5 # 8 ? doc) # 4` | perspective becomes `4`, doc **preserved** |
| `(5 # 8 ? docA) ? docB` | doc replaced by `docB`, perspective `8` preserved |

The provider is the value's own attribute slot (a value that is itself annotated, or a
bound name carrying an attribute) or, for a compound, the `gcd`-meet of its
sub-expressions' slots (which is only ever validated, never kept as the slot). A plain
leaf with no attribute of its own has no provider, so the annotation *is* the slot
(`1 # 4` → `4`).

### The gate must compute its operands

`check_unify_relaxed` computes **both** operands before it attempts the unify
(`Checker::check_unify_relaxed`, `crates/lichen-highlevel/src/checker/diagnostics.rs`).
That is not an optimization; it is what makes the gate a gate.

A compound provider is a `Gcd`-meet **node**, not a value (`AttrExt::combine`), and
nothing has evaluated the expression by the time its annotation is checked — the
checker's statement pass runs *after* the whole build (`Checker::build`). An
uncomputed provider meeting a decided requirement is *undecided against a value*,
which is the one unification arm that **writes** instead of comparing: the
requirement lands in the provider's slot and the check reports nothing. Every
compound annotation was therefore accepted, whatever its numbers were, and the
larger the annotation the less it constrained:

| program | before | after |
|---|---|---|
| `((1 # 4) + (2 # 6)) # 5` | checks — `2 ≢ 5` is never compared | refused, "expected 5, found 2" |
| `((1 # 2) + (2 # 2)) # 4` | checks | refused, "expected 4, found 2" |
| `f = x # 4 => x; f (5 # [1,2][3])` | refused, but spelled "expected none, found none" | refused, "expected 4, found none" |

The third row is the same cause read from the render side: with the provider
unreadable, both sides of the mismatch read as undecided, so the two values the
diagnostic prints were both `none` — including the declared `4`, which has
nothing to do with the failed read. Computing the operands first is what lets the
declared side print `4` and the failed read print its own `none`.

A **leaf** annotation never showed this: its slot is a literal, decided when the
checker builds it. That is why the defect only ever appeared on a compound, and
why `f = x # 4 => x; f (5 # 2)` was refused correctly all along.

**The same step is one shared rule, and the type gate takes it too.** It is
`Checker::compute_operands` (`checker/diagnostics.rs`), and the checker's two
gates both call it: this attribute gate, and the **type** annotation gate
(`check_ann`). The type gate's operand is the annotated expression's *own type*,
which for an applied struct constructor is that application's result — a node
nothing had run — so annotating an instance of `S1` as `S2` was **accepted and
rewritten** rather than compared: the annotation's type was written into the
instance's type cell, and

```lichen
A  = I => struct<.n Int, .I I>
S1 = A Int
S2 = A Float
x  = S1(.n 3, .I 5)
y  = (x : S2)
```

printed `(3, 5): struct<.n Int, .I Float>` — the annotation's own type — instead
of failing. With the step in place the same program is refused:
`expected struct<.n Int, .I Float>#0, found struct<.n Int, .I Int>#0` — the
refusal `pipeline::an_applied_struct_constructor_keeps_the_occurrence_identity`
pins.

The **single-node** run is the right strength for both gates: the operand is one
node whose operator reads what it needs, and the deep pass would additionally
descend its whole reachable subtree — for a type operand, the entire type value —
and publish a concreteness verdict over it. The three perspective rows above and
every suite are unchanged by using the single-node run.

The three tests that pin these rows are
`perspective.rs::a_compound_annotation_rejects_a_mismatched_perspective`,
`..._rejects_a_narrower_declared_perspective` and
`..._a_failed_read_in_an_attribute_renders_as_none`.

What this does **not** decide: an operand that is still undecided *after* being
computed — a runtime-dependent perspective, or a position behind a shallow mark —
leaves the check on the unify arm, where a free cell is a wildcard. Whether a
requirement may bind a runtime-dependent provider is a separate question, and it
is the one this design leaves standing.

## Syntax

`expr [: expr] [# expr] [? expr]` — `:` fills the type slot, `#` fills the
perspective (constraint) slot, `?` fills the doc (label) slot. A
`x # n => e` parameter (and `x : T # n => e`) is accepted; the frontend desugars a
parameter annotation to a leading body statement (`x => { x # n; e }`) and keeps it as
an optimization on `ExprKind::Function.parameter_attribute`. See the
[language spec](../language-spec.md) for the grammar and `#` precedence.

### Labels (`?`)

`?` is the **label** attribute slot — metadata that attaches to an expression but
carries no constraint. Unlike `#` (a constraint a compound lives with and that the
apply-time check enforces), a label contributes no apply-time constraint slot, and the
attribute's own `is_subtype` (a doc returns `true`) is what permits `? b` to override
an existing `? a` without conflict — the checker never special-cases a label's
*unification*, only its metadata slot. A label's value is any first-class lichen value
(by convention a struct instance); the renderer reads the value's field *names* from
its **type chain** (the label's runtime slot carries the annotation value's
`[value, type]` term pair), so nothing about a label's shape is hardcoded.

`?` takes a **general expression**, exactly as `#` does: the value is just the
expression's value, and there is no label-specific literal. A **`Doc`** is therefore a
plain, generic struct-typed value the *user* defines and constructs:

```lichen
Doc = struct<.name string, .description string>
5 ? Doc(.name "five", .description "an int")
```

which renders `5 ? name = "five", description = "an int": Int`. The `Doc` marker stays
(the attribute exists and is ordered), but the slot is fully generic — the checker
understands only "it's a label". Because `Doc` is a real struct:

- **field forcing is automatic**: `Doc{ name = … }` without `description` is an
  ordinary struct-instantiation arity error, with no extra doc check;
- **the renderer reads the type chain** through `render_struct_fields_named`, a
  program-generic renderer, rather than through a positionally-rendered record; a
  string label instead *names* the value it attaches to (`Doc::label`, used by the
  operator refinement's `@in` spelling — see
  [operator-polymorphism](operator-polymorphism.md) §8.1).

The attribute itself — its marker and its extension — lives in `crates/lichen-doc`;
the crate was split out of `lichen-language` along with the rest of the frontend (see
[frontend-syntax-separation](frontend-syntax-separation.md)).


## Non-goals (currently)

- The apply **checks** an attribute (equality + subtype) but does not yet *flow* it out
  through a function (auto-derivation); the return value reads the body's slot. That
  was blamed on the representation — an arrow carried `dom` and `cod` and nowhere to
  hang an attribute — and
  [a function's type is the function](function-type-merge.md) removed the excuse: a
  function type *is* a function, so a signature's parameter and return are terms and
  carry attribute cells, and `?a: Int => ?a: Int` states one cell on both the
  argument and the result. What landed is the ability to *state* the constraint, not
  the flow of it, so auto-derivation remains a non-goal here.
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
  pair, and a constraint's does too — the one uniform slot shape. A constraint reads its
  lattice value from that pair's element 0 (the checker stores the pair in `attr[e]`,
  the apply-time unify and subtype read the value); a label reads the whole pair (its
  renderer walks the value's type chain).
- **A slot is an attribute's position in the composition's `attrs` manifest, not a
  number a plugin declares.** The slot used to be declared three times over (the
  `AttrExt::slot` impls, the checker's sort, the frontend's tail push order) and
  agreed only by hand; a plugin attribute was a silent mis-unification away. The
  manifest order is now the single authority (`AttrSet::ORDER` / `order_index`, the
  shape module's `attr_slot` for the arithmetic), which makes the order — not a
  collision — the thing to protect, exactly like the codec tags.
- The **residual** hand step when the language grows a third attribute: an
  attribute is a *syntax*, and the frontend's `Expr::Annotation` has one fixed
  field per attribute kind, so the grammar/AST gains a field and `compile.rs`
  gains one `(marker, expression)` push. That push needs no slot number and no
  order knowledge — it is sorted into the canonical order — so the **order**
  itself stays a one-list edit (the manifest).

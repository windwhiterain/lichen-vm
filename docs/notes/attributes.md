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

**A pair is also a type value.** When an expression *is* a type — a type parameter, a
struct constructor's argument — its `[value, type]` pair is the type the `[shape, kind]`
encoding reads, so widening every pair to the canonical attribute width would make a
type `[shape, kind, attrs…]` and break type-level functions. That is why an expression
pair stays dense while a compound's *element slot* is full width (a slot is never a
type value: `docs/notes/compound-element-slots.md`). The canonical positions still
travel with the pair: a function's **parameter pair** states its requirement at the
attribute's canonical position (the pair rides the artifact), and the **return
certificate** is the applied clone's own return pair: the call's attribute is the
clone's stated attribute, exactly as the call's type is the clone's codomain. So a
frozen call reads the requirement from the signature pair exactly as a source call does.

**A return attribute computed from the argument follows it.** The vocabulary writes
`perspective_of` the way `type_of` is written — `x => {p = _; x # p; p}` — and a return
spells `# (perspective_of x)` or `# (perspective_of x.to)`. The checker recognizes the
**shape**, never the callee's name: a call whose argument is the enclosing parameter (or
one of its fields) is a **reference** (`Checker::attribute_reference`). The applied
clone's return pair states the call's attribute: a reference to the whole parameter is
decided against the argument's provided slot, and a reference the clone can decide (a
field of a **dynamic** callee's bound parameter) is decided per call site. A frozen
callee's reference is only materialized by the runtime apply, so a *static* field
reference falls back to the argument's whole provided slot. The reference slot is a
fresh cell, and the annotation's computed value is not validated against the body's
provider — the body cannot know the caller's value, so the check belongs to the call
site (`docs/notes/array-attribute-view.md`).

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

**And the certificate's sources are stated, and a call site reads them.** The compute
sub-language's wrapper ([compute.lichen](../crates/lichen-compute/src/compute.lichen))
declares `# 1` on the lane-varying leaves — `range`, `row`, `col` and `read` — which is
the lattice's bottom: the value is uniform over exactly its own thread. A frozen
callee's parameter requirement is the **parameter pair's** stated attribute at its
canonical position (the pair rides the artifact); the return certificate is the applied
clone's return pair, materialized per call (`Checker::check_app`, `Module::applied_pairs`).
So a false claim is refused:
`(compute.range k.n # 64)` answers `expected 64, found 1`.

**Why the attribute slot is not yet a type slot.** The type slot holds the type's
*term*: the clone and the artifact materialize it, so a call's type is always read from
the applied clone. The attribute slot holds only a *cell* — the annotation's value is
unified into it, and the attribute's expression is not an item of the pair — so nothing
in the pair can be re-evaluated at a call site, and a reference such as
`perspective_of x.to` cannot name its field from the pair. That is the root of the
non-isomorphism the superior measured: a frozen callee's field reference falls back to
the argument's whole provided slot. Restoring the field-level reading isomorphically
means putting the attribute's term in the slot as the type's term is: the clone then
carries the expression and the call site evaluates it with the parameter bound.

**An attribute slot is unified like any other slot.** The superior's verdict: if an
attribute is right in the type slot it is right in *any* attribute slot, so a pair's
unify descends the attribute positions exactly as it descends the type slot — no skip,
no second dispatch. What differs is the *rule* at that position: a type must be equal,
an attribute is a refinement, so the call's requirement is checked by the attribute's
own `AttrExt::unify_slots` — and its failure is suppressed when the attribute's
`is_subtype` holds (the highlevel filtering a lowlevel unify error). One mechanism for
a type field's `# n`, a parameter's requirement and a return's certificate alike; a
wildcard states nothing.

Two readings changed with it, and both are the same statement — **a type is not a
uniformity requirement**:

| program | result |
|---|---|
| `x = 5 # 4; (x : Int) # 4` | ✓ — the annotation names a type, not a width |
| `g = x => (x # 2); y = (g 3 : Int); y # 4` | ✗ — the call's certificate survives the annotation |
| `f = (x : Int # 4) => x; f (1 # 2)` | ✗ — a *parameter* requirement is a requirement |

A plain type used to demand the absent top of the value it annotated (the type's
missing slot was reconciled as `0` against the value's `# 1`), which refused every
`(read … : Float)` the moment the seeds stated the bottom; `unify_type_attributes`
now reconciles only the markers the **type expression carries**. And a call's term is
its apply node rather than a pair, so its propagated slot lives in the state and is
read by `value_attr_for` when an annotation preserves it.

### A lane-group requirement (the uniform-load shape)

A value the whole lane group must agree on — a uniform load, a tile base, a config field
a warp reads together — is a requirement the lattice already spells: `# 32` accepts a
provider uniform over 32-groups (`32 | 0`, `32 | 64`) and refuses a lane-varying one
(`32 ∤ 1`) or an incomparable width (`32 ∤ 6`). Measured:

| program | result |
|---|---|
| `warp = 32; f = (acc : Int # warp) => acc; f 7` | ✓ — a uniform value satisfies the group |
| `f (x # 64)` | ✓ — uniform over 64-groups implies uniform over 32 |
| `f (0 # 1)` | ✗ — `expected 32, found 1` |
| `f (0 # 6)` | ✗ — `expected 32, found 6` (divisibility, not order) |

**The shipping vocabulary names the width**: [compute.lichen](../crates/lichen-compute/src/compute.lichen)
binds `warp = 32` (the backend's `SUBGROUP_WIDTH`), so a GPU program writes
`f = (x : Int # compute.warp) => x` and gets the same four readings under `Mode::Check`
— `f 7` ✓, `f (0 # 64)` ✓, `f (0 # 1)` → `expected 32, found 1`, `f (0 # 6)` →
`expected 32, found 6`. Nothing in the runtime reads `warp` yet; it is the width an
annotation may name.

**This is the register half of what the paper's `requires(thread[32])` protects.** The
reference document types `wmma` with a *scope* requirement — the 32 lanes participate in
one operation — while a fragment's data is lane-varying inside the group. That
participation is the *dispatch's* on this backend (64-lane workgroups, a 32-lane
subgroup), so the scope half is a target invariant rather than a program annotation; what
the value lattice states and checks is the register half — the operand the whole group
must agree on (a uniform load, a tile base) is `# 32`, and a fragment's lane layout is
the value's own perspective. The author's ruling and the recorded shape are in
[perspective-scope-and-mma](perspective-scope-and-mma.md) §3, §6.

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

**A third consumer has since been named, and it needs the lattice's precision rather
than its coarse half: how much staging a placement needs.** A value's uniformity is what
decides whether it needs *staging* at all — a value uniform over the group is one every
lane computes, so nothing is exchanged — and a value uniform over `n < group` needs
staging only where a reader sits outside the writer's own `n`-subgroup. Where a value
*is* placed is not this attribute's: the buffer **type** carries the `DP` (`block[1]` /
`grid[1]`), and §4's def-cone query is what refutes a wrong one. So the certificate is a
**precision upgrade on a rule that already works** and not the permission for a
placement, and it is stated as a seam of the memory plan in
[graph-fusion](graph-fusion.md#the-memory-plan-the-second-thing-a-pack-hands-over) §5,
feeding the walk in
[shared-memory-and-barriers](shared-memory-and-barriers.md) §5. The lattice is
load-bearing there for exactly the reason the mismatch above gives: a placement for a
width no device has must refuse rather than round.

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
| `id (5 # 4)` | ✓ — a parameter without `# p` states nothing, as a parameter without a type does |
| `f = x # 4 => x; f 5` | `4 \| 0` ✓ (a uniform value satisfies a `#4` requirement) |
| `f = x # 2 => x; f (5 # 4)` | `2 \| 4` ✓ (real subtype relaxation) |
| `f = x # 4 => x; f (5 # 2)` | `4 ∤ 2` ✗ |
| `f = (x : Int # _) => x; f (5 # 8); f (5 # 2)` | ✓ — a wildcard states nothing, so no call pins the template |

**An absent requirement states none, and `#_` spells it.** A parameter without a
perspective is unconstrained — the same reading a parameter without a type has — so
`id (5 # 4)` is accepted; `# p` is how a parameter asks for a width, and the explicit
wildcard `#_` states the same absence in writing. The apply reads the declared slot as
a value and skips the comparison when it is still undecided; unifying an undecided
declaration instead would bind the template's own cell to the first call's argument —
the second call would then be checked against the first (`f (5 # 8)` then `f (5 # 2)`
used to answer `expected 8, found 2`).

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

The provider is the value's own attribute slot (a value that is itself annotated, a
bound name carrying an attribute, or a call whose callee's return states one) or, for a
compound, the `gcd`-meet of its sub-expressions' slots (which is only ever validated,
never kept as the slot). A plain leaf with no attribute of its own has no provider, so
the annotation *is* the slot (`1 # 4` → `4`).

### A declared return attribute rides to the call site

A function whose returned value is annotated states that attribute as a fact about
**every** value it returns, so a call carries it: the checker records the return
expression's constraint slot, and each call site copies it into a slot of its own
(`Checker::function_return_attr`; the copy is a concrete constant, so the callee's
slot is read, never bound). An **undecided** return slot is not recorded: the call
then carries no attribute and its value reads as the absent form.

| program | result |
|---|---|
| `g = x => (x # 2); (g 3) # 4` | `4 ∤ 2` ✗ → "expected 4, found 2" |
| `g = x => (x # 2); (g 3) # 2` | `2 \| 2` ✓ |
| `g = x => (x # 2); (g 3) # 1` | `1 \| 2` ✓ |
| `h = y => (g y); (h 3) # 4` | ✗ — the certificate is copied at each hop |
| `g = x => x; (g 3) # 4` | ✓ — no return attribute is stated, so the call states none |

**A parameter's requirement is not a return attribute.** `f = x # 4 => x` checks
every argument against `4` at the call, but its result states nothing: a perspective
on a parameter is a requirement on the caller's value, not a promise about what the
body returns. The body's reference to `x` is the parameter's own slot, and copying it
to the call would restate the caller's value as the callee's answer.

**What it does not do.** The certificate reaches the call site, not through a
compound the caller builds: `h = y => (g y) + 1` states nothing, because its returned
expression is an unannotated `+` and the combine is only ever validated at an
annotation site (above). Deriving a provider for an arbitrary expression is the
inference this note does **not** have, and the certificate's first consumer is still
unbuilt ([gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) §4.2).

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
A  = I => struct{.n Int, .I I}
S1 = A Int
S2 = A Float
x  = S1(.n 3, .I 5)
y  = (x : S2)
```

printed `(3, 5): struct{.n Int, .I Float}` — the annotation's own type — instead
of failing. With the step in place the same program is refused:
`expected struct{.n Int, .I Float}#0, found struct{.n Int, .I Int}#0` — the
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

Adding an attribute to the tail widens the pair, and that is the right trade for
metadata a *value* carries. Metadata that must stay **pinned to one expression** is not
an attribute: a slot is padded, propagated and meet-combined, and the pair that holds
it is unreachable from the value it annotates, so graph fusion's interface claim rides a
**mark** (`~`) in a side table beside the expression tree instead — see
[graph-fusion](graph-fusion.md) §3.

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
Doc = struct{.name string, .description string}
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

## Recovered measurements

- The shared missing-slot node is built on **first use**, not in the install pass:
  the site is cold — a program that never has an absent perspective never reaches
  it — so an eager install would spend two nodes on *every* build to save them only
  on the few that ask.  The cost of laziness is that a slot first needed *inside* a
  lambda is tagged into that function's template and so is cloned per apply, which
  is exactly what the per-occurrence form did; it is never worse, only sometimes no
  better.
- The missing-slot cache is keyed by the marker's **position in the set's own order
  list**, not by `AttrSet::order_index`: the list is what sizes
  `Checker::missing_slots`, so the position is in range by construction, while an
  index a plugin returns is only checked in debug builds by `order_is_canonical` —
  a hand-written set that disagrees with its own list would index out of bounds, or
  alias another attribute's cached slot, in a release build.  For a set whose order
  *is* canonical the two are the same number.  `order_is_canonical` itself has two
  enforcement points: a generated set asserts well-formedness at build time as a
  `const`-evaluated check in the composition macro, and a hand-written set is
  checked in debug builds when a checker is built over it.
- `AttrExt::share_missing_slot`'s contract: an attribute decides, about its own
  value, whether every occurrence of an absent attribute may read **one** shared
  missing-slot node installed once per build instead of a fresh node per site, and
  the checker only asks and caches the answer.  What a shareable missing value must
  satisfy is that it is **concrete**: reconciling two slots is a real unification,
  and a unify *writes* whichever side is undecided — a concrete node is only ever
  read, so one node can serve every occurrence, while an undecided one would be
  written by whichever occurrence unified first and every later occurrence would
  read the bound value.  That is the whole difference between the two attributes
  highlevel ships against: a perspective's absent form is the constant `0` (the
  lattice's meet identity), so it shares; a doc's absent form is an **undecided
  cell**, which a unify binds on purpose so a doc passes from one side to the
  other, so it must be fresh per site — sharing it would let the first bind poison
  every later read.  The default is `false`, so an attribute opts in by stating
  that its missing value is concrete.
- `AttrExt::unify_slots` receives the *found/value* side as `a` and the
  *expected/declared* side as `b`; a perspective's default impl is an equality
  unify, while an attribute that defines `is_subtype` may call
  `Ctx::check_unify_relaxed` instead so a subtype — not just an exact match —
  passes.  `is_subtype` is the optional subtype relation on the attribute's slot
  values, consulted **after** a failed equality unify: if it holds, the failure's
  error is suppressed and the check passes.  Its default is `false`, so exact
  equality is required; implementations read the two slot values with
  `Ctx::node_value`, and an undecided value (a runtime-dependent perspective)
  should return `false` so the check stays conservative.
- `combine` merges the direct sub-expressions' attribute slots into one node — a
  perspective runs the language's meet operator over the operand array, a lazy
  operand gets a fresh empty cell — over `children` that are already compiled and
  pre-padded with `missing_value`, built through the curated `Ctx` and never raw
  lowlevel nodes.
- `is_label` selects only the *semantic* difference between the two kinds of
  attribute, because the checker's slot handling is uniform: a label contributes no
  apply-time constraint slot and is never validated against the provider, while a
  constraint is, and whether a label *conflicts* is decided solely by `is_subtype`
  (a metadata attribute overrides it to `true`, so the checker never has to
  special-case a label's unification, while a constraint attribute returns
  `false`).  The checker consults it in `Checker::check_ann` to choose the
  metadata-slot path (label) over the provider/unify path (constraint).
- `render` is what the output printers use: they iterate the expression's schema
  tail and render every *present* attribute, so an un-annotated expression spells
  nothing; `AttrExtRegistry`'s `P` is the host program and `Attr` its marker type —
  `P::Attr` for a composed program (the checker's `build_in_attr` and siblings), or
  a concrete marker when a language layer returns its own registry (e.g.
  `lichen-perspective`'s `persp_attr_ext`).
- `unify_slots` implementations that compare the attribute's *value* map through
  `slot_value_node` first: a perspective compares lattice numbers and a refinement
  compares predicates.  A slot whose value is not a pair (`None`, or a bare value)
  is returned as it stands, so a caller needs no case analysis.
- `Checker::build` is the path a host reaches when it composes an attribute set but
  does not pass the matching extension.  The schema's attribute cannot be lowered
  there, so it is a reported diagnostic plus a well-formed hole — never a panic
  inside the checker.  The build is missing **one** thing, so it reports **one**
  diagnostic however many sites read an attribute: the apply's own slot check is
  skipped rather than reported again.  A refused annotation still compiles to the
  schema's full width, and its attribute slot is a fresh undecided pair of the same
  hole shape the other check-time guards leave.
- `:` and `#` are the **loosest** operators — they wrap the whole expression to
  their right — so a compound whose operands are individually perspective-annotated
  is written with explicit parens, `((1 # 4) + (2 # 6)) # 2`, to express the
  grouped operands with `# 2` over the `+`; the bare `1 # 4 + 2 # 6` spelling is
  the same semantics under that reading.
- `lichen-doc` is a **compiler plugin**, not a native plugin (see
  [plugin-taxonomy](plugin-taxonomy.md)), and the host language layer must
  **codesign** with it: the grammar production and AST node for `? expr`
  (`AnnPiece::Doc`), the `Schema` tail form the checker reads, and the lowering of
  a `?` annotation into the label slot (`check_ann`) live in the host layer, tied
  to one language's surface and on-disk contract.  What is shared, and is in the
  crate, is the **meaning**: the `Doc` marker and its `AttrExt` lowering, with a
  host composing the marker into its own vocabulary and calling
  `doc_attr_ext::<P>()`.
- `Doc`'s `combine` returns *no doc*: a compound's doc is its own annotation, never
  a meet of its children's docs.  Its `unify_slots` attempts a real unify — an
  undecided doc cell binds to the concrete doc, which is how a doc *passes from one
  side to another* — and when two already-concrete docs differ, `is_subtype` is
  `true` so the mismatch is suppressed and the existing doc is kept (the override
  case).  `is_subtype` is the attribute's only lever for "never conflicts"; the
  checker never special-cases a label's unification.
- The struct names table, whose source entry recorded no home: letting the vacuous
  `all` decide put the `Error` marker in a checked struct type's names slot, which
  made an empty struct read as an *anonymous* one
  (`DiagKind::StructAnonymousField` reachable from source) and falsified a claim
  then in `code-audit.md` that only hand-built IR reaches it (the claim no longer
  exists, so the citation was dropped rather than repointed).  The surviving rule
  is `build_struct_names`'s `# Invariant`: a non-empty list of wholly unnamed
  fields builds the `LowLevel::Error` marker, while an empty list builds a present,
  empty table.

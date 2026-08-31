# Typed Perspective Attribute — Design & Implementation Plan

**Status:** proposal (not implemented).
**Upstream:** none — self-contained on top of the existing highlevel checker.
**Inspiration:** *Modular GPU Programming with Typed Perspectives* (Bansal,
Sainati, Cutler, Amarasinghe, Ragan-Kelley), arXiv:2511.11939 — the "typed
perspectives" idea, re-derived here as a *minimal, general* mechanism rather
than a GPU-specific one.

---

## 1. Motivation & goals

The paper materializes, at the type level, the granularity at which the
programmer controls threads (`thread[1]`, `thread[32]`, `block[1]`, …). Its
power is a **second attribute on code/data that participates in the type
system with its own partial order and local rules**, orthogonal to the
ordinary type.

lichen has no GPU and does not need lifetimes. The value of the idea here is
the *mechanism*: a language where every expression already carries a
first-class `[value, type]` pair should be able to carry **extra, optional
attributes** whose *lattice* (order, meet, top) is not baked into the core but
supplied by a pluggable **extension**. Perspective is the first instance; the
point is to prove the extension mechanism is real, so a later GPU DSL frontend
(or lifetimes, effects, staging levels) can plug in the same way.

Stage-1 goals, in priority order:

1. **Prove compatibility.** A program with no `#` keeps its value/type
   semantics exactly as today, with the lowlevel unify/clone/apply machinery
   untouched.
2. **Carry an optional attribute through the pipeline**, derived by a lattice
   **meet**, not by equality unification, and **checked at function application**.
3. **Make the semantics an extension, not a builtin.** The highlevel core knows
   only the *shape* of "an attribute combines over children" — never
   "perspective = gcd, missing = 0".

Non-goal for stage 1: enforcement beyond the apply's equality check. The meet
(`gcd`) is total; stage 1 tracks and *checks* (at apply and `# p`) but does not
yet do the `⊑` subtype check. That is stage 2.

---

## 2. Core idea: the schema is a *compile-time* type

This is the load-bearing decision, and it distinguishes lichen from every other
layer decision so far:

> **In lichen, the ordinary "type" is a *runtime* value** — `[value, type]` is
> a pair of graph nodes, types are first-class, unification runs in the VM, and
> "the runtime *is* the typechecker."
>
> **The schema is a *compile-time* (static) type.** It describes the *shape* of
> an expression's runtime pair — its arity and which attribute sits in which
> slot — and is known and fixed at lowering. It is **never a runtime node, never
> unified, never cloned**: the highlevel consumes it to decide how to *build*
> the runtime pair, then it is gone.

`e # p` therefore does not add a runtime "perspective type" that flows around
like a value; it changes the **static schema** of `e` to
`[value, type, perspective]`, and the highlevel lowering honors that schema by
building a **3-wide runtime pair** with the perspective in slot 2.

**Nothing has a fixed arity.** Whether a pair is 2-wide or 3-wide is decided
entirely by the schema at lowering time, expression by expression. The highlevel
lowering reads the schema and builds the pair at exactly that arity; at every
unify site it pads an absent attribute with the extension's `missing()` value
so the two sides are the same length, slot-aligned, and the positional lowlevel
unify just works.

---

## 3. The perspective lattice

A perspective is a plain non-negative integer. The order is **divisibility**,
not numeric size:

```
a ⊑ b   (a is a subtype of b)   ⟺   a divides b   (a | b)
```

| quantity | value | role |
|---|---|---|
| subtype relation | `a \| b` | `2 ⊑ 4`, `2 ⊑ 6`, `4 ⊑ 12`; `4` and `6` are incomparable |
| **meet** (greatest common subtype) | `gcd` | `gcd(4, 6) = 2` — the combine rule |
| join (least common supertype) | `lcm` | `lcm(4, 6) = 12` — unused in stage 1 |
| **top** ("uniform" / "most supreme") | `0` | every `n` divides `0`; `gcd(n, 0) = n` makes it the meet identity |
| **missing** | `0` | the value read for an absent perspective |
| bottom ("finest") | `1` | `1` divides everything |

Two traps (both caused backtracking during design):

- **The order is divisibility, not `≤`.** `2` is numerically smaller than `4`
  yet is a *subtype* of it (`2 | 4`).
- **`0` is the top, encoded as `0` (not `∞`).** "Uniform" is the semantic name;
  the bits are `0`, because `gcd(n, 0) = n`.

---

## 4. Syntax

```
expr [: expr] [# expr]
```

- `:` annotates the **type** slot (existing).
- `#` annotates the **perspective** slot (new) — chosen over `@` because the
  preprocessor owns `@import`. `#` is currently unused in the language.
- `# p` unifies the slot with `p` (equality) *and* marks the whole annotated
  expression's schema as carrying the `Perspective` tail — the one asymmetry
  with `:` (the slot comes into existence by being annotated).
- `e : T # p` does both; `#` binds at the same precedence as `:`.

### 4.1 `# p` has three homes

1. **`e # p` on a term** (`5 # 4`, `x # 4`) — that value's **data perspective**.
2. **`(x => e) # p` on a function** — the function *value's* **code perspective**
   = "this function's logic is uniform in `p`". Carried on the function value's
   pair; **not checked in stage 1** (it is for the future code-perspective / GPU
   stage).
3. **`x # n => e` on a parameter** — the *parameter's* perspective, exactly
   analogous to `x : T => e` for the type.

### 4.2 Parameter annotations desugar to a body statement

The spec's "`x : T => e` desugars to `(x => e) : (T -> _)`" is **wrong**. The
correct desugar — for type and perspective alike — is:

```
x : T => e      ⟶   x => { x : T; e }
x # n => e      ⟶   x => { x # n; e }
x : T # n => e  ⟶   x => { x : T # n; e }
```

The annotation becomes a *leading bare annotation statement* in the body block,
which unifies the parameter's slot directly in body scope (so `T`/`n` may
reference `x`, e.g. `x : x -> Int`). The implementation keeps this as an
**optimization**: `ExprKind::Function` carries `parameter_type` (existing) and
`parameter_perspective` (new), both compiled in body scope — semantically the
body-statement desugar, but without materializing the block.

---

## 5. Semantics

### 5.1 The rules

1. **No auto-propagation (terms).** `schemas[e]` gains the `[Perspective]` tail
   only from `e # p`. An unannotated term's schema stays `[value, type]`.
2. **Read-missing = `0`.** Reading an absent perspective yields `missing()` =
   `0` — neutral in `gcd`, concrete in equality unify.
3. **Compound combine.** A `# p`-annotated compound derives its slot as `gcd`
   over its direct sub-expressions' perspectives (absent → `0`), then the `# p`
   equality-unifies that slot with `p` — i.e. `# p` on a compound *checks* the
   derived perspective. A `# p`-annotated leaf's slot is simply `p`.
4. **Return value's perspective** = the return expression's (data) perspective,
   independent of the function's own `# p` and of the argument.
5. **Apply checks value and type.** The apply unifies the argument against the
   parameter (value binding) and against the function's **arrow** (type +
   attributes, see §6.3). A missing perspective on either side reads `0`.

### 5.2 Worked examples

| program | result |
|---|---|
| `1 # 4` | perspective `4` |
| `1 # 4 + 2 # 6` | `+` unannotated → no slot, no gcd; `3 : Int` |
| `(1 # 4 + 2 # 6) # 2` | slot = `gcd(4,6)` = `2` → check `2 ≡ 2` ✓ |
| `(1 # 4 + 2) # 4` | `gcd(4,0)` = `4` → check ✓ (unannotated `2` contributes `0`) |
| `(1 # 4 + 2 # 6) # 5` | slot = `2` → check `2 ≡ 5` ✗ |
| `id = x => x; id 5` | param reads `0`, arg `0` → `0 ≡ 0` → `5` |
| `id = x => x; id (5 # 4)` | `0 ≡ 4` → **error** |
| `f = x # 4 => x; f (5 # 4)` | `4 ≡ 4` → `5 # 4` (body `x` reads `4`) |
| `f = x # 4 => x; f 5` | `4 ≡ 0` → **error** |
| `g = x => (x # 4); g 5` | apply `0 ≡ 0` ok → returns `5 # 4` |

The combine rule per construct (only when the construct itself is `# p`
-annotated; absent children read `0`):

| construct | combine over |
|---|---|
| `BinOp`, `Apply`, `Instantiate`, `Index`, `Field`, `Find`, `TypeArray`, `TypeFunction` | the named sub-expressions |
| `Tuple`, `TypeTuple`, `Array`, `TypeStruct`, `ShallowArray`, `Table` | the `children` range |
| `Annotation` | transparent — the perspective of its `value` |
| `Constant`, `Parameter`, `Placeholder`, `Static` | leaf — the annotation binds the slot |
| `Function` (lambda) | see risk #2 |

---

## 6. Design

### 6.1 Schema types (`crates/lichen-highlevel/src/ir.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attr { Perspective }

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Schema { pub tail: Vec<Attr> }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SchemaId(pub u32);
```

`IR` gains two fields (the parallel-arena style of `children`/`depths`):

```rust
pub struct IR<V> {
    pub expr: Vec<Expr<V>>,
    pub schemas: Vec<SchemaId>,      // one u32 per ExprId
    pub schema_table: Vec<Schema>,   // interned
    // ... existing children / depths / root / block_roots ...
}
```

`IR::alloc` pushes the default empty schema; `IR::intern_schema` dedups;
`IR::set_schema(ExprId, SchemaId)` stamps an allocated node. The default `[]`
schema is what every existing program gets — the ordinary path is unchanged.

`ExprKind::Annotation` generalizes:

```rust
Annotation {
    value: ExprId,
    r#type: Option<ExprId>,        // `: T`
    perspective: Option<ExprId>,   // `# p`
}
```

### 6.2 The extension point (`crates/lichen-highlevel/src/attr.rs`, new)

The schema names *which* attribute; an attribute's **compile-time lowering
behavior** lives in an extension. The checker calls these attribute-agnostically:

```rust
pub trait AttrExt<V: ValueType> {
    fn missing(&self) -> LowValue;                                        // perspective → 0
    fn combine(&mut self, module, block, children: &[NodeId]) -> NodeId;  // perspective → Gcd
    fn unify(&mut self, module, a: NodeId, b: NodeId);                    // perspective → module.unify
}

pub struct Perspective;  // the first instance; "gcd"/"0" appear only here and in `Gcd`
```

`schemas[e].tail` is a list of `Attr`; each is dispatched to its `AttrExt`. For
stage 1 the only `Attr` is `Perspective`.

### 6.3 The arrow (function type) encodes attributes

The function type `T -> U` becomes schema-shaped. Its **shape** is a 2-wide
array of the parameter's and return's **type spines**:

```
arrow shape = [ param_spine, return_spine ]
param_spine = [ param_type, param_persp, ... ]   // the parameter's slots 1..n
return_spine = [ return_type, return_persp, ... ] // the return's slots 1..n
arrow pair  = [ shape, [FunctionType, Type] ]
```

The spine is the expression's pair *without the value slot* — `[type]` when the
side has no perspective, `[type, persp]` when it does. The arrow's slots **unify
with the nodes in the function body**: `param_spine` with the parameter node's
type/attributes, `return_spine` with the return expression's type/attributes
(so they share nodes — the arrow is a view of the body, not a copy).

The apply therefore checks **both**:

1. **value** — the argument's value binds to the parameter's value (the lowlevel
   apply clone/unify);
2. **type (arrow)** — the argument's type + attributes unify against the arrow's
   `param_spine` (which *is* the parameter's type + attributes, by node sharing).

A missing attribute on either side reads `missing()` = `0`, so:

- `id = x => x` (param spine `[type]`, no persp) applied to `5 # 4` → the
  perspective check is `0 ≡ 4` → **error**.
- `f = x # 4 => x` (param spine `[type, 4]`) applied to `5` → `4 ≡ 0` → **error**.

### 6.4 The `Gcd` operator (`crates/lichen-highlevel/src/program.rs`)

`gcd` is a **program operator** (the existing `enum_ext!` / `OperatorExt::run`
mechanism), so the lowlevel core never names it:

```rust
pub enum TypeOperator {
    Fresh, Add, Sub, Leq, Eq,
    /// Perspective combine: n-ary gcd over its operand array. Empty → 0
    /// (the meet identity / top). Reads each element as `USize`; a lazy
    /// operand yields `Parameterized`, like Add/Sub.
    Gcd,
}
```

---

## 7. Phased implementation

### Phase 1 — `Gcd` operator

File: `crates/lichen-highlevel/src/program.rs`. Add `TypeOperator::Gcd` and its
`OperatorExt::run` arm (fold `gcd` over the operand array, empty → `0`, lazy →
`Parameterized`). Tests over `[4,6]`, `[4,0]`, `[]`, lazy.

### Phase 2 — schema types + IR arena + `Annotation` generalization

File: `crates/lichen-highlevel/src/ir.rs`.

1. `Attr`, `Schema`, `SchemaId`; `IR::schemas` + `IR::schema_table`; `IR::alloc`
   stamps the default; `intern_schema` / `set_schema`.
2. `ExprKind::Annotation { value, r#type: Option, perspective: Option }`;
   add `ExprKind::Function::parameter_perspective: Option<ExprId>` alongside the
   existing `parameter_type` (the optimization for the §4.2 desugar).

### Phase 3 — `AttrExt` + `Perspective`

File: `crates/lichen-highlevel/src/attr.rs` (new); export from `lib.rs`. Unit-test
`Perspective::combine` emits a `Gcd` node evaluating to the right meet.

### Phase 4 — checker (schema-driven lowering)

File: `crates/lichen-highlevel/src/checker.rs`.

1. Each `check_*` arm builds the pair at `schemas[e]`'s arity:
   - `[]` → `[value, type]` (unchanged);
   - `[Perspective]` → `[value, type, persp]`, where `persp` is a fresh cell for
     a leaf, or `combine(children)` for a compound.
2. `check_ann` with `perspective: Some(p)` → `check_expr(p)`;
   `ext.unify(persp_slot(value), value_of(p))`.
3. `check_lam`:
   - build the parameter at the parameter's schema arity (`[value_cell, type_cell]`
     or `[value_cell, type_cell, persp_cell]` — `persp_cell` fresh, bound by the
     body's leading `x # n` statement);
   - compile the body (block with leading `x : T` / `x # n` annotations);
   - build the arrow `[[param_spine], [return_spine]]` from the parameter's and
     return's type spines (schema-driven arity), unifying each spine against the
     body nodes.
4. `check_app`:
   - lowlevel apply for the value/type binding (parameter pair vs argument pair,
     both at the parameter's arity — the highlevel pads the argument's missing
     slot with `missing()`);
   - the perspective check via the arrow: `check_unify(persp_or_zero(param),
     persp_or_zero(arg))` where `persp_or_zero` reads the spine's perspective or
     `0`. (Mechanically this *is* the "type (arrow)" check the user described;
     the exact site — a highlevel `check_unify` against the arrow's `param_spine`
     vs folding it into the lowlevel apply — is the one detail to settle in
     Phase 4, with the §5.2 table as the acceptance test.)

### Phase 5 — frontend (lex / parse / ast / compile)

Files: `crates/lichen-language/src/{lex,ast,parse,compile}.rs`.

1. **lex** — `TokenKind::Hash`.
2. **ast** — `Annotation { value, r#type: Option, perspective: Option, span }`.
3. **parse** — extend the `:` annotation fold with `#`; `x : T => e` /
   `x # n => e` / `x : T # n => e` become `Lambda { parameter_type, parameter_perspective }`
   (the optimization for the §4.2 desugar).
4. **compile** — `ExprKind::Annotation { value, r#type, perspective }`; when
   `perspective` is `Some`, `set_schema(value_id, schema_with(Perspective))`.

### Phase 6 — tests, examples, spec

- The §5.2 table as integration tests.
- An `examples/programs/perspective.lichen` example (readme sync if observable).
- `docs/language-spec.md`: correct the `x : T => e` desugar and add the
  perspective grammar `expr [: expr] [# expr]`.

---

## 8. Key design decisions & rationale

1. **Schema = compile-time type; ordinary type = runtime value.** The schema is
   lichen's first *static* thing — consumed at lowering, never a runtime node.
2. **Nothing has a fixed arity.** Arity and slot positions are decided by the
   schema at lowering, expression by expression. The highlevel pads absent
   attributes with `missing()` at unify sites; the lowlevel unify stays
   positional/equal-length.
3. **The arrow encodes the parameter's and return's type spines** — `[[param_type,
   param_persp, …], [return_type, return_persp, …]]` — and unifies with the body
   nodes. The apply checks value (binding) *and* type (arrow), which is what type
   checking *is*.
4. **Parameter annotations desugar to a body statement** (`x : T => e` →
   `x => { x : T; e }`, `x # n => e` → `x => { x # n; e }`); the implementation
   keeps this as an **optimization** — `parameter_type` (existing) +
   `parameter_perspective` (new) fields compiled in body scope.
5. **`# p` has three homes**: term (data perspective), function value (code
   perspective, carried not checked), parameter (`x # n`, data perspective,
   desugared).
6. **Return value's perspective = the return expression's**, independent of the
   function's `# p` and the argument.
7. **Apply is an equality check** (`missing` → `0`): a function without a
   perspective reads `0` and rejects a `# 4` argument. Perspective does not yet
   *flow* through functions — that needs auto-derivation, a later stage.
8. **`#`, not `@`** (preprocessor owns `@import`).
9. **Divisibility lattice, meet = gcd** — total, so stage 1 tracks/checks without
   enforcement.
10. **Semantics are an extension (`AttrExt`)**: `missing`/`combine`/`unify` =
    `0`/`gcd`/equality live in `Perspective`, never in the core; `Gcd` is a
    program operator.
11. **`Schema::tail` is a `Vec<Attr>`** — multiple attributes fall out naturally,
    but only `Perspective` is built/tested in stage 1.

---

## 9. Test plan

### program.rs
`Gcd [4,6]` → `2`; `[4,0]` → `4`; `[]` → `0`; lazy → `Parameterized`.

### ir.rs
`IR::alloc` stamps the default schema; `set_schema`/`intern_schema` dedup;
`schemas` index-aligned with `expr`; `Annotation` with both fields optional.

### checker
- `1 # 4` → 3-wide pair, `Index(pair, 2)` = `4`.
- `1 # 4 + 2 # 6` → `+` 2-wide (no auto-propagation).
- `(1 # 4 + 2 # 6) # 2` → slot `2` ✓; `# 5` → unify error.
- `(1 # 4 + 2) # 4` → `4` ✓ (missing contributes `0`).
- `id 5` / `id (5 # 4)` / `f (5 # 4)` / `f 5` / `g 5` per §5.2 (the apply
  equality check, `0`-padding).
- **Regression:** every existing program passes unchanged.

### language (frontend)
- `1 # 4`, `e : T # p` parse; `x # n => e` desugars to `x => { x # n; e }`.
- §5.2 programs typecheck/error as specified.
- `#` does not clash with `@import`.

---

## 10. Non-goals (stage 1)

- **Enforcement (stage-1 non-goal, now implemented)** — stage 1 was equality on
  the apply/`# p`; the `⊑` check is now added as the attribute's *optional*
  [`AttrExt::is_subtype`] hook.  An attribute opts in by overriding it
  (`Perspective` does: `a ⊑ b ⟺ a | b`, with `0`/missing as a distinct kind
  matched only by `0`); the checker retries a failed attribute unify through
  [`Checker::check_unify_relaxed`] and suppresses the error when the subtype
  holds.  `lcm` join and perspective *flowing through functions* (auto-derivation)
  are still future work.
- **Perspective flowing through functions** — the apply *checks* (equality), it
  does not *propagate*; that needs auto-derivation, a later stage.
- **The function's code perspective** (`(x => e) # p`) being *checked* — it is
  carried for the future GPU/code-perspective stage.
- **Reorder/drop schemas** — `[type, value]`, `[value]`, `[type]` unsupported
  (value@0, type@1 are load-bearing).
- **A second attribute** — only `Perspective` exists; `Schema::tail` is a `Vec`
  but no second `AttrExt` is built.
- **Observation in the CLI/spec output** — tests observe `Index(pair, 2)`.
- **GPU DSL frontend**.

---

## 11. Risks & open questions

1. **The padding site.** §5.2 fixes the semantics; whether the apply's
   perspective check is a highlevel `check_unify` against the arrow's
   `param_spine` or is folded into the lowlevel apply (padded argument) is the
   one mechanical detail to settle in Phase 4. The acceptance test is the §5.2
   table, which is unambiguous.
2. **`Function` (lambda) combine.** A lambda's own `# p` slot (`gcd(parameter_type?,
   return)`) is the least-thought-through rule; treat a lambda as a leaf (its
   `# p` binds a fresh slot) or combine over its body — decide in Phase 4.
3. **`r#type` → `Option` + `parameter_perspective`.** Mechanical
   (exhaustiveness-checked); `parameter_type` stays as-is.
4. **`Gcd` arity.** N-ary (one node per compound) vs binary (fold). N-ary chosen;
   fall back to binary + fold in `Perspective::combine` if the `run` arm grows
   unwieldy.
5. **Graph sharing.** `a = 1; a # 4` stamps the shared `1` node's schema, so all
   uses of `a` see a 3-wide pair — consistent with "one node, one shape", but
   worth an explicit test.
6. **Stage 2.** Enforcement = relax the apply/`# p` equality to the `⊑` check.
   **Direction (from the paper, §3.6):** a perspective is "uniform over `n`
   aligned threads", so a value is usable where `q` is declared iff `q | n` (an
   aligned `n`-group partitions into `q`-groups) — the *declared* is the
   subtype, `declared ⊑ argument`.  `0` is the lattice **top** ("uniform over
   all threads", the `∞` fold: a value with no `#` is not expressed per-thread
   in GPU code, so it is uniform across every thread; Rust integers have no
   `∞`, so the top folds to `0`, the divisibility identity `n | 0`,
   `gcd(n, 0) = n`).  Hence
   `divides(sub, sup) = sub == 0 ? sup == 0 : sup % sub == 0`: only a
   uniform-over-all value satisfies a uniform-over-all requirement, and a
   uniform-over-all value satisfies any specific requirement (so `id (5 # 4)`
   fails—a `# 4` value is not uniform over all threads—while
   `f = x # 4 => x; f 5` passes).  `lcm` join, letting perspective *flow*
   through functions (auto-derivation), and using the carried function
   code-perspective are still future work.  The `AttrExt::unify_slots` seam and
   the arrow's `param_spine` are where those land.  (Implementation note: the
   parameter's *live* attribute cell must stay unbound — binding it to the
   declared value lets the deep pass bake it, so the per-apply clone
   references the template's cell and enforces the declared value at the
   lowlevel apply, defeating the subtype relaxation.)

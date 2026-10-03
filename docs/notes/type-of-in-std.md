# `type_of` is a standard-library function, not a language form

> Status: current — the keyword, the AST form, the highlevel `ExprKind::TypeOf`
> and its checker special case are gone; the type read lives in
> [lichen-std/_.lichen](../../lichen-std/_.lichen).  The `.sig` mis-render the
> removal first exposed is **fixed**: it was a *printer* misclassification of a
> frozen kind, not a checker or deferral fault —
> ["contains the universe" being read as "is the
> universe"](universe-containment.md) §2.
> Points at: `crates/lichen-language-lex`, `crates/lichen-language-parser`
> (`ast`/`parse`), `crates/lichen-language` (`compile`/`resolve`/`dirty`/`spans`),
> `crates/lichen-highlevel` (`ir`/`checker`), `crates/lichen-compute`
> (`compute.lichen`), `lichen-std/_.lichen`.
> The language-level statement is the spec's *Reading a value's type*
> ([language-spec.md §3](../language-spec.md)); this note records the decision
> and what it removed.

## The expression

```lichen
type_of = x => {t = _; x: t; t}
```

`t = _` binds a fresh cell, the annotation `x : t` unifies that cell with the
argument's type, and the body returns it. Because a type **is** a value in this
system, the cell that holds `x`'s type *is* `x`'s type expression — so the call
works in a type position (`5 : type_of (5)` checks) with no extra machinery.
Every observation the builtin produced is reproduced:

| probe | result |
|---|---|
| `type_of (1)` | `Int: Type` |
| `type_of Int`, `type_of Type` | `Type: Type` |
| `type_of [1, 2]` | `array<Int, 2>: TypeArray` |
| `type_of (1, Int)` | `<Int, Type>: TypeTuple` |
| `type_of (x => x)` | `?a -> ?a: TypeFunction` |
| `type_of (type_of (1))` | `Type: Type` |
| `5 : type_of (5)`, `type_of (1) : Type` | `5: Int`, `Int: Type` |

## Why it moved

The builtin was a keyword, an AST variant, an `ExprKind::TypeOf`, and a checker
arm that read element 1 of the operand's `[value, type]` pair through the raw
lowlevel `Index`. None of that was load-bearing: the read needs no grammar
(juxtaposed application is the whole story) and no checker special case (an
annotation already unifies a cell with a type). A form that the language can
express has no business in the language definition, so it was deleted rather
than kept as a primitive.

What changed for a user: `type_of` is **no longer reserved** — it can be bound,
shadowed, and passed like any name — and a program that wants the read imports
it (`std.type_of`) instead of getting it from the grammar.

## What was removed

- `TokenKind::KwTypeOf` (lexer keyword, display name, keyword table).
- `Expr::TypeOf` (AST variant, its span, the expression-start set, the
  children/content-key/error-block/semantic-token walks, the LSP's four
  exhaustive matches).
- `ExprKind::TypeOf` (highlevel IR variant, its `fix` walk, the `range_children`
  and attribute-leaf arms) and `Checker::check_type_of`.
- The tree-sitter `type_of` rule and its two highlight queries (the generated
  parser files are build outputs — see
  [tree-sitter-generated-files](tree-sitter-generated-files.md)).
- The spec's grammar line, keyword lists, §3 bullet and §4 compile-table row;
  the tests that asserted the old IR shape.

## The duplication this leaves

`crates/lichen-compute/src/compute.lichen` repeats the definition as a private
`let type_of = x => {t = _; x: t; t}`: an embedded native plugin source is
compiled against that plugin's own private native registry and cannot depend on
a package. The `let` keeps it out of the module's exported struct. Every other
consumer either imports the standard library or spells the one-liner next to
the probe that needs it (as the language tests do — `compile` takes a bare
source with no package store).

## The defect the removal exposed: the printer misread a frozen kind as the universe

**Status: fixed** — the renderer's `is_universe` now tests the whole `[Type, ↺]`
shape, so the three rendering assertions run un-parked and the `compute` suite is
green ([universe-containment](universe-containment.md) §2 carries the analysis
and the change; its §3 is a separate, still-open lowlevel hole in the same
predicate family).  The record below is kept because it is what the fix was
accepted against, and because it names the wrapper-level dodges that *cannot*
work — they move *when* the read happens, never *where the arrow's `K` comes
from*.

### The symptom

```lichen
k = compute.jit (y => y + y)
k
```

| build | output |
|---|---|
| with the builtin (before the removal) | `(Kernel, parameterized): struct<.native raw[?a, ?b], .sig Int -> Int>` |
| with the library `type_of` | `(Kernel, parameterized): struct<.native raw[?a, ?b], .sig TypeStruct>` |

The kernel's `.sig` field is *declared* as the read of the argument's type; the
read's **value** is right (`compute.launch k 5` is `10`, and `k.sig` still gates
as a function type), and — as it turns out — so is its **declared type**: a
class dump of the `.sig` field-type node shows the committed value *is* the
arrow `[[Int, Int], [TypeFunction, K]]`.  Only the **printing** was wrong.  Three
assertions measured it (the middle row above is what they pinned, and they were
`#[ignore]`d until the printer fix landed):

- `a_kernel_value_and_type_render_by_name` and
  `a_tuple_domain_kernel_type_renders_as_a_function` in
  `crates/lichen-language/tests/compute.rs`;
- `compute_kernel_bindings_render_by_name_not_raw_layout` in
  `crates/lichen-language-server/tests/statement_values.rs` (the LSP-visible
  half: the same kernel type through the language server's snapshot).

### The minimal reproducer (no native plugin needed)

```lichen
let type_of = x => {t = _; x: t; t}
jit = f => {
  f : _ -> _
  (struct<.native _, .sig (type_of f)>)(.native 0, .sig _)
}
jit (y => y + 1)
```

The read must sit in the struct type's **field-type** position and the
argument's type must be pinned to a **fresh arrow** (`f : _ -> _` here; the
plugin's function-ness gate does the same with `ctx.arrow(ctx.fresh(), ctx.fresh())`).
The builtin rendered `.sig Int -> Int` for exactly this program; the library
definition rendered `.sig TypeStruct` (until the printer fix).  With the read in
a *value* position the same shape was always correct (`f : _ -> _; type_of f` is
`Int -> Int: TypeFunction`), and with `.native 0` replaced by a concrete value
and no gate the field type was correct too.

### The mechanism (corrected — the earlier reading is withdrawn)

The defect was **not** in the checker, the deferral, or the wrapper: no
`defer_pending` verdict fires anywhere in the repro (measured), and the graph
commits the correct arrow onto the `.sig` field type.  It was the printer: the
renderer's `is_universe` asked whether a node *contains* the universe instead of
whether it *is* it, and the gate's arrow — built inside the **frozen** module —
has a kind `[TypeFunction, K_static]` whose tail is the static universe.  The
struct-kind branch then also needs `marker_is_struct`'s "any 2-element array"
guess to fire on the shape `[Int, Int]`, and the two together misread the arrow
type value as a struct kind.  Without the gate the arrow comes from the argument
(local, dynamic `K`) and everything prints correctly — which is why every
wrapper-level experiment below changed nothing: they moved *when* the read
happens, never *where the arrow's `K` comes from*.

Full analysis and the landed one-function fix (`is_universe` tests the whole
`[Type, ↺]` shape):
[universe-containment](universe-containment.md) §2.

### What was tried and did **not** work

Every spelling-level dodge, and one plugin-side change — all still rendered
`TypeStruct` (as § *The mechanism* explains: none of them touched the printer):

- hoisting the read into a binding (`s = type_of f`, then `.sig s`);
- pinning the parameter first (`f : _ -> _` as a leading statement, i.e. the
  annotated-parameter desugar) with the read before or after it;
- running the plugin's gate first (`n = $jit(f)` before the read);
- giving `JitOp`'s function-ness gate `LaunchOp`'s **lazy Index** shape
  (reading the domain/codomain out of `f.ty` instead of two fresh cells) — this
  one needs `P::Operator: From<LowOperator>` on the impl and still does not fix
  the render; it was reverted.


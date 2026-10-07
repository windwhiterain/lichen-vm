# `type_of` is a standard-library function, not a language form

> Status: current — the keyword, the AST form, the highlevel `ExprKind::TypeOf`
> and its checker special case are gone; the type read lives in
> [lichen-std/_.lichen](../../lichen-std/_.lichen).  The `.sig` mis-render the
> removal first exposed is **fixed**: it was a *printer* misclassification of a
> frozen kind, not a checker or deferral fault —
> ["contains the universe" being read as "is the
> universe"](universe-containment.md) §2.  A **second, still-open** defect of the
> same read — it is monomorphic when a lambda returns it, found while writing the
> operator contract in lichen — is recorded below (*The monomorphism a wrapping
> lambda induces*).
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

The read is **not** in the built-in [prelude `core`](core-prelude.md), and the
split is deliberate: the prelude carries the operators' *contract* (`Num`,
`in_num`, and a binding per operator), which every program needs, while the read
is a library function a program imports when it wants one — and a contract no
longer needs it at all, because a class refinement's predicate receives the type
directly ([operator-polymorphism](operator-polymorphism.md) §3).

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

The kernel's `.sig` field **was** *declared* as the read of the argument's type;
the read's **value** was right (`compute.launch k 5` returned `10`, and `k.sig`
gated as a function type), and — as it turns out — so was its **declared type**: a
class dump of the `.sig` field-type node showed the committed value *was* the
arrow `[[Int, Int], [TypeFunction, K]]`.  Only the **printing** was wrong.  That
read is what the removal exposed: it rides in the successors of that field, the
`.I`/`.O` of `struct<.native _, .I _, .O _>`.  Three assertions measured it (the
middle row above is what they pinned, and they were `#[ignore]`d until the
printer fix landed):

- `a_kernel_value_and_type_render_by_name` and
  `a_tuple_domain_kernel_type_renders_as_a_function` in
  `crates/lichen-language/tests/compute.rs`;
- `compute_kernel_bindings_render_by_name_not_raw_layout` in
  `crates/lichen-language-server/tests/statement_values.rs` (the LSP-visible
  half: the same kernel type through the language server's snapshot).

### The minimal reproducer (no native plugin needed)

The program is the wrapper spelling of the time, with its retired `.sig` field;
the wrapper carries the signature in `.I`/`.O` now:

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

## The monomorphism a wrapping lambda induces

> Status: **open** — pre-existing, reproducible on `dev` without any of the
> operator work, and found while writing the operator contract in lichen
> ([operator-polymorphism](operator-polymorphism.md) §3, §9 Phase 3).

The read is polymorphic **called directly** and **monomorphic when a lambda
returns it**:

| program | result |
|---|---|
| `(type_of 1, type_of 1.5)` | `(Int, Float): <Type, Type>` |
| `f = v => type_of v; (f 1, f 1.5)` | **refused**: `expected Int, found Float` |
| `f = v => type_of v; f 1` | `Int: Type` |
| `f = v => type_of v; f 1.5` | `Float: Type` |
| `f = v => type_of v; f` | `Function: raw[?a, ?b] -> ?b` |

The declaration is the tell: the wrapper's **domain prints as a raw pair**
(`raw[?a, ?b]`, not `?a`), because `type_of`'s declared result type *is* the
argument's type cell — the `x : t` annotation inside the read unifies `t` with the
parameter's type slot, and the body returns `t`, so the read's type and its
argument's type are one class.  Applying the wrapper twice then writes that one
cell twice: the *first* application's class sticks and the second is refused.  An
explicit `t : Type` statement in the read fixes the *printed* signature
(`?a -> Type`) but not the sharing, so the sharing is in the per-call
instantiation of the read's body, not in its declared type: the lowlevel apply
re-instantiates a function's conditions and parameter per call
(`apply_parameter_check`, `Function::asserts`), and a body cell that only an
annotation tied to the parameter's type slot is not among the nodes it remaps.

**Until it is fixed** a predicate that needs the class does **not** use the read
at all: the refinement is written on the **type** (`x : (_ ! in_num)`), so its
predicate receives the type value and `in_num = t => t @in Num` needs no read
([operator-polymorphism](operator-polymorphism.md) §3, §9 Phase 3).  Before that
landed, the workaround was to keep the read's cells out of the caller's type
through one opaque combinator:

```lichen
type_of = x => {t = _; x : t; t}
compose = f => g => x => f (g x)
in_num = compose (t => t @in Num) type_of     -- measured: (in_num 1, in_num 1.5, in_num "a") = (1, 1, 0)
```

It is kept here as the measurement that localised the defect, not as a spelling to
use: the class refinement supersedes it.

**The preferred fix** is the one the measurement points at: the per-call
instantiation must remap every cell in the equality class of the parameter's type
slot, so a body cell an annotation tied to it is re-instantiated too — which would
let the read be wrapped plainly (`in_num = v => type_of v @in Num`) and would
retire the `raw[?a, ?b]` domain.  It touches the apply/instantiation path the
kernel workstream also depends on, so it is its own change, not this one's.
**Where the open domain shows in public output**: `examples/import/math.lichen`
(`.add`) and `geometry.lichen` (`.double`) declare it in their `output =` lines,
because an arithmetic lambda over the refined `add` never pins its class; those
two declarations are re-pinned where the printer is, and this fix is what would
retire them as well ([operator-polymorphism](operator-polymorphism.md) §7.1
cost 2).

The read itself is unchanged and stays where it is: `type_of` is an ordinary
library function ([lichen-std/\_.lichen](../../lichen-std/_.lichen)), and every
consumer that wants the read at two classes in one program either calls it
directly or goes through the combinator above.



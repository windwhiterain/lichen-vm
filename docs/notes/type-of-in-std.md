# `type_of` is a standard-library function, not a language form

> Status: current — the keyword, the AST form, the highlevel `ExprKind::TypeOf`
> and its checker special case are gone; the type read lives in
> [lichen-std/_.lichen](../../lichen-std/_.lichen).  **One defect is open** (§
> *Open defect*): a library type read used as a struct field's declared type
> does not behave like the builtin's lazy read, so three rendering assertions
> are parked with `#[ignore]` (they are the acceptance test for the fix).> Points at: `crates/lichen-language-lex`, `crates/lichen-language-parser`
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

## Open defect: a library read is not the builtin read in a field-type position

**Status: open — not fixed, deliberately parked for a separate change.**  The
removal shipped with this known gap; the two `compute` assertions that measure
it are `#[ignore]`d with the note cited in the reason string, so they are the
acceptance test for whoever closes this.

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
read's **value** is still right (`compute.launch k 5` is `10`, and `k.sig` still
gates as a function type), but the field's **declared type** resolves to the
enclosing struct type's kind marker instead of the signature.  Three assertions
measure it, all parked with `#[ignore]` and this note in the reason string:

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
The builtin renders `.sig Int -> Int` for exactly this program; the library
definition renders `.sig TypeStruct`.  With the read in a *value* position the
same shape is correct (`f : _ -> _; type_of f` is `Int -> Int: TypeFunction`),
and with `.native 0` replaced by a concrete value and no gate the field type is
correct too.

### What differs, mechanically

- The builtin (`Checker::check_type_of`, now deleted) set the read's
  `state[e].term` to `Index(operand_pair, 1)` — a **plain lazy element read**
  over the operand's pair, so the template rewrite that reaches the operand's
  pair also reaches the read.
- A call's `term` is the **Apply node itself**
  (`crates/lichen-highlevel/src/checker/lambda.rs`, the `check_apply` tail: the
  node's operands are `[function_value, argument_pair, result_cell]` and "the
  apply node *is* the return pair" once the runtime apply writes it).
- A struct field's declared type is `state[el].term`
  (`crates/lichen-highlevel/src/checker/tuples.rs`, `check_type_element`), read
  while the call is still unresolved in the frozen module.
- The deferral then commits the wrong side: the gate's unification makes the
  read pending, and the pin
  (`crates/lichen-lowlevel/src/equality.rs`, `pin_committed_value` /
  `is_pending_apply`, reached from
  `crates/lichen-highlevel/src/shape.rs`, `defer_pending`) commits the
  enclosing struct type's kind onto the class instead of the argument's arrow.

See [defer-pending-type-forms](defer-pending-type-forms.md) for the machinery
this rides on (that note is the fix's starting point).

### What was tried and did **not** work

Every spelling-level dodge, and one plugin-side change — all still render
`TypeStruct`:

- hoisting the read into a binding (`s = type_of f`, then `.sig s`);
- pinning the parameter first (`f : _ -> _` as a leading statement, i.e. the
  annotated-parameter desugar) with the read before or after it;
- running the plugin's gate first (`n = $jit(f)` before the read);
- giving `JitOp`'s function-ness gate `LaunchOp`'s **lazy Index** shape
  (reading the domain/codomain out of `f.ty` instead of two fresh cells) — this
  one needs `P::Operator: From<LowOperator>` on the impl and still does not fix
  the render; it was reverted.

So the fix is on the checker side (making an unresolved call result usable as a
type expression the way the builtin's raw read was), not in the wrapper.


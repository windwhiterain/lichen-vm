
# The erroring API usages against the Ideal API

> Status: **review.** This note audits the field/parameter spellings that the strict
> branch's `examples/convolution2d.lichen` still fails on, against the five forms in
> `docs/language-spec.md` §Ideal API. Two spellings are measured: the example's current
> one fails with a collide, and the Ideal API's value/type split fails with a different
> diagnostic. Neither is accepted by this implementation, which is the finding.

## The forms

1. Uniform signature: `sum3 := [@any: Int, 3]['a, 'b, 'c] -> 'a + 'b + 'c`.
2. Named signature: `div := {.'dividend: Int, .'dividor: Int} -> dividend / dividor`.
3. Struct signature: `Div := {.dividend @any: Int, .dividor @any: Int}` and
   `div := 'x = Div -> x.dividend / x.dividor`.
4. Wildcard generic: `Div := {.dividen @any, .dividor @any}`.
5. Function generic: `Div := 'T -> {.dividen @any: T, .dividor @any: T}` and
   `div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor`.

The split each form keeps is **field name, value, type**: form 3 writes `@any` as the
value and `Int` as the type; form 5 writes `'<name>: T`, a type-valued field.

## What the examples write

`examples/convolution2d.lichen` (and the same shape in `compute.lichen`):

```lichen
CIn  := {.x (compute.Buf (compute.BufParam{.type Float, .length 25})), .w (compute.Buf (compute.BufParam{.type Float, .length 9}))}
CPar := {.n @any : Int, .cols @any : Int, .taps @any : Int, .in (CIn)}
XBuf := {.x (compute.Buf (compute.BufParam{.type Float, .length 25}))}
```

Each `Buf` field's **value is a type**: the constructor application `(compute.Buf …)`.
No Ideal API form writes a type in a **value** position; form 3 puts the value first
(`@any`) and the type second, and form 5's `'<name>: T` states the field *is* a type
only inside a type function's body.

Measured against the strict branch (`strict-clean`, after the launch fix):
`CIn{.x x.x, .w weights.w}` fails at `102:79` — `expected ?, found ?`, the collide of
`blurred`'s `x.x` and `replicated`'s `edge.x` in the declared `Buf` type's `.native`
cell (see [one-answer-per-class.md](one-answer-per-class.md)).

## The Ideal API rewrite, measured

Rewriting the same fields to the form-3 split (`{.x @any: (compute.Buf …)}`,
`{.in @any: (CIn)}`, and the same for `COut`/`XBuf`/`WBuf`/`EBuf`) produces a different
failure:

```text
error: expected @any, found Struct
   --> 102:72
```

So the implementation accepts neither spelling: the current one merges two instances'
buffers into one shared declared-type cell, and the Ideal API's split rejects the
struct-valued argument against the `@any` value. Form 3's `@any: Int` case works for
scalars (`Int`), but its `@any: (compute.Buf …)` case with a struct argument does not.

## The usages that are conformant

- `convolve_raw := 'k = CPar -> …` — form 3 (`div := 'x = Div -> …`).
- `Buf := 'x = BufParam -> {.native @any, .type x.type, .length x.length}` — form 3's
  parameter; the body reads the parameter's fields (`x.type`), as form 3's body does
  (`x.dividend`).
- `(compute.Read _){.from …, .at …}` — form 5's constructor application
  `(Div _){.dividen 1, .dividor 2}`.
- `plrun := 'k = K -> 'a = (k.I _) -> …` — form 3 for `k`; the **inner** parameter
  `'a = (k.I _)` has no Ideal API form: its type depends on a **value** parameter
  (`k`'s field), while form 5's dependency is on a type parameter (`T`).

## The finding

The erroring usages are the **type-in-value-position fields** (`{.x (compute.Buf …)}`)
and the **dependent inner parameter** (`'a = (k.I _)`). The first is not an Ideal API
form; the Ideal API's replacement for it is rejected by this implementation with
`expected @any, found Struct`. The second has no Ideal API form at all — the SPEC's forms only parameterize over types, not over a value parameter's field.

## Why `@any` and `Struct` report a unify error

Measured (`DBG_APPLY` probes in `unify_inner`'s recording sites, then removed):

- The failing unify is at **slot 0, the value position**: the shared `Struct` template
  marker (`LowValue::TypeValue(TypeStruct)`, the type every struct value carries) against a
  struct **type value** (an `Array` holding the instance's name table and fields).
- `record_error` receives exactly that pair, twice, once per `CIn` instance
  (`102:72`, `102:76`).
- The comparison that fails is `value_expr_equal`'s raw `value_eq`: the template marker and
  the array are different values, so the unify errors. The `@any` in the message is the
  field's wildcard value on the other side, not the side that conflicts.

So the report is not "`@any` versus `Struct`": it is the **`Struct` template against a
concrete struct type value**. Per the SPEC §Struct the template is the shared layout every
struct value instantiates (`v := @raw[name_table, nominal_id, @raw None, …]`), so the two
should unify; the implementation compares their raw values instead. That is the next fix:
the `Struct` template is a wildcard for struct-typed positions, exactly as `@any` is for
values.

### The call site, measured

The failing pair comes from `check_instantiate`'s **type-declaring-field branch**
(`crates/lichen-highlevel/src/checker/structs.rs:993`):

```rust
let declared = self.module.expr_value_half(field);   // the field's VALUE half
let actual_ty = self.state[argument_expr].ty;        // the argument's TYPE
self.check_unify(actual_ty, declared, …)
```

For a field written `@any: (CIn)` the field's **value half resolves to the declared type
value** (the `CIn` array), while the argument's type is the shared `Struct` template
(`[StructType, ↺]`, every struct value's type). The descent recorded at the recording site:
root `(34560, 1185)` → slot 1 (the type position) → the two type exprs `(34, 774)` → slot 0
their values `(8, 770)` — node 8 is the `StructType` marker, node 770 the struct type value.
That is exactly the Array-versus-StructType pair: the SPEC's annotation sugar
(`ann1 = ann2`) unifies the argument's type attribute with the annotation expression, and the
two are the generic template and a concrete type value.

So the unify is *by design* the annotation check, and the pair is a **type versus a type
value** — the generic `Struct` against a specific struct type. The next question is which of
the two the annotation is meant to compare: the SPEC's `: T` unifies the *type attribute*
with T, so a struct value can only be annotated `: Struct`; annotating it `: CIn` compares
the generic template with the concrete type value and cannot succeed.

### Conformance of the Ideal API forms

The SPEC's own form-3 program was run twice as it stands:

```lichen
Div := {.dividend @any: Int, .dividor @any: Int}
div := 'x = Div -> x.dividend / x.dividor
(div _{.dividend 6, .dividor 3}, div _{.dividend 8, .dividor 4})
```

- **Before the rebase onto dev `e28164e8`:** `(2, 2)`, `EXIT=0` — the form works.
- **After the rebase** (same program, fresh `LICHEN_HOME`): `(none, none)` — both calls are
  undecided.

So the implementation does support the Ideal API's forms; the regression is in the rebase
adaptation (the dropped `resolved_field`/named-field callee arm, or dev's declared-field
deletions), not in the forms. The named-type instance (`CIn2{.a 6, .b 3}`) and the
type-in-value fields also answer `none`, so the forms cannot be exercised through them on
this branch. The next step is to restore the form-3 behaviour on the rebased branch before
judging the example's usages.

### The Nested form (6), audited

The SPEC now carries form 6:

```lichen
A := {.x @any: Int, .y @any: Float}
B := {.a A, .z @any: String}
f := {.'a=A, .'b=B, .w': Type} -> ...
```

- **`B := {.a A, …}` is the pattern the examples use** for `CIn`/`CPar` (`{.x (compute.Buf …)}`,
  `{.in (CIn)}`). The Nested section therefore **sanctions it**: the earlier review's verdict
  that a type in a field's value position is "not an Ideal API form" is superseded, and the
  collide in `convolution2d` is an implementation gap, not a non-conformant usage.
- **`String` does not resolve**: the language spells the builtin `string`
  (`B := {.a A, .z @any: string}` parses), so form 6's `String` is a typo in the SPEC.
- **The signature spelling is not understood**: `f := {.'a=A, .'b=B}` with the caller
  `f _{.a …, .b …}` reports `no field named b in the struct type Struct` — the anonymous
  signature does not give the instance a name table, so the field lookup is on the generic
  `Struct`.
- **The named nested form is undecided**: with
  `S := {.a A, .b B}; f := 's = S -> s.a.x; f _{…}` the answer is `(none, none)` — the same
  reading the plain form 3 gives on the **rebased** branch (it answered `(2, 2)` before the
  rebase).

So the red tests' first blocker is the rebase regression on the struct-signature call path,
and form 6's own two spellings (`String`, the anonymous signature) are additional
conformance findings.

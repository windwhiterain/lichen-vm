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
`expected @any, found Struct`. The second has no Ideal API form at all — the SPEC's
forms only parameterize over types, not over a value parameter's field.

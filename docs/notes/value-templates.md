# Constructor templates

> Status: in progress
> Points at: `crates/lichen-highlevel/src/checker` (value construction and reads),
> `crates/lichen-language-parser` (the template syntax), and
> `crates/lichen-language/src/compile.rs` (lowering).

Compound layout lives in a value, not in a shape-bearing type. `Tuple`, `Array`,
and `Struct` are the shared layout types; a template is a function whose
application produces a fresh requirement. The source of truth is [the language
spec](../language-spec.md); this note records the migration decisions and the
reasons for them.

## Confirmed rulings

- A template is a **constructor**: a function that, applied once, returns a
  fresh requirement. `S := _x => {.a _, .b _}` is a struct constructor, and
  `I := _x => _` is a constructor that returns a bare hole.
- Ordinary `:` annotation is unchanged. It is not a template constructor.
- The match operator applies the constructor exactly once:

  ```text
  actual <: S  =>  actual = S _
  ```

  `actual <: S _` is a **double application** — the `_` is written twice — and
  is not the match.
- Plain `=` performs **no application**. It is the unify for an already
  evaluated requirement, for a call-result template, and for a direct function
  signature.
- A parameter uses the same rule by desugaring into its body:

  ```text
  x <: T => body  =>  x => { x = T _; body }
  ```

- There is no source-level `@clone`. Freshness comes from function
  application, whose existing parameter scope instantiates the body and
  shares captured cells. Cloning every reachable node would be wrong: an outer
  `x` must not be cloned into an unrelated class when a struct is
  instantiated.
- A struct's constructor is a function, for example
  `S := _x => {.x @any : Int}`. `S{.x 1}` applies `S` to `_` once and fills the
  fresh struct value it returns. There is no second application. Writing
  `(S _){...}` applies the returned struct value again and is not the
  constructor form.
- An array instance also applies its constructor once. `T[x, y]` evaluates
  `T _`, checks the returned length, and unifies every element with the same
  returned element requirement. A comma-free `a[i]` is still an array read;
  `T[x,]` is the one-element instance and `T[]` the zero-element instance.
- Scalar wildcard fields use `@any : Int` or `@any : I`; bare `I` stores a type
  value, not an arbitrary value of type `I`.
- **A binary operator's operands stay the tuple ABI.** The operand group is
  `operator (left, right)`, not the array form `[left, right]`: the array
  requirement's `[requirement, length]` is a different two-slot shape, and a
  binary operator that borrowed its spelling would be ambiguous with it. The
  spec's binary sugar is corrected to `operator (left, right)`.
- The thin arrow `->` is removed. A function constraint can use plain `=` with
  a signature, or `<:` with a constructor that returns a signature.
  Unification compares the signature and preserves the actual callable body;
  it never replaces that body with the constructor's body.
- A call-result template is used with plain `=`, not `<:`. For example, a
  native wrapper states `r : type_of (k.O); r = k.O`. `<:` would apply `k.O`
  again, which is valid only when `k.O` is itself a constructor function.

## The direct signature

A signature is stated with plain `=`, which performs no application, and the
unification keeps the actual's own body:

```text
I := _x => _
f = (i <: I => _)
O := f (I _)
r = O
```

`f = (i <: I => _)` says that `f`'s parameter takes the requirement `I`
returns, applied once, and that `f`'s result is a hole. `O := f (I _)` applies
`f` to the same requirement, so `O` is the result the signature describes.
The result is then reused with plain `=`: `r = O` for a local result and
`r = k.O` for a result that came out of a kernel struct.

## Implementation direction

The existing lazy expression graph already supplies the required scope. A
function application instantiates the function's parameter and body, preserving
captured cells and nominal struct identity. The constructor result is an
ordinary value: `[name table, nominal id, fields…]` for a struct,
`[element template, elements…]` for an array, and `[elements…]` for a tuple.
The shared layout type is `Struct`, `Array`, or `Tuple` respectively.

The compiler performs only the two desugarings:

- `<:` inserts an application of the constructor to `_` before unification.
- Struct and array instantiation inserts the same application before reading
  the returned value's fields or element requirement.

No arbitrary-clone scope is inferred from graph reachability. Function scope,
closed-cell sharing, and the per-application remap remain the ownership model.
`Node::origin` is not a keep-alive edge.

## The renderer

The renderer reads layout from the type marker and the value's own offsets:
tuples start at `0`, arrays at `1` (after the element template), and structs at
`2` (after the name table and nominal id). Struct headers and array templates
are internal, so a struct prints as its field tuple and an array as its
elements. The type of a compound prints as `Tuple`, `Array`, or `Struct`.

Recursive bindings may leave a two-cell `[value, type]` skeleton. The renderer
reads those cells and guards cycles by equality class, not just node id; the
shared layout type does not truncate a nested value merely because it appears
again in the type position. Array template metadata is ignored for table-key
content but retained for template presence, keeping arrays and tuples distinct.

## The compute layer

The compute package's own source follows the same model. A kernel struct is a
constructor, `K := _x => {.native _, .I _, .O _}`, and a kernel is matched
against it with `<:` — `launch := k <: K => …` — which applies `K` once.

A computed struct call **returns a value**; it is not an instantiation.
`Buf x` returns the buffer struct, and the buffer is initialised by assigning
into it:

```text
buf := x <: BufParam => { b := Buf x; b.native = $buf(x.type, x.length); b }
```

`(Buf x){…}` is not a supported second instantiation: the call has already
returned a value, and the instantiation form applies a *constructor* once.

`type_of (k.O)` needs the space. `type_of (k.O)` is the application of
`type_of` to the field read; `type_of(k.O)` is a parenthesized read — the
tuple read of the name `type_of` — and it is refused as such.

## Dependent lengths

A decidable array length is built directly. A length that depends on an
enclosing lambda parameter is deferred through the constructor result's own
length read: the instance's count pins that cell, and a later argument mismatch
is a **runtime** error, not a check-time one. A top-level `_` remains guarded;
`[T, _]` does not infer a length. A layout-only constraint can use ordinary
`Array` annotation.

## Open details

- The lazy fallback for an arbitrary computed template is not yet a proven
  element/length check. Computed struct templates remain part of the compute
  API, and their reachability must be measured rather than assumed.
- Parallel compute lowering for buffer/struct-valued kernels remains under
  repair. The scalar nested `compute.jit` path returns `6: Int`.
- The highlevel clone boundary for nested lambdas is deliberately limited to
  enclosing lambda parameters. Constructor application must not be used as an
  excuse to infer lexical ownership for local `let` bindings.
- **Measured at `254cd301`:** the array instance path does not accept a
  constructor yet. `T := _x => [@any : Int, 2]` followed by `T[1, 2]` is
  refused with `expected Array, found Array`, and the plain array template
  `[@any : Int, 2][1, 2]` is refused as a call of a non-function. The struct
  instance path accepts a constructor (`S := _x => {.x @any : Int}` then
  `S{.x 1}`), and the tuple match accepts one
  (`T := _x => (@any : Int, @any : Int)` then `(1, 2) <: T`). The array
  instance is therefore documented as the rule, not as a measured acceptance.
- The language spec's grammar has no source-level `clone` form, but its
  *Function* section still spells an application's internal instantiation
  with the lowlevel `@clone` primitive. That primitive is the apply's own
  clone walk, which is what the constructor model reuses; it is not a form a
  program can write. The spec is the contract and is not edited here.

## Verification boundary

The slice is not complete when only the new syntax parses. It must demonstrate
that compound shape is read from values, constructor application produces fresh
requirements without cloning outer captures, `<:` applies exactly once, plain
`=` applies never, and no compiler consumer still requires the retired
`tuple{…}`, `array{…}`, `struct{…}`, or source `@clone` spellings.

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

- Ordinary `:` annotation is unchanged. It is not a template constructor.
- Template match applies the constructor once and unifies with its result:

  ```text
  actual <: template  =>  actual = template _
  ```

  Plain `=` performs no application and no clone. Use it for an already
  evaluated requirement or a function signature.
- A parameter uses the same rule by desugaring into its body:

  ```text
  x <: T => body  =>  x => { x = T _; body }
  ```

- There is no source-level `@clone`. Freshness comes from function application,
  whose existing parameter scope clones the body and shares captured cells.
  Cloning every reachable node would be wrong: an outer `x` must not be cloned
  into an unrelated class when a struct is instantiated.
- A struct template is a constructor function, for example
  `S := _x => {.x @any : Int}`. `S{.x 1}` applies `S` to `_` once and fills the
  fresh struct value it returns. There is no second clone. Writing `(S _){...}`
  applies the returned struct value again and is not the constructor form.
- An array instance also applies its constructor once. `T[x, y]` evaluates
  `T _`, checks the returned length, and unifies every element with the same
  returned element requirement. A comma-free `a[i]` is still an array read;
  `T[x,]` is the one-element instance and `T[]` the zero-element instance.
- Scalar wildcard fields use `@any : Int` or `@any : I`; bare `I` stores a type
  value, not an arbitrary value of type `I`.
- The thin arrow `->` is removed. A function constraint can use plain `=` with
  a signature, or `<:` with a function that returns a signature. Unification
  compares the signature and preserves the actual callable body; it never
  replaces that body with the template's body.
- A call-result template is used with plain `=`, not `<:`. For example, a
  native wrapper states `r : type_of (k.O); r = k.O`. `<:` would apply `k.O`
  again, which is valid only when `k.O` is itself a constructor function.

## Implementation direction

The existing lazy expression graph already supplies the required scope. A
function application instantiates the function's parameter and body, preserving
captured cells and nominal struct identity. The constructor result is an
ordinary value: `[name table, nominal id, fields…]` for a struct,
`[element template, elements…]` for an array, and `[elements…]` for a tuple.
The shared layout type is `Struct`, `Array`, or `Tuple` respectively.

The compiler performs only the two desugarings:

- `<:` inserts an application of the template to `_` before unification.
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

## Function templates

A function constraint is an ordinary signature unification. `<:` can apply a
constructor that returns a signature; plain `=` can use a signature directly.
In both cases static and dynamic functions are compared by parameter and return
cells, including the node-less static-reference path. The actual function body
is preserved.

Compute uses `f = (i <: I => _)` for a direct signature constraint and
`O := f I` to obtain a symbolic output. A constructor such as
`S := T <: KT _ => (i <: (A T.I) => _ : T.O)` remains a signature-template
constructor. Native wrappers own result types: `r : type_of (k.O); r = k.O`.
The space in `type_of (k.O)` is required; `type_of(k.O)` is a parenthesized read.

## Dependent lengths

A decidable array length is built directly. A length that depends on an
enclosing lambda parameter is deferred through the constructor result's own
length read: the instance's count pins that cell, and a later argument mismatch
is a runtime error. A top-level `_` remains guarded; `[T, _]` does not infer a
length. A layout-only constraint can use ordinary `Array` annotation.

## Open details

- The lazy fallback for an arbitrary computed template is not yet a proven
  element/length check. Computed struct templates remain part of the compute
  API, and their reachability must be measured rather than assumed.
- Parallel compute lowering for buffer/struct-valued kernels remains under
  repair. The scalar nested `compute.jit` path returns `6: Int`.
- The highlevel clone boundary for nested lambdas is deliberately limited to
  enclosing lambda parameters. Constructor application must not be used as an
  excuse to infer lexical ownership for local `let` bindings.

## Verification boundary

The slice is not complete when only the new syntax parses. It must demonstrate
that compound shape is read from values, constructor application produces fresh
requirements without cloning outer captures, `<:` applies exactly once, plain
`=` applies never, and no compiler consumer still requires the retired
`tuple{…}`, `array{…}`, `struct{…}`, or source `@clone` spellings.

# Value templates

> Status: in progress
> Points at: `crates/lichen-highlevel/src/checker` (value construction and reads),
> `crates/lichen-language-parser` (the template syntax), and
> `crates/lichen-language/src/compile.rs` (lowering).

The language is moving compound layout out of a value's type and into the value
itself. `Tuple`, `Array`, and `Struct` are layout templates, not shape-bearing
type expressions. The source of truth is [the language spec](../language-spec.md);
this note records the migration decisions and the reasons for them.

## Confirmed rulings

- Ordinary `:` annotation is unchanged. It is not the template-match operator.
- Template match is explicit and clones the template before unification:

  ```text
  actual <: template  =>  actual = @clone template
  ```

- A parameter uses the same rule by desugaring into its body:

  ```text
  x <: T => body  =>  x => { x <: T; body }
  ```

  There is no second parameter-annotation channel to keep in sync.
- This slice does not change scalar type annotations or make source compatibility
  with the retired compound-type spellings a goal.

## Implementation direction

The existing graph can represent the change without a second value model. The
self-referential universe can represent `Atomic`; the existing tuple and array
markers can form the shared `Tuple` and `Array` templates; the struct marker can
form `Struct` without the old kind payload. Element expressions, the struct name
table, and the nominal id move into the value. A read therefore checks the layout
template in the type position, then decodes the value at that template's offsets.

`@clone` must be the same instantiation operation as function application, not a
second graph walk with different freshness rules. One candidate is a hygienic
immediate application of a synthetic function whose body is the cloned expression:
local holes are instantiated while captured expressions stay shared. This candidate
must be checked for nested application, internal sharing, dependent templates, and
nominal-id preservation before it becomes the implementation contract.

## Open details

- A single comma-free `a[i]` is an array read. Array instantiation needs an
  unambiguous zero- or one-element spelling; the trailing-separator convention is
  the candidate consistent with the other instantiation forms.
- An array template's length may be unevaluated. Its representation must not force
  an arbitrary expression merely to allocate the value's repeated holes.
- The interaction of `@shallow` with template arrays and instances needs to be
  stated rather than inherited accidentally from the retired array literal.
- The spec's scalar `Atomic` paragraph is outside this slice. It must not be used
  to silently change `Int`, `Float`, `string`, or ordinary `:` semantics.

## Verification boundary

The slice is not complete when only the new syntax parses. It must demonstrate
that compound shape is read from values, parameter templates are checked through
`<:`, cloning does not bind the original template, and no compiler consumer still
requires the retired tuple/array/struct `[shape, kind]` encoding.

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
  with the retired compound-type spellings a goal. `Atomic` was rejected as a
  duplicate concept: the self-referential universe remains `Type`.
- An array instance clones its element template independently for each element.
  One clone preserves its own internal sharing, so a template such as `('a, 'a)`
  does not become two unrelated positions. Independent clones do not impose
  homogeneity across elements.
- Binary operators take a tuple `(left, right)`. The old `[left, right]` spelling
  now denotes an array template and cannot keep its former meaning.

## Implementation direction

The existing graph can represent the change without a second value model. The
self-referential universe remains `Type`; the existing tuple and array markers
can form the shared `Tuple` and `Array` templates; the struct marker can form
`Struct` without the old kind payload. Element expressions, the struct name
table, and the nominal id move into the value. A read therefore checks the layout
template in the type position, then decodes the value at that template's offsets.

`@clone` must instantiate the value graph reached through a name, not merely the
syntax at the call site. The lowlevel provides [the template
instantiation](template-instantiation.md), which reuses the apply's clone rules
and class regroup with an explicit caller-owned sharing boundary. A synthetic
immediate lambda application was rejected: `T := (_, _); x <: T` would capture
the name and share the template's holes, so the match would write back into `T`.

## The renderer

The renderer reads the layout the same way the checker does: from the **type
marker** and the **value's own offsets**. It never reads a shape out of a type.

- A tuple value is `[element exprs…]` and its type is the shared `Tuple`
  template; the renderer reads the elements from offset `0`.
- An array value is `[element template, element exprs…]` and its type is the
  shared `Array` template; the elements start at offset `1`.
- A struct value is `[name table, nominal id, field exprs…]` and its type is the
  shared `Struct` template; the fields start at offset `2`.

The offsets are the highlevel's (`shape::TUPLE_VALUE_ELEMENT_BASE`,
`shape::ARRAY_VALUE_ELEMENT_BASE`, `shape::STRUCT_VALUE_FIELD_BASE`); the
renderer names them rather than writing its own `+1` or `+2`.

The **header** of a struct value — its name table and its nominal id — and the
**template** of an array value are internal: they name the layout and are never
user data, so a struct reads as its field tuple and an array as its elements.

The **type** of a compound is the template itself, so a type prints as `Tuple`,
`Array` or `Struct`; the element types live in the elements' own expressions
and are read from there. The retired `tuple{…}`, `array{…}` and `struct{…}`
spellings are gone, and so are the `TypeTuple`, `TypeArray` and `TypeStruct`
markers' names: the templates are `Tuple`, `Array` and `Struct`.

A struct's **field names** live in the value's own name table, so a reader that
wants them reads the **value**, not the type. The diagnostic contract follows:
a struct field diagnostic names the struct's value in `a` and its type in `b`.
The `show_struct_ids` switch and the `struct_type_named_fields` reader are gone
with the type that carried the names; `struct_value_named_fields` reads the
value's table instead.

## Open details

- A single comma-free `a[i]` is an array read. `T[x,]` is a one-element
  instance; `T[]` and `T[x, y]` are the zero- and multi-element forms.
- An array template's length may be unevaluated. Its representation must not force
  an arbitrary expression merely to allocate the value's repeated holes.
- The interaction of `@shallow` with template arrays and instances needs to be
  stated rather than inherited accidentally from the retired array literal.

## Verification boundary

The slice is not complete when only the new syntax parses. It must demonstrate
that compound shape is read from values, parameter templates are checked through
`<:`, cloning does not bind the original template, and no compiler consumer still
requires the retired tuple/array/struct `[shape, kind]` encoding.

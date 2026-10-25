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
- A compound domain in a function arrow uses template matching. Scalar domains
  and the ordinary `:` stay unchanged. Generic hole I/O inference is required:
  a whole bare hole in a domain or a codomain captures the actual parameter's
  value and type, or the actual return's type and value.
- A single comma-free `a[i]` is an array read. `T[x,]` is a one-element
  instance; `T[]` and `T[x, y]` are the zero- and multi-element forms.

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

## The arrow's two sides

A written arrow `A -> B` is a **signature**: a real function whose parameter
states the domain and whose return states the codomain. The two sides do not
take the same kind of statement, and the difference is the difference between
`:` and `<:`.

### The domain

The domain is classified after a deep evaluation, and the class decides what
the signature's parameter holds.

- A **scalar or type-valued** domain — `Int`, `Type`, the bare `Struct`, a
  nested scalar function type — states a *type*. It fills the parameter's
  **type slot**, as it always did.
- A **bare open hole** — `_`, or a name bound to one (`I := _`) — states
  *nothing*. It fills the **whole** parameter, so the actual parameter's value
  and type are captured in the hole's own cells. No clone is taken: the hole is
  the capture.
- A **compound template** — a tuple, an array or a struct value, whose type is
  the shared `Tuple`/`Array`/`Struct` layout — states a *value*. It is
  **cloned** with the ordinary template instantiation, and the whole parameter
  unifies with the clone, exactly `<:`. The clone's holes are fresh, so a
  match writes into the clone and never into the written template; a *named*
  template's holes are not shared either, because the clone boundary shares
  only the enclosing lambda's parameters.
- An **operation that has not decided** is cloned and whole-unified as well, so
  the parameter decides with it. This is the one place where a scalar domain
  behaves like a value: an operation domain that later decides to a scalar
  leaves the parameter's *value* bound to the operation's answer instead of the
  type slot. It is a limitation, not a rule.

The apply's function-ness guard has no written sides at all: its two cells are
fresh, so the pattern states nothing and stays harmless.

### The codomain

The codomain's **type half** is the signature's return type when the codomain
is a hole or a compound template — a hole's type half captures the actual
function's return type, and a compound template's is the layout. A scalar
codomain is a type value and keeps its whole denotation, as before.

### The directed value capture

The generic function equality compares **type cells**, so it never carries a
return *value*. Where the check knows both the arrow's syntax and the actual
function — an annotation `f : A -> B` — the codomain is therefore directed at
the function's return:

- a **bare hole** takes the return's value, while its type half stays the
  layout;
- a **compound codomain** is cloned and the whole return is matched against
  the clone.

The return is never forced: an undecided answer states nothing, so there is
nothing to take and nothing to match, and a scalar return keeps the ordinary
type behaviour.

Two limits are deliberate. A **frozen** callee is not captured: its return
lives in another module, and reading it would need lowlevel metadata this
slice does not add. A parameter whose type is an arrow (`x : A -> B`) cannot
be captured locally either: the parameter's value is bound at the apply, in
the lowlevel's generic function equality, which compares type cells only. The
codomain hole's **type** half is captured there; its **value** half is not,
and no local capture reaches it.

### The poison channel

A captured compound input or output is a **value**, not a type. It must be
consumed with `<:` — which clones it — and never with `:` — which would pin the
captured value into the place that uses it. A captured scalar input or output is
also a full expression: its value stays open and its type half states the scalar
type, so the compute prelude can consume both scalar and compound I/O with `<:`.
A concrete scalar type constant still uses ordinary `:`.

Only a **whole** bare hole captures. A hole *inside* a concrete template is a
match position: the template is cloned and the hole is cloned with it, so the
match fills it and never shares it with the written template.

## Open details

- An array template's length may be unevaluated. Its representation must not force
  an arbitrary expression merely to allocate the value's repeated holes. The
  current checker builds a decidable length directly and refuses an instance
  whose cloned value or length is not decidable; that is a limitation, not the
  dynamic-array semantics. A layout-only constraint can use ordinary `Array`
  annotation: `std.sort` does so, and the native operator retains its non-`Int`
  rejection rather than pretending `[T, _]` already infers the length.
- The interaction of `@shallow` with template arrays and instances needs to be
  stated rather than inherited accidentally from the retired array literal.
- The highlevel clone boundary currently shares only enclosing lambda
  parameters. It deliberately does not infer lexical ownership for local
  bindings in nested lambdas.
- Compound arrow domains and bare-hole capture are implemented. Remaining limits
  are a frozen callee's return value, a parameter-typed arrow's codomain value,
  and a deferred operation domain that later decides to a scalar; **The arrow's
  two sides** above records the exact boundary.

## Verification boundary

The slice is not complete when only the new syntax parses. It must demonstrate
that compound shape is read from values, parameter templates are checked through
`<:`, cloning does not bind the original template, and no compiler consumer still
requires the retired tuple/array/struct `[shape, kind]` encoding.

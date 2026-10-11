# Compound element slots

> Status: **in progress.** `LowValue::Any` has landed and is merged
> (`8b2fa7a`); every compound's element is a full `[value, type, attrs…]` slot —
> tuple, struct, array, table, and set — with both construction sites, the
> readers, the printers, the element-attribute and value-pin reconciliation, the
> wildcard literal `@any`, a value-less annotation `: T # a`, `ShapeElements`
> gone, the angle spellings retired (`tuple{…}` / `struct{…}` / `array{T, n}`),
> the type-component read back as `X{n}` with the table read at `e <== key`, and
> set/table types spelled `set{E}` / `table{K, V}` against their `set(…)` /
> `table(…)` values. What remains is in the open list below.
>
> Companion: [function-type-merge](function-type-merge.md) is the same refactor
> for the function kind, and it is the precedent every choice below is read
> against.
>
> Points at: `crates/lichen-lowlevel/src/{lib,codec,equality,evaluation,table}.rs`
> (`Any`), `crates/lichen-highlevel/src/checker/tuples.rs` (both construction
> sites), `crates/lichen-highlevel/src/shape.rs` (the element-slot readers),
> `crates/lichen-render/src/render/{type_printer,value_printer}.rs` (`@any`).

## `Any`, the wildcard

A written compound type is one node that many values are checked against, so its
slots must not bind. A plain placeholder (`struct<_:Int>`) does bind: the cell is
one cell, the first value unified against it is written into its class, and every
later use reads that value — which is why the alternative was a type lambda
(`type = _x => struct<_:Int>; (type _)`) instantiated per use. `Any` is the
constant that removes that burden:

- **Unification.** A class holding `Any` unifies with anything, and *neither
  merges the classes nor writes a value* (`Module::unify_inner`, checked before
  the undecided/decided write arms). The wildcard node therefore stays reusable,
  and nothing is ever bound by it.
- **A value reader** treats it as **undecided**: evaluation answers no value
  (`evaluate_node_body`), it states no low type, a table key that is `Any` is not
  key content (`table.rs`), and both printers spell it `@any`.
- **The concreteness verdict alone** takes it as **decided**
  (`evaluate_node_deep_inner`). That is what lets a structure a wildcard sits in
  be proven concrete: a written compound type is content-complete, so an apply
  clone references it **in place** instead of copying an array whose own value no
  deep pass ever wrote. Without the rule every wildcard-bearing type is unproven,
  and both a printer reading it and an assert condition over it answer nothing.
- **Codec.** Tag 9, additive; tag 4 stays reserved for the deleted marker.

## The element slot is a full term

A compound type's element slot used to hold the element's *type value* alone, so
an element's own value and attribute had nowhere to sit: `type_denotation`
stripped the annotation, and a perspective written on a type's element was
dropped. Measured on the tree before this change:

```text
(1 # 2, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
(1 # 4, 2) : <Int # 2, Int>   =>  (1, 2): <Int, Int>
```

Both checked, and neither carried the `# 2` anywhere. Every element is now an
expression's term — `[value, type, attrs…]` — inside the kinded wrapper
`[ [slot…], [marker, K] ]`, with one construction site per kind.

The slot's **width is dense over the attribute order**: `2 + |P::Attr::ORDER|`
positions, `shape::slot_width`. The syntax stays sparse; the width does not,
because a value's slot list and a written type's are unified position by
position, and no value can know how many attributes a written type will spell.

```text
<Int # 2, string>   ≡   tuple{ @any : (Int # 2), @any : string }   # full syntax
  element 0 = [ @any, int_type,    <perspective 2>, @any, @any ]
  element 1 = [ @any, string_type, @any,            @any, @any ]

(1, "a")   # a literal's type is the compound's shape: every value slot `@any`
  element 0 = [ @any, int_type,    @any, @any, @any ]
  element 1 = [ @any, string_type, @any, @any, @any ]
```

Decisions that shape this layout:

- **A value's element value slot is `@any`.** The type is the compound's
  *shape*, so `[(1, 2), (3, 4)]` and `f = x => (x, 1); [f 1, f 2]` stay legal
  (measured before the change as `array<<Int, Int>, 2>`), and one written type
  stays shareable across values that differ. The function precedent answers the
  same question the same way: a function's type carries its return's *type*, never
  its value. A written **pin** therefore rides in a written type's slot and is
  enforced by a checker walk over the value's own element terms, not by the
  lowlevel structural compare.
- **An element's attributes are carried *and* reconciled.** The dense attribute
  positions of a written type's slot hold the element expression's own attribute
  slots (`checker::element_attr_slots`), so a written perspective is no longer
  dropped. The value's side holds `@any`, so the lowlevel compare is vacuous by
  construction; the requirement is compared by marker through `unify_slots`, the
  way the function kind's declared parameter attribute is — a walk
  (`checker::unify_element_attributes`) that runs wherever a type expression meets
  the value it types, one element at a time, descending nested compounds.

  Its rule is the whole annotation's, not a second one: a side that spells no
  attribute contributes the **missing** slot, so `(1 # 4, 2) : <Int, Int>` is
  refused exactly as `(1 # 4) : Int` is — a value's claim must be authorized
  (`declared | value`), and an unspelled requirement authorizes none. Measured:

  ```text
  (1 # 2, 2) : <Int # 4, Int>          =>  refused: expected 4, found 2
  (1 # 4, 2) : <Int # 4, Int>          =>  (1, 2): <Int, Int>
  ((1 # 2, 2), 3) : <<Int # 4, Int>, Int>  =>  refused: expected 4, found 2
  ```

The readers follow the slot, and the layout is transitional until the struct step
lands:

- `shape::slot_type` reads an element's type position; `shape::field_type` takes
  a `ShapeElements` its caller states — `Slots` for a tuple type's shape,
  `Types` for a struct's — so no reader infers a layout from a node that may not
  be evaluated yet.
- `shape::low_type_of`'s tuple arm decodes each element through its slot;
  `checker::indexing::element_read` reads a slot container's component one level
  in; `checker::structs::slot_read`'s **lazy** fallback does the same, which is
  what a resolved read and an apply-time one must agree about.
- Both printers render a tuple's elements through their slots, and an unreadable
  slot falls back to the raw mark rather than printing an element pair whole.

## The spelling

A compound type writes each element as a **slot**: `tuple{a, b}` for a tuple,
`struct{.f a}` for a struct, `array{T, n}` for an array. A slot is `value : type`,
so an element with a top-level `:` spells the value position — a **pin** — and an
element without one is a type expression, whose value position is `@any`:

```text
tuple{Int # 2}            is  tuple{@any : (Int # 2)}
tuple{5 : Int, Int}           (5, 2) holds, (6, 2) is refused: expected 5, found 6
t = tuple{5 : Int, Int}       (6, 2) : t is still refused — the pin rides in the type
```

The angle spellings this note was written against (`<a, b>`, `struct<.f a>`,
`array<T, n>`) were retired afterwards. The raw index `X<e>` was removed with them
and then brought back as the **type access** `X{n}` — the table read gave up
`e{key}` for `e <== key`, so `{…}` reads a type component and `<`/`>` are
comparisons alone ([type-access.md](type-access.md)). Measurements recorded before
those changes keep their spelling.

A pin cannot be enforced by the lowlevel compare — a value's element slots hold
the wildcard — so it is reconciled checker-side (`checker::unify_element_values`)
exactly as an attribute is, and `@any : T`'s arm is vacuous because a wildcard's
own class accepts everything. Both printers read a slot back as source, so a pin
is visible: `(5, 2) : tuple{5 : Int, Int}` renders `(5, 2): tuple{5 : Int, Int}`,
while `@any`'s own position renders as no pin at all.

A slot's *attributes* are the **element's own**: `5 : Int # 2` folds into one
annotation whose tail holds the requirement, and a bare `Int # 2` element is that
same annotation with no type — so the slot reads its attributes from the element
the source wrote, not from the type child. The printers spell them back through
the attribute registry `render_attributes` reads (an unspelled position, the
wildcard included, answers `None`), so a requirement stays visible:
`(1 # 4, 2) : tuple{Int # 2, Int}` renders `(1, 2): tuple{Int # 2, Int}`.

## A value-less annotation

An annotation chain may omit its value: `: T # a` lowers to exactly the
annotation `@any : T # a`, so the *slot* rule needs no second spelling — a
compound element or field is an expression, and one that spells no value is the
wildcard's own position. `struct{.a :Int}` is then the struct form's field,
spelled with braces, and no parallel parser path is needed for it.

## The struct step

The struct's shape was the last bare type list, so a struct field could not state
a value or an attribute and the two compound kinds could not share a reader. It
is now the same slot list, with three consequences worth recording:

- **A field-type list is the slots' type positions.** Every reader that walks a
  struct's shape goes through one of two spellings: `shape::field_type`, or
  `checker::slot_types_of` when the list is compared positionally. A struct
  instantiation unifies the argument types against that list, so its field check
  is the same one a tuple's is.
- **A deferred callee's field list is read lazily**, one
  `Index(Index(shape, i), 1)` per position, which is what makes a *correct*
  deferred positional instantiation check at all
  (`f = s => s(1, 2); f S` reads `(1, 2)`). The lazy named reorder's lookup key
  reads the same position, or its keys never match the supply table's.
- **The deferred refusal is reported at the instantiation**, with the check's own
  kind, instead of through the apply's argument unify — the requirement it states
  is the instantiation's, and a non-struct actual is refused per call either way
  (`docs/language-spec.md` §Structs).

## The read path

A field read's type has exactly one source: the element **slot's type position**
of the container's type. A compound *value* holds bare element values, so the
element's own type is not stored beside them — and it need not be, because the
slot's type is the element's type: the instantiation unified them. Two measured
consequences of taking that seriously:

- **The read goes through the container's own type cell**, not through the
  pattern an open container was pinned to. A cell's class carries the actual type
  once it resolves, so the elements resolve with it: `lazy_infinite` reads
  `(1, 2, 3): tuple{Int, Int, Int}` where a read through the pattern gave
  `(raw 1, raw 2, raw 3): tuple{?a, ?b, ?c}`.
- **The requirement is the container's *kind*, never its type.** Naming the kind
  (`checker::tuple_kind_requirement` / `array_kind_requirement` for a decided
  container, a registered `TypeOperator::IsTupleType` / `IsArrayType` assert for an
  open one, spelled `AssertSpelling::TupleKind` / `ArrayKind`) leaves the
  container's own type cell free, so the read's elements resolve with the
  container; the named read's `StructKind` is the same shape. The element type is
  then read out of the container's own shape (`shape::field_type` at
  `ARRAY_TYPE_ELEMENT_SLOT` for an array, `shape::array_length` for its length),
  with a lazy `Index` chain when the shape is still open. Consistency is about the
  *slots*, not the kind, so the requirement is load-bearing: without it `l(0)` on
  an array and `s(0)` on a struct instance are accepted (measured), because an
  open container's positions are readable whatever it turns out to be.

The pattern is therefore built only where the requirement must be stated, so a
read of a decided tuple type allocates nothing.

## The array step

An array's element position is **one slot serving every element**, so the two
rules that need a count are stated once each:

- `check_array_type` (`array{T, n}`), `check_array_term`, and `check_index`'s pin
  build `[element slot, length]`; `low_type_of_array` and both printers read the
  slot's type position.
- The element walk pairs a **single** type element with *each* value element
  (`checker::element_pairs`), which is what a pin on an array's element means:
  `[1, 2] : array{5 : Int, 2}` is refused twice, once per element, and
  `[1 # 4, 2] : array{Int # 2, 2}` holds while `[1 # 2, 2] : array{Int # 4, 2}` is
  refused. A one-element tuple pairs the same way, so the rule needs no kind test.

## The table and set steps

A set's shape is its element **slot** alone and a table's is `[key slot, value
slot]`, built by the value forms (`check_set_term`, `check_table_term`, the
`table <==` read's pin, and the `@in` pin) **and** the type forms `set{E}` /
`table{K, V}` (`check_set_type`, `check_table_type`), which read the same slots
through the one element rule; `low_type_of`'s table arm and both printers read the
slots' type positions.

Both have a *type* spelling, so an element requirement is written for them like
any other compound's: `set(1 # 4, 2) : set{Int # 2}` holds, `set(1 # 2, 2) :
set{Int # 4}` is refused, and `set(1, 2) : set{1 : Int}` is refused on the second
member — the set's single slot pairs with every member through `element_pairs`'
single-element arm, the array's rule. `table{K, V}` pairs its key and value slots
with each entry's, so a pin or requirement inside either position is enforced
per entry.

## What is open

- **`tree-sitter-lichen/grammar.js` still models the retired forms.** Regenerating
  and validating it needs the tree-sitter CLI, which this checkout cannot run;
  `docs/notes/operators.md` §10 records the gap.

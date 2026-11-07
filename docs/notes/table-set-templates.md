# The table and set are templates, like the array

> Status: **proposal; the type-level step landed.** Points at
> `crates/lichen-highlevel/src/shape.rs` (`is_table_type_any`,
> `is_set_type_any`), `checker/templates.rs` (`is_compound_type`,
> `requirement_is_open`, `instantiate_requirement`), and
> `crates/lichen-language/tests/language/pipeline.rs` (the probe).

## 1. The array's model

The array is a **template value**: its value is `[element template, elements…]`,
slot `0` the element template and the elements after it
(`check_array_template`). Its type is the shared kind `[ArrayType]`, not the
element list, so a type value is the kind and the layout lives in the value. The
`=` requirement unifies the argument's **value** with the requirement's value —
both are the same array shape — and the per-use fresh cells come from the direct
template's instantiation (`instantiate_requirement`, whose array branch keeps
slot `0` and copies the elements).

## 2. What the table and set still do

Their **type** value is `[key type, value type]` for a table and
`[element type]` for a set, with the kind (`[TableType]`, `[SetType]`) in the
type slot; their **value** is `LowValue::Table`, a hash-sorted entry handle. The
type shape and the value shape are therefore different, which is the pre-template
form: an array's type and value share one shape.

Measured on the branch's first step (the type predicates and the compound set
landed; the value shapes did not change):

```lichen
F := ('x = table{_, Int}) => x
(F table(1 ==> 2), F table("a" ==> 2))
# expected ?, found table{Int, Int}
# expected ?, found raw[raw[@any, Int], raw[@any, Int]]

G := ('x = set{_}) => x
(G set(1), G set("a"))
# expected SetType, found set{Int}
# expected SetType, found set{string}
```

The requirement's value is the type shape; the argument's value is the entry
handle, so the `=` match has no common shape. The template machinery also had
no table/set branch before this step: `is_compound_type` covered only the
struct, array and tuple, and `is_table_type_any`/`is_set_type_any` did not
exist.

## 3. The target

- A table's value is `[key template, value template, entries…]`, a set's is
  `[element template, members…]`; the type is the shared kind, as the array's
  is. Then the `=` match, the reads, the element slots, the template
  instantiation and the clone see one shape.
- The table's lookup stays hash-sorted: the template slots are the types, the
  entries are the sorted pairs, and the read walks the entries.
- The set's member slot is the element template, so `set(a)` and `set(a, b)`
  share the type as `compound-element-slots` already states for the set.

## 4. Open questions

- The entry shape: `[key, value]` pairs as array items, or parallel arrays. The
  pair makes the read one item; the parallel arrays match the table's two slots.
- The table operators (`TableGet`, `TableMerge`, `TableWithout`, `TableCount`,
  `TableKey`) read the entry handle today; the template form moves the entries
  into the value, so every one of them changes with the codec and the GC.
- The persist/codec format: the handle is written as a table today; the
  template array would be an array layout.
- The compute ABI: the table values crossing the kernel boundary; the serve's
  table inputs.
- The type constructors (`check_table_type`, `check_set_type`) build the type
  shape; the target builds the value template and the kind.

## 5. Slices

1. **landed** — the type predicates, `is_compound_type`'s table/set members and
   the instantiation branch; the type layer recognizes the table/set as
   compounds.
2. The **set** value as the array-like template: the literal `set(...)`, the
   membership `@in`, the type kind, the reads, the codec and the GC.
3. The **table** value as the template: the entries, the hash-sorted lookup, the
   five operators, the type kind, the codec and the GC.
4. The persist/compute/serve paths.
5. The examples and the book blocks.

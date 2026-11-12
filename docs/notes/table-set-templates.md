# The table and set are templates, like the array

> Status: **the raw encoding landed; the suites are green.** Points at
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
F := 'x = table{_, Int}) -> x
(F table(1 => 2), F table("a" => 2))
# expected ?, found table{Int, Int}
# expected ?, found raw[raw[@any, Int], raw[@any, Int]]

G := 'x = set{_}) -> x
(G set(1), G set("a"))
# expected SetType, found set{Int}
# expected SetType, found set{string}
```

The requirement's value is the type shape; the argument's value is the entry
handle, so the `=` match has no common shape. The template machinery also had
no table/set branch before this step: `is_compound_type` covered only the
struct, array and tuple, and `is_table_type_any`/`is_set_type_any` did not
exist.

Two further measurements bound the design. Making the set value's **type** the
shared kind (as the array instance's type is) fixes the two-call `set{_}`
requirement, but it fails on a two-member set — `('x = set{_}) -> x; x set(1, 2)`
— with `expected SetType, found SetType`, because the requirement's value is the
one-element shape and the argument's value is the two-member array: the `=`
match compares the value lengths. It also refuses `('x = set{Int}) -> x;
x set("a")`, so the element is enforced only through that same value compare.
The array's template avoids both because its type carries the **length**
(`[_, 2]` has two holes). A set has no count — one type, any member count — so
the set's match must compare the **element slot** against each member, not the
array lengths, the rule `compound-element-slots.md` states for the set. The
`=` match is therefore the design point: the array's shape cannot be reused
verbatim. The set's kind change was measured and reverted; the type-level step
(the predicates, `is_compound_type`, the instantiation branch) is what landed.

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

## 6. The raw encoding, and what it broke

The user's encoding: a set value is `[element template, raw set]`; a table
value is `[key template, value template, raw table]`; the **type** value carries
the same slots with `@any` in the raw position, so the template match compares
the template slots and never the payload — the set's member count and the
table's entry count stay out of the type. This landed in the checker's
construction, the table read (`TableGet` over the raw), the table operators (the
raw operands, the wrapped result value), the prefix reads, and
`set::members` (the raw set's items).

Measured after the change: the compiler passes; the language suite is **red with
17 failures**. Most are not table programs — package loading refuses with
`does not satisfy {Float, Type, @any, @any, @any}`, and the recursion, closure,
merge-carry and persist areas fail — so the first suspect is the shared
`@any`/type-shape change reaching the package signatures, not the table reads.
`pipeline::a_table_removal_drops_one_entry_and_names_a_miss` also fails. The
branch is a WIP; the next step is that diagnosis before any merge.

## 7. The core's set is the root of the red suite

Bisect: `c931d4c7` (the type predicates only) is green at 283; `e4bead9e` (the
construction) is **49 failed**; `2430365e` (construction plus the read/operator
consumers) is **17 failed**. The construction alone explains the wide failures.

`crates/lichen-language/src/core.lichen` is eleven lines and starts with

```lichen
Num := set(Int, Float)
in_num := 't -> t @in Num; let Pair := (@any ! in_num, @any ! in_num)
```

so `Num` is a **set**, `in_num` is its `@in` refinement and `Pair` carries it.
The set's type moving from the element shape to the shared kind changed the
core package's exported requirement — the importer renders it
`{Float, Type, @any, @any, @any}` — and every program that imports the prelude
fails to resolve it. The stores are fresh in-memory instances
(`PackageStore::new()` per run), so this is the signature, not a stale cache.

The next step is the core's `Num`/`in_num`/`Pair` signature under the new
encoding: either the importer's expectation follows the kind, or the set's type
value keeps `[element, @any]` for the signature while the `=` match reads the
template slots. The table removal test is the one table failure left after the
consumers.

## 8. The set is the array's layout

The red suite was the set's members read: a set value is
`[element template, raw set]` and the raw set is an **array instance** — the
element template, then the members — so `set::members` reads the array's own
elements after the template, exactly the array's convention. A plain array —
the arithmetic refinement's `[int, float]` domain — has no template slot and is
its own members. With that, the core's `Num`/`in_num`/`Pair` check again and the
package, recursion, closure, merge-carry and block areas pass.

The table removal's right operand is its **key**, not a second table; it was
being indexed at slot 2 like a table and the removal missed. Measured: the
language suite is **283/283**, the highlevel **136/136**, and the compute
**247/247 (1 ignored)** plus its 14 unit tests.

The set/table encoding the user stated is in place: a set
`[element template, raw set]`, a table `[key template, value template, raw
table]`, with the type's raw position the wildcard.

## 9. The spec's type values

`docs/language-spec.md` (the maintainer's section split, in flight) states the
shapes the implementation now uses: `set{t}` is `[t]`, `table{kt, vt}` is
`[kt, vt]`, a set value is `[t, @raw Set(members…)]` and a table value
`[kt, vt, @raw Table(…)]`. The type values previously carried an extra `@any`
raw slot and the set's raw was an array instance; both were removed, so the
type value and the instance's template positions agree.

Measured after the change: language **283/283**, highlevel **136/136**, compute
**247/247 (1 ignored)** plus its 14 unit tests.

Still not the spec's surface, and the next slice: the instances take their type
explicitly (`table(t)(k => v, …)`, `set(t)(e, …)`), the arrows are `->` for the
lambda and `=>` for the entry — the spelling the implementation now uses — and
an instance's type is still the shared kind rather than the type value the
spec's `set{t}`/`table{kt, vt}` names.

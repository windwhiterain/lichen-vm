# Named tensors: a buffer, a named shape, and a named pitch

> Status: **proposal. The value form is measured; the language's field-type instantiation is
> the blocker; nothing outside the probe is implemented.**
>
> The vision, recorded as given: **a tensor is wrapped by the language layer, written
> physically, and every dimension has a name.**
>
> Points at: `crates/lichen-compute/tests/lichen_compute/zz_tensor_probe.rs` (the
> measurement), `crates/lichen-compute/src/compute.lichen` (`Buf`, `read`, `write`,
> `parallel`), `crates/lichen-compute/src/compute.rs` (`roles_from_value`, `walk_role`,
> `scalar_leaf_classes`), `crates/lichen-serve/src/qwen.rs` (the host's flat tensors), and
> `crates/lichen-serve/src/model.rs` (`MATVEC_STEP`, the host path that already carries two
> buffers of different lengths).
>
> Companions: [compute-buffer-wrapper](compute-buffer-wrapper.md) (the `Buf` ABI),
> [value-templates](value-templates.md) ("layout lives in a value"; the template cell rules),
> [compute-kernel-struct](compute-kernel-struct.md) (the lazy `Index` field read),
> [graph-fusion](graph-fusion.md) §10 (the tensor-operator decision),
> [two-dimensional-launch](two-dimensional-launch.md) (the second extent),
> [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) §5 (views declined),
> [qwen-inference](qwen-inference.md) (the model's tensors today).

## 1. The model

A tensor is a **value**: a buffer, a named shape, a named pitch, and an origin.

```lichen
Pair   := {.row @any : Int, .column @any : Int}
Tensor := {
  .buffer (compute.Buf (compute.BufParam{.type Int, .length _})),
  .shape  Pair,
  .stride Pair,
  .origin @any : Int,
}
```

- `.shape`'s fields are the **dimension names**; their values are the **extents**. A struct
  field is the language's only O(1), nominal, checked naming device.
- `.stride`'s fields are the same names; their values are the pitches, **in elements**,
  written by the author. This is the physical order.
- `.origin` is the element the view starts at; a dense tensor has `0`.
- There is **no `.element` field**: the element class is the `Buf`'s `.type`.

The tensor *is* its address function:

```
address(t, i) = t.origin + sum over d of i.d * t.stride.d
```

A read, a write, a view and a tile are that one function. Nothing above the body knows how
many dimensions exist: the launch is still one flat extent, as
[two-dimensional-launch](two-dimensional-launch.md) measured.

**Why a value.** The tree's ruling is that a compound's layout lives in a value
([value-templates](value-templates.md)); a type-level shape would be a second mechanism beside
it. The dimensions are values, so an extent may be a literal (a compile-time check) or a runtime
leaf (a batch's rows), and the same tensor serves both.

## 2. The spelling

```lichen
# the physical pitch, stated by the author:
# w is `out` rows of `in` elements, row-major, in a buffer of out*in
w := (Tensor ...){.buffer wb, .shape Pair{.row 2, .column 3}, .stride Pair{.row 3, .column 1}, .origin 0}

# the same bytes, read column-major: no transpose, no copy
w_t := (Tensor ...){.buffer wb, .shape Pair{.row 2, .column 3}, .stride Pair{.row 1, .column 2}, .origin 0}

# a named index is a struct value; the address is one expression
at2 := 't -> 'i -> t.origin + i.row * t.stride.row + i.column * t.stride.column
```

A dense constructor derives the pitch from the shape (last dimension fastest), so the common
case writes no strides.

The physical spelling is the point: a column-major tensor over a row-major run of `1..6`
answers `[1, 3, 5, 2, 4, 6]` **without reading a second copy**, where the tree's
`Tensors::Narrowed`/`by_column` path transposes a layer at load to make the kernel read
row-major.

## 3. What is measured

Every row is a program in the probe on the `cpu` backend.

| program | answer | settles |
|---|---|---|
| one bare-template tensor, `.buffer`/`.shape`/`.stride`/`.origin` read | `[1, 3, 5, 2, 4, 6]` | the walk finds `.buffer` as the input and the numbers as scalar leaves; a named address reads the file at the stride it was given; `.origin` is fine |
| two bare-template tensors of the **same fixed** type, buffers read | `[2, 4, 6]` | a second tensor field is not itself the block |
| two descriptor fields with nested `Pair`s, each read | `[4, 11]` | two uses of a nested struct type are readable |
| a tensor under `.out` (the one-value form) | **refused**: the codomain gives its numbers the scalar role, and no output states a decided class | an output tensor is the one shape the ABI has no place for |
| the output split into a `Buf` and a descriptor | `[0, 1, 2, 3, 4, 5]` | an output is fine when the buffer is a plain field |
| a per-use constructor `(Tensor 6)` as a field type | `[1, 4, 0, 3, 6, 0]` — wrong cells | a constructor applied in a field-type position resolves to the `Struct` marker, not to its fields |
| two constructor-typed fields | **refused**: "no field with this name in the struct type Struct" | the same, compounded |
| two buffers of different lengths, built by the language, `.length _` or fixed | **refused** | the parameter template's field types are shared, so the `.length` cell cannot take two lengths |

The last row is not the ABI's limit. The model's own `MATVEC_STEP` carries two buffers of
different lengths (a weight and a vector) in one parameter, and it runs: the host binds the
open cells with `RunArgument`s at run time, so the checker never unifies them. **The host path
already carries tensors; a language program that builds both buffers itself does not yet.**

## 4. The blocker: a parameter field's type is shared, not per use

The rule that bites is [value-templates](value-templates.md)'s: a direct template's open
**value** slots are fresh per use, while its **types** are shared. A parameter field's type is
a type, so two tensors written from one template share one `.length _` cell and one `Pair`
type instance. That is why:

- two tensors of different lengths cannot share one template (the `.length` cell takes two
  numbers);
- a per-use **constructor** gives fresh cells, but a field whose type is a constructor
  application is read as the `Struct` marker — the checker does not instantiate the
  application to read the field's names (the same lazy-`Index` boundary
  [compute-kernel-struct](compute-kernel-struct.md) describes for a generic wrapper).

**The repair** is one checker capability: instantiate a **parameter field's type per field**,
so `(Tensor 6)` and `(Tensor 3)` are distinct struct layouts and two `Buf`s of different
lengths get their own cells. That is the first compiler slice; it makes the one-value tensor
work, and it also removes the constructor/template split.

Until it lands, three spellings exist:

1. **One tensor per parameter.** Works today, but a matvec needs two.
2. **The split**: buffers as plain fields, descriptors as a bare template with nested `Pair`s.
   Two descriptors read (measured); the matvec over two buffers is still refused for the
   length reason, so this does not unblock the language path either.
3. **The host path**: the serve crate passes `RunArgument`s, so the model's two-buffer
   parameters work. This is the path the model uses today.

So the model and the host are ready; the language front end needs the per-use field type.

## 5. The operators

The operator vocabulary is a **library**, not a compiler operator set. Per operator: a named
tensor template and a kernel body over the names, with the claims the fusion engine already
reads.

```lichen
# one lane per `out` element; the fold walks `in` by the tensor's own pitch
mv := ('k = Par) -> {
  i := compute.range k.n
  wb := k.in.w.buffer
  wsr := k.in.w.stride.row
  wsc := k.in.w.stride.column
  xb := k.in.x.buffer
  xsc := k.in.x.stride.column
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1,
    s(1) + read wb at (i * wsr + (s(0) - 1) * wsc) * read xb at ((s(0) - 1) * xsc))
  ...
}
```

- **The fold count is a tensor leaf** (`w.shape.column`), so an operator's shape is a
  parameter, not a literal — the property `matmul` uses today.
- **The fold spelling follows the model's**: the parameter's fields are read in the body. The
  probe's hoisted form refused; the model's `MATVEC_STEP` reads `k.in.w` inside the fold and
  runs.
- **Three constraints meet here**: the per-use field type (§4), the leaf order (§5.1), and the
  address arithmetic (the body's).

The decision to keep: [graph-fusion](graph-fusion.md) §10 rejected a tensor-operator
vocabulary **in the engine**. `ml.matvec` is lichen code with `~ "claim:Reduction(...)"`
marks; the engine still matches algebra, not the operator's name, and a pack still steers the
implementation. A compiler-level `matmul`/`softmax` operator — fusion by name, shape rules the
compiler owns — would reopen that decision and is a separate ruling.

### 5.1 The leaf order is a hash order

A struct's field order is its name table's order, a deep content hash — not the declaration
order. The model measured `{.a, .b, .c}` answering `321`
([qwen-inference](qwen-inference.md)). The scalar leaves of a tensor parameter are therefore
ordered by a hash; the author cannot read the order off the source, and the host's argument
list must match it silently.

The durable fix is **names in `KernelRoles`**: the role table already carries a path per leaf,
and the artifact could carry the field name beside it. The host would build the arguments by
name and a mismatch would be refused by name. This is an artifact change (the encoding is a
compatibility contract) and it is what makes "every dimension has a name" true across the ABI,
not only in the source.

### 5.2 Selecting a dimension without naming it: a closure, not a type variable

A contraction must say "this dim of A with that dim of B" without knowing either name. A
struct field is not a value, so an operator cannot take `.in` as an argument, and it cannot be
written "for any field" (there is no row polymorphism). The selector is an ordinary
**closure**, and the names live only at the call site:

```lichen
# the selector: an axis ordinal to the dim it names
sel := 'i -> if i == 0
  then Dim{.extent x.shape.row, .stride x.stride.row}
  else Dim{.extent x.shape.column, .stride x.stride.column}

# the operator: two tensors, their selectors, and the two axes to contract
contract x sx y sy ax ay
```

The operator applies `sx`/`sy` on the host, **before the kernel is built**. The kernel cannot
call a closure: a kernel body is compiled and the ABI has no function arguments, so the
selector's results — extents, pitches, the plan — must become data (scalar leaves or a plan
buffer) before the launch.

Measured:

- a selector that **captures** the tensor at the call site reads its named fields and answers
  (`(sel 0)(1)` is `3`);
- a selector whose parameter is only **typed** (`('d = Dims) -> ...`) is refused: "this field
  read cannot be checked: the container's type is not decided here" — the body is checked at
  definition, before the parameter's layout is bound;
- a higher-order consumer needs the selector's function type stated, or its application is
  "not decided".

So the closure form works **today**, with the caller writing one capturing selector per
tensor. What it asks of the language is one checker capability: **defer a field read on a
parameter whose struct type is known until the application**, so a library can provide
`dimsOf` once for every tensor instead of per call site. That is the same lazy-`Index` rule
[compute-kernel-struct](compute-kernel-struct.md) already applies to a wrapper's `.I`/`.O`.

What the closure approach deliberately does **not** need: row polymorphism, first-class field
names, or a kernel that takes a function.

### 5.3 The contraction algorithm

With dims as data, a contraction is ordinary bookkeeping:

1. check `extent(A[ax]) = extent(B[by])`, by value (a library refusal, not a type);
2. the output dims are the remaining ones; the caller's plan fixes their order and resolves a
   name collision (`a.row` beside `b.row`);
3. each operand's dim maps to an output axis or to the contracted axis;
4. the address is `origin + sum over d of coord.d * stride.d`; a lane's flat index decodes to
   the output coords, and the contracted extent is the fold;
5. the kernel is either a **plan-specialized** body (a flat launch with the selected
   extents/pitches as leaves — the fast path for matvec/matmul/attention) or one **generic**
   body over a plan buffer (any rank, dynamic folds, the reference path).

### 5.4 The result type: a table in the type, evaluated at the use

**Correction first, because an earlier draft of this note had it wrong.** The language does not
separate compile time from run time: `TableGet` is evaluated whenever its table and key are
determined, and the checker's deep pass reaches it while checking (`evaluation.rs`'s
`TableGet` arm; a miss is `EvalError::TableMiss`, an error rather than "undecided"). The
boundary is **determined vs undetermined**, not compile vs run.

That makes the table route viable, with one placement decision: **the shape table must be a
type-level value, not a field of a runtime parameter.** A runtime parameter's shape is an
undetermined cell at the definition, so its keys are not known until the call; a type argument
is concrete at the use, so the checker evaluates the reads there.

Why tables fit the generic case better than struct fields:

- `check_table_find` needs only the container's **type** — `table{K, V}` — which an annotated
  parameter already states; a generic function over shape tables therefore compiles, and its
  key lookups stay undetermined until the use.
- A **struct field** read needs the layout at definition; a parameter that is only typed is
  refused ("this field read cannot be checked: the container's type is not decided"), which is
  what the value-level selector probe measured. The table route avoids that eager failure.

So a contraction is a **type-level function over shape tables**:

```lichen
# shape is a type-level table; the extents are values (a batch's rows included)
A : Tensor (table{"r0": 2048, "c0": 2048}) Float
B : Tensor (table{"r1": 2048, "c1": 2048}) Float

C = contract A B (`c0, `r1)
# C's type : Tensor (contract_shape A.shape B.shape) Float
#          = Tensor (table{"r0": 2048, "c1": 2048}) Float
```

`contract_shape` is an ordinary lichen function over tables (drop the contracted labels, merge
the rest, keep the extents); per-application specialization gives it the concrete tables at the
use, and the deep pass evaluates the `TableGet`s there. A contracted label that the operands do
not have is a `TableMiss` at check time; the result's key set is known, so completion can list
`r0`/`c1`. No record type and no first-class label is required: **a record is just a table whose
key set is known.**

The extents may be cells while the keys are static: a dynamic extent (the batch's rows) is an
undetermined value in the table, the keys are still determined, and the kernel receives the
extents as leaves or a plan buffer. Static extents can be inlined.

The label/record surface of §5.3 remains an option — a spelling — but it is not a necessity if
a table value can stand in a type position.

### 5.5 What remains for the table route

1. **The tensor type surface**: `Tensor shape element` with a table value as the type argument;
   the parser and the type printer must accept and render it. The type printer currently spells
   `table{K, V}`, not a specific key set; completion needs the type's table value, not just its
   kind.
2. **The type-level shape functions**: `contract_shape` and the selectors as table operations
   (drop/merge/relabel); ordinary lichen, evaluated by the deep pass at the use.
3. **Dynamic extents**: a table with static keys and value extents; the kernel gets the extents
   as leaves (or a plan buffer) because the device cannot read a type.
4. **Completion**: the LSP reads the type argument's table keys. The type is a value; the
   question is the tooling, not the semantics.
5. **The value-level selector** (a closure over a runtime tensor) is the limited spelling: its
   field read is eager at definition. The type-level table makes that closure unnecessary; if
   both are wanted, the deferred field read is a separate capability.

Measured, in the current table spelling:

- a generic function over a **shape table** compiles (`pick := ('s : (table{Int, Int})) -> s <== 1`), where the struct-field selector was refused — a table read needs only the container's *type*;
- the determined read is evaluated at the use: `pick (table(1 => 10, 2 => 20))` answers `10`;
- a miss is a check-time refusal: `pick (table(2 => 20))` refuses with "table lookup missed — no entry for key 1";
- a call-site capture with string labels answers (`shape := table("r0" => 2, "c0" => 3); shape <== "r0"` is `2`).

**Landed for this slice**: `@table_merge` (the union, the right entry winning a duplicate key),
`@table_without` (a table without one key; a miss is a named `TableRemoveMiss`), and the
reflection pair `@table_count` / `@table_key` (the entries by ordinal; an out-of-range ordinal
is a named `TableKeyOutOfRange`). They are structural low operators, emitted by the checker and
dispatched by the VM, so the determined evaluation reaches them like any other operator. The
contraction's result shape is expressible as `without (without (merge A B) c0) r1`, and the
entries are enumerable for the plan.

Measured with the primitives:

- `table(1 => 10, 2 => 20) @table_merge table(2 => 21, 3 => 30)` answers `10`, `21` and
  `30` for the keys `1`, `2`, `3` — the duplicate key takes the right entry;
- `table("a" => 10, "b" => 20) @table_without "a"` answers `20` for `"b"`;
- `table(1 => 10) @table_without 7` refuses at check time with "table removal missed — no
  entry for key 7".

Still open:

- the result shape's keys visible to the printer/completion;
- a dynamic extent (a cell) in the table beside static keys: keys checked, extent left to the
  kernel leaves.

### 5.6 A less ugly spelling for shapes

The semantics can stay tables; the spelling should not. The current spellings are
`table("r0" => 2048)`, `s <== "r0"` and `table{string, Int}`. A static-key table is a
**record**: structural, named, and it is what a shape is. Proposal:

| use | now | proposed |
|---|---|---|
| shape literal | `table("r0" => 2048, "c0" => 2048)` | `record{ .r0 2048, .c0 2048 }` |
| shape access | `s <== "r0"` | `s.r0` |
| shape type | `table{string, Int}` | `record{ .r0 Int, .c0 Int }` |
| tensor type | `Tensor (table(...)) Float` | `Tensor record{ .r0 2048, .c0 2048 } Float` |
| dynamic literal | `table(k => v, ...)` | `table((k, v), ...)` |
| dynamic read | `t <== k` | `t[k]` |
| table type | `table{K, V}` | `table{K, V}` (unchanged) |

- **`record{ .name value, ... }`** uses the struct field syntax, so a dimension reads the way
  the author writes it, `.r0 2048`. The `record` keyword is what makes it **structural** (key
  set and values, content-hash identity) rather than the nominal struct `{ .r0 2048 }`.
- **`s.r0`** is the struct access spelling; the checker resolves by the type — a record reads a
  key, a struct reads a field. The lookup is O(1) when the key is static.
- **`t[k]`** for the dynamic table read shares the array access spelling; the checker resolves
  by the type. This retires `t <== k` (and `k => v` for the literal, which becomes a pair).
- The **library ops** (`merge`, `drop`, `rename`) take the key as a *value* — a string for the
  table API (`drop s "c0"`). Their key is checked by the determined evaluation, so a typo is a
  check-time `TableMiss`, not a silent miss. A label literal (`#c0`) is only needed if the ops
  must spell the key as syntax rather than a string; it is not needed for access.
- The **bridge**: a `record{...}` literal lowers to a table with the labels as string keys;
  `s.r0` lowers to the table get; the record *type* is the refined key set, which is what
  completion reads.

The single new parser form is `record{...}`; the rest is spelling over the existing table value
(and `t[k]` is the table read under the array syntax). The record is the authoring view, the
table the library's view, and the determined evaluation makes the label checks static where the
shape is determined.

## 6. The host and lichen-serve

The host mirrors the value:

```rust
pub struct Tensor {
    pub dims: Vec<Dimension>,   // (name, extent, stride), in the source's spelling
    pub origin: usize,
    pub class: ScalarClass,
    pub payload: Payload,       // shared bytes or a resident
}
```

- `Qwen`'s `SHAPES`, `COLUMN_MAJOR`, `by_column` and `split_layer` become descriptors;
  `Tensors::Wide/Narrowed` stays as the storage question.
- **`by_column` disappears**: a column-major tensor is a stride, not a transpose. The load
  stops paying a second pass over a layer's projections.
- `embed_row` becomes a row view (origin advanced), not a copy.
- `place` receives a class and an extent from the descriptor rather than raw bytes plus a
  class.
- The layer's eleven tensors get names; a parameter `{.out, .in}` and the host's descriptor
  say the same names.

The host passes each extent and pitch as a `RunArgument` (in role order until 5.1 lands). That
is a handful of integers beside 96 MB of weights: the value form's leaf cost is not a reason
to prefer a type-carried shape.

## 7. Slices

1. **The checker's per-use field type** (§4) and **the deferred field read** (§5.2): a
   parameter field whose type is a constructor application, or a template with open cells, gets
   its own cells and a readable layout; and a field read on a parameter with a known struct type
   waits for the application, so `dimsOf` can be generic. The probe's one-value tensor then
   works for a matvec; this is the gating slice.
2. **The tensor library**: `Tensor`, `dense2`, `view2`, `at2`, `read2`/`write2`, and the
   selector closures; the probe's programs as tests.
3. **Output tensors**: ruling 8.4; flat outputs first.
4. **Names in the roles** (§5.1): the artifact carries each leaf's field name; the host builds
   by name.
5. **The operator library**: matvec, norm, softmax over named tensors, each with its claims;
   `contract` over the selector closures, plan-specialized first and one generic plan-buffer
   body as the reference; a measurement that a tensor program's fusion report equals the
   hand-written one's.
6. **The host descriptor and the serve path**: `Qwen` holds tensors; `by_column` removed; a
   token and a load measured before/after.
7. **Views**: ruling 8.2; transpose and tile, the aliasing contract, the load measured.

## 8. Open rulings

### 8.1 Value-carried or type-carried shape

| | value-carried (this proposal) | type-carried |
|---|---|---|
| ABI | unchanged; extents/pitches are leaves | tensor-aware walk; metadata skipped |
| dynamic extents | work | need a second mechanism |
| output tensors | refused today | possible if the walk skips metadata |
| static checks | where values are decided | full |
| cost | a few integers a launch | a checker/emitter change and two mechanisms |

Recommendation: value-carried.

### 8.2 Views and strides

| | allow | dense only |
|---|---|---|
| transpose/tile | a stride, no copy | a transpose pass at load |
| aliasing | the program's contract (as `write` already is) | none |
| roadmap | reverses [gpu-algorithm-roadmap](gpu-algorithm-roadmap.md) §5 | keeps it |
| risk | a write through a view aliases silently | the load pays the transpose |

Recommendation: allow; "physical writing" **is** the stride, and the contract is the one
`write` already states — a lane's write names its own slot, and the program owns what it shares.

### 8.3 Library or compiler vocabulary

| | library | compiler operators |
|---|---|---|
| fusion | marks, as today | by operator name; reverses graph-fusion §10 |
| shape checks | the values the language can decide | the compiler owns them |
| distribution | a package | an IR vocabulary |
| cost | per-application compilation | new IR, passes, a second matching rule |

Recommendation: library.

### 8.4 Output tensors

| | flat outputs | tensor-aware walk | result structure |
|---|---|---|---|
| first slice | today | metadata classification | a run/result change |
| dynamic shape | n/a | still needs leaves | yes |
| signature | asymmetric | symmetric | symmetric |

Recommendation: flat outputs first, or the split spelling (measured working).

### 8.5 The gating slice: the checker fix or a spelling

| | per-use field type (checker) | split spelling + per-use constructor |
|---|---|---|
| one-value tensor | works | the buffer is a separate field |
| matvec | works | still blocked by the `.length` cell |
| cost | a checker change with the lazy-`Index` machinery | no compiler change, but no language matvec |

Recommendation: the checker fix; it is the difference between a language tensor and a host
descriptor.

## 9. What is measured, and what is read

The probes behind this section used `compute.buffer`, which the buffer-declarations design
removed; the forms stand and the live input path is the fill kernel (§5.5, §11).

| claim | how |
|---|---|
| a tensor under `.in` is a buffer plus scalar leaves | measured |
| a column-major stride reads the file without a transpose | measured: `[1, 3, 5, 2, 4, 6]` |
| `.origin` is a readable field | measured |
| a second tensor field is not the block | measured: `[2, 4, 6]`, `[4, 11]` |
| an output tensor in the one-value form is refused | measured |
| the split output works | measured: `[0, 1, 2, 3, 4, 5]` |
| a constructor in a field-type position is not read as its struct | measured: `[1, 4, 0, 3, 6, 0]` |
| two buffers of different lengths from a language program are refused | measured |
| a selector closure **capturing** the tensor reads its named fields | measured: `(sel 0)(1)` is `3` |
| a selector over an unbound parameter is refused | measured: "the container's type is not decided here" |
| the host path carries two buffers of different lengths | read/used: `MATVEC_STEP` |
| the leaf order is the name table's (hash) order | read: `walk_role` + `qwen-inference`'s `321` |
| `by_column` is a load-time transpose | read: `qwen.rs`'s `held_layer` |

## 10. Open measurements

- The per-use field type landed: the probe's matvec answers `[6, 15]` on `cpu` and the device,
  with a permuted stride answering a different list.
- The cost of the shape/pitch leaves: a token with and without them (expected: nothing beside
  a 96 MB weight).
- The checker's handling of a shared shape binding across two operands.
- The load time of a layer with `by_column` and with a column-major tensor.
- The operator library's per-application compilation cost on the graph path.
- The closure-selector spelling on a device kernel: the plan-specialized contraction answers a
  named contraction, and a permuted stride answers a different result rather than the same one.
- The generic contraction kernel over a plan buffer against the plan-specialized body, on the
  same contraction — the price of rank-agnosticism, measured.
- The checker deferral (§5.2): `dimsOf` written once for two tensor types, and the refusal it
  replaces.

## 11. What a contract kernel still needs

Measured now: the result shape is `merge` then `without`, and its **type position** works —
`[shape value] (2, 4)` and `[shape type] 2`, where a tensor-typed value carries the computed
shape table and reads a label back through it at check time. The contract's result type is not
the gap.

**The two blockers this note named are gone.**

- **The loop-nest rule is fixed, and measured.** An inner `@loop` nest's result now feeds the
  enclosing step: `zz_nested_loop_probe` answers `[9.0, 9.0, 9.0]`. Three changes in
  `compute/body.rs`: a block's terminator is set where the walk ended, so a nest's entry branch
  is not overwritten; `value_inner` restores the cursor only when its own lowering did not move
  it, so a nest's merge stands; and a merged nest becomes the enclosing loop's current block.
  The full compute suite passes (246 tests).
- **A table's entries are enumerable.** `@table_count t` and `@table_key t i` (an out-of-range
  ordinal refuses by name) give the keys by ordinal in the table's stored order, so a library
  can build the plan and the output descriptor from a shape table. `merge`, `without`, `count`
  and `key` are the whole reflection surface.

What remains:

1. **The dynamic-rank body is not yet writable.** `zz_dynamic_contract_probe` builds it and is
   still refused, but the plan now rides the fold's `Buf` state and **passes the kernel's role
   walk**. The loop conversion admits a loop-invariant `Buf` (`01e72b04`); the compute lowering
   gives each loop instance its own value frame and re-seeds a block's state bindings when the
   nest enters it (`f1699103`, `4e55fca7`); and a state read resolves value -> parameter -> the
   call's argument (`110b6163`, `6ffce226`). The remaining refusal is the enclosing `dot` fold:
   a `@loop` lambda at a kernel body's top level is applied through a **static/frozen** callee,
   so `apply`'s frozen branch inlines it and its body hits a block-0 dominance refusal; the
   freeze copies no `@loop` mark onto `StaticFunction` at the time it runs. The infrastructure
   for a frozen conversion is in place (`StaticFunction.looping`,
   `StaticModule.dynamic_functions`, `Module::static_dynamic_function`, and the `apply`
   branch), but the frozen functions carry no mark. Wrapping the dot in a lambda template makes
   the callee dynamic and converts the dot; the next refusal is then the frozen residual's
   index placement: the inlined callee's parameter reads do not place against the caller's
   domain (`parameter_path`'s base never matches the clone's parameter node).

   Measured with module pointers: the `@loop` marks (`mark_looping`) land on the same module
   the compute lowering reads (one pointer, 33 functions), while the closure freeze that
   produced the callee's static ref ran **before** them — the module was frozen from a build
   that had not yet normalized the kernel body's lambdas. The freeze is the package build's
   `Registry::freeze_mapped` (`crates/lichen-language/src/package.rs:739`), which runs before
   the kernel body's lazy lowering. So the static copy's `looping` is a pre-normalization
   snapshot, not a static/semantic limit; a freeze taken after the kernel body's normalization
   carries the mark. The fix direction is therefore two-part: normalize
   (the kernel body's lambdas) before the freeze that produces the callee's static ref, and
   read the frozen function through one graph interface shared with the dynamic module
   (`loop_conversion` over both) instead of a cross-module `FunctionId` bridge.
2. **The plan's order** is the table's stored (hash) order. It is deterministic and the strides
   are explicit, so it is a valid output order; the library and the host must both use it.
3. **The input path is the fill kernel or the serve's host `RunArgument`s.** The
   array-to-buffer conversion (`compute.buffer`) is removed; a host-level `compute.buf` is
   kernel-only. The fill kernel is the language-level path; the serve's contraction is
   host-driven.
4. **The output** is a flat `Buf`; the caller wraps it with the result shape (the split form
   measured working).
5. **The contracted extents' equality** is a value comparison: check-time when literal, the
   library or host otherwise.

**Measured: the 2-D contraction runs.** `A{r0,c0}` (2×6, `A[r][t] = 6r+t+1`) contracts with
`B{r1,c1}` (6×2, `B[t][c] = t+1+c`) over `c0`/`r1`; `C` is 2×2 and the probe answers
`[91, 112, 217, 274]`, with `C`'s shape `{r0: 2, c1: 2}` from `merge`/`without`
(`zz_contract_probe`). The kernel is plan-specialized: the extent, the contraction and both
pitches are scalar leaves in the proven `.meta`/`.in` parameter, and the address unfolds inside
the one fold. Its input buffers come from a fill kernel. The direct spelling the probes used is **gone with `compute.buffer`**: the module has `buf`
and `collect` only, so `compute.buffer` is a missing field; a host-level `compute.buf` is
kernel-only. The JIT's metadata now comes from `.type`/`.length`, and the native `$buf_type`
is removed entirely (`Buf`'s `.native` is a plain `@any` slot). The serve example
`contract_direct.rs` hands one prepared step two host buffers of different lengths directly
and answers `[5, 8, 11, 11, 18, 25, 17, 28, 39, 23, 38, 53]`. The two spelling lessons are
the spec's: the step's domain is `'s = GArg ->` (the struct signature's `=`), not the `: `
annotation, and the graph's ABI wants the buffers under `.in`.

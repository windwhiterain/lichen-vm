# Direct @any templates

> Status: **current for the source language; the compute migration is
> incomplete.**
> Points at: `crates/lichen-language/src/compile.rs` (no implicit template
> application), `crates/lichen-highlevel/src/checker/templates.rs` (requirement
> instantiation), `crates/lichen-highlevel/src/checker` (value construction and
> reads), `crates/lichen-lowlevel/src/apply.rs`, `function.rs`, and
> `static_module/apply.rs` (the function apply's scope and its frozen mirror),
> and `crates/lichen-compute/src/compute.rs` / `compute/body.rs` (the compute
> surface).

Compound layout lives in a value, not in a shape-bearing type. `Tuple`, `Array`,
and `Struct` are the shared layout types; a **template is a direct compound value**
whose open slots are `@any`. The source of truth is [the language
spec](../language-spec.md); this note records the migration decisions, the
reasons for them, and the measurements that now hold.

## Confirmed rulings

- A direct template is a value, for example `S := {.a @any, .b @any}`,
  `T := [@any : Int, 3]`, or `P := (@any : Int, @any : Int)`. It is not a
  function and is never applied.
- `=` is the only template match. `<:` is retired in source; the compiler
  lowers it as a plain unify and the migration rewrites it to `=`.
- A direct template is instantiated **per use**: its open value slots become
  fresh empty cells, while its types, attributes, name table and nominal id are
  shared. The actual fills the fresh slots, so the template itself is never
  specialized.
- A parameterized constructor remains a function: `F := I => {.in I}`. It is
  applied explicitly everywhere it is used: `x = (F I)`, `(F _){.in 1}`,
  `(F _)[…]`. A zero-parameter constructor is direct unless its body is a bare
  hole, a function, or another non-compound.
- Ordinary `:` annotation is unchanged. It is not a template match.
- There is no source-level `@clone`. Freshness for a function comes from its
  application; freshness for a direct template comes from requirement
  instantiation. Function scope, closed-cell sharing, and the per-application
  remap remain the ownership model; graph reachability is not.
- A struct instance `S{.x 1}` uses the template value `S` directly and builds
  the result from the supplied field values. An array instance `T[x, y]` uses
  `T` directly, checks its length, and unifies every element with the same
  element requirement. A comma-free `a[i]` is still an array read; `T[x,]` is
  the one-element instance and `T[]` the zero-element instance.
- Scalar wildcard fields use `@any : Int` or `@any : I`; bare `I` stores a type
  value, not an arbitrary value of type `I`.
- **A binary operator's operands stay the tuple ABI.** The operand group is
  `operator (left, right)`, not the array form `[left, right]`: the array
  requirement's `[requirement, length]` is a different two-slot shape, and a
  binary operator that borrowed its spelling would be ambiguous with it. The
  spec's binary sugar is `operator (left, right)`.
- The thin arrow `->` is removed. A function constraint uses plain `=` with a
  signature; a function template is applied explicitly. Unification compares the
  signature and preserves the actual callable body; it never replaces that body
  with the template's body.
- A call-result template is used with plain `=`. For example, a native wrapper
  states `r : type_of (k.O); r = k.O`.

## The direct signature

A signature is stated with plain `=`, and the unification keeps the actual's own
body:

```text
I := _x => _
f = (i = (I _) => _)
O := f (I _)
r = O
```

`f = (i = (I _) => _)` says that `f`'s parameter takes the requirement
applied from `I`, and that `f`'s result is a hole. `O := f (I _)` applies `f` to
the same requirement, so `O` is the result the signature describes. The result
is then reused with plain `=`: `r = O` for a local result and `r = k.O` for a
result that came out of a kernel struct.

## Implementation

The source-to-IR step performs no implicit application: `<:` lowers exactly like
`=`, and an instance uses its callee value directly. A template that is a
function is applied at the call site with the ordinary apply syntax.

`check_unify_expr` asks whether the expected value has an open value slot. If it
does, `instantiate_requirement` copies the value graph, replacing every open
value slot with a fresh cell and sharing every type, attribute, name table and
nominal id. The actual is then unified with that fresh copy. This is what keeps
one written template reusable across instances without a function wrapper.

A function's application still uses the apply's own clone. `ApplyCtx` clones the
callee's template, shares the cells the template captured, mints a fresh closure
per call, and rebuilds the class topology with `apply::regroup_clones` /
`unify_clone_groups`. `Node::origin` records where a clone came from; it is not a
keep-alive edge, and no pass infers ownership from it. `LowOperator::Unify`
evaluates the actual value once before it evaluates the requirement, so a
deferred unify compares the value and not an unevaluated apply.

The template result is an ordinary value: `[name table, nominal id, fields…]`
for a struct, `[element template, elements…]` for an array, and `[elements…]`
for a tuple. The shared layout type is `Struct`, `Array`, or `Tuple`
respectively.

### The renderer

The renderer reads the layout from the type marker and the value's own offsets:
tuples start at `0`, arrays at `1` (after the element template), and structs at
`2` (after the name table and nominal id). Struct headers and array templates
are internal, so a struct prints as its field tuple and an array as its
elements. The type of a compound prints as `Tuple`, `Array`, or `Struct`.

Recursive bindings may leave a two-cell `[value, type]` skeleton. The renderer
reads those cells and guards cycles by equality class, not just node id; the
shared layout type does not truncate a nested value merely because it appears
again in the type position. Array template metadata is ignored for table-key
content but retained for template presence, keeping arrays and tuples
distinct.

### A materialized result keeps its owner

A computed struct result is an ordinary value, and materializing it can drop the
operation that produced it. The module therefore carries `apply_result_owners`,
a map from a returned value node to the apply that produced it.
`apply::record_applied_expr` is the dynamic write; `function::node_apply` and the
static mirror carry the mapping across clones, and the frozen form persists it.
`equality::applied_constructor` reads the table — directly, by the class root, or
by any node in the class — to recover the constructor of a value whose operation
is gone. The table is part of the artifact's format version 23: a v22 artifact
has no way to represent that ownership, so it is rebuilt rather than loaded with
a different answer. See [artifact-cache](artifact-cache.md).

### Constructor fields keep their source

`field_sources` complements constructor ownership with directed provenance: a
required field's exact value node names the actual field supplied for it. The
link never unifies the two nodes and never uses an equality-class representative
as its key.

Struct instantiation records the link while filling each field. If the supplied
value is still undecided, the field's **value position is not unified** with a
decided requirement: that comparison is vacuous and joining the classes would
let the requirement displace the actual. Type and attribute positions are still
checked. A decided supplied value keeps the ordinary whole-slot comparison and
conflict reporting.

A template
refinement whose actual is the current function parameter instead records a
constant projection of that parameter on the constructor's own template return
fields; a concrete actual is never written back into a shared template. Dynamic
and frozen clone remaps carry these links. When an `Index(Apply, 0)` view is
cloned, its source also reaches that apply, and `record_applied_expr` transfers
it to the corresponding returned value. After the parameter check,
`bind_field_sources` resolves constant parameter projections against the same
slots of the actual argument, so the source leaves the formal parameter and
names the supplied field.

Parallel input placement follows these links after its direct path and class
checks, with a bounded walk. Physical struct-slot paths are normalized against
the actual parameter values rather than compared directly with logical field
paths. The frozen table is artifact format 27;
[compute-replay-handoff](compute-replay-handoff.md) records the failed
alternatives and the measurements.

## The compute layer

The compute package's own source follows the same model. A kernel struct is a
direct template, `K := {.native @any, .I @any, .O @any}`, and a kernel is
matched against it with plain `=` — `launch := k = K => …`.

A computed struct call **returns a value**; it is not an instantiation.
`Buf x` returns the buffer struct, and the buffer is initialised by assigning
into it:

```text
buf := x = BufParam => { b := Buf x; b.native = $buf(x.type, x.length); b }
```

`(Buf x){…}` is not a supported second instantiation: the call has already
returned a value. A parameterized constructor such as `Buf` is a function and
must be applied explicitly where its result is used.

`type_of (k.O)` needs the space. `type_of (k.O)` is the application of
`type_of` to the field read; `type_of(k.O)` is a parenthesized read — the
tuple read of the name `type_of` — and it is refused as such.

The migration itself is not complete. The parts that follow the constructor
model now:

- `value_parameter_domain` reads a parameter's value-level tuple domain and
  evaluates a computed element whose type is not yet materialized; its fallback
  reads the applied expression's own type before `element_type_of`.
- `domain_value_leaves_of` and `body::domain_leaves` flatten nested tuple
  domains recursively, and `parameter_path` maps a leaf's empty path to the
  flattened ordinal. Before this a nested domain routed every leaf read at slot
  0.
- A residual `TypeOperator` node in a kernel body is lowered through the
  `Apply` it came from, not as a second operator. Before this a computed tuple's
  arithmetic emitted `[Bin(Add), Bin(Add)]` and answered `16` where the source
  says `8`.
- A computed conditional read shares a result type only when both branch type
  nodes resolve to the same class root; otherwise the read keeps the lazy type.
- In the frozen mirror, an in-parameter `@any` becomes a fresh hole, the plain
  cells are regrouped on the value and signature sides, and a cached `Index`
  answer reconciles its wildcard payload as the actual binds.

### Measured: the class-wide scope verdict is the compute regression

The suite fell from 112 passing to 67 at one commit, not across the migration.
Measured on `lichen-compute --test lichen_compute`, first failing run per commit,
the same tree each time:

| commit | passed | failed |
|---|---|---|
| `ea08077b` | 111 | 129 |
| `b71b81dd` | 112 | 128 |
| `14285cd8` — the class-wide scope verdict | **67** | **173** |
| `c1955675` — the replay and the materialized-parameter gate | 77 | 163 |
| `c730f83f`, `b602e5fa` | 77 | 163 |
| `e34e986e` (the compute branch's base) | 78 | 162 |

The reproduction is four lines and needs nothing compute-specific but the
prelude:

```lichen
---
  compute = import "compute.lichen"
---
a := 42
k := compute.jit (x => x + a)
compute.launch k 1
```

`43: Int` at `b71b81dd`; `none: Int` from `14285cd8` on, and the same with a
bound argument (`b := 1`) instead of the literal, so the argument's owner is
not the trigger. The program's own value is not the only loss: `probe`-level
instrumentation of the `$launch` operator shows its operand array as
`[kernel, empty cell]` — the second element has no value, no class and an
`Index` operation.

**Measured, not proposed**: forcing the dynamic verdict's answer to "cloned"
(`node_class_inside_scope` never refuses, behind an environment flag, then
reverted) answers `43` again. So the verdict is what turns the body's read into
a *reference*.

The mechanism, as far as it is measured. `compute.launch k 1` applies a
**static** prelude function to `k`, which *mints* a dynamic closure for the
frozen inner lambda and the program then applies that minted closure to `1`.
The parameter check of that last apply binds the argument into the parameter
**image** (measured: the image's value half reads `USize(1)`), while the body's
read of the same parameter is `Index(parameter_slot, 0)` and was *referenced in
place* instead of cloned — its class was judged outside the walk's scope. The
two are not one class, so the bind and the read land on different cells and
`$launch` sees an empty argument.

**The external member is measured.** Printed on the refusal of each verdict, the
apply of the minted closure (its own `FunctionId`) refuses exactly two nodes:
`595` — an operation with no value — and `602` — a decided cell — and each is
its class's **only** member, with `owner=None`. So the member is the node
itself: the frozen walk left it unowned because `FrozenClassRule::inside` said
its frozen class reaches outside the closure's scope, and an unowned class
summarizes to the virtual root, which no scope contains. The parameter's own
slot, by contrast, is owned by the minted closure and is cloned — hence the
split: the argument lands in the clone, the body reads the unowned cell.

Why the frozen class of a body node reaches outside is the open question. The
manifestation this slice can already name: `a <: k.I` in the frozen body is a
`Unify` that joins the parameter's cell with the kernel struct's field class,
whose owner is outside the closure — so the node is the closure's *own* and its
class is not. The dynamic path does not split here (`launch := k <: K => a =>
{a}` applied directly answers `3`), because every clone it makes is owned by
the call.

The frozen side says the same thing from the other end. Printed on every
`outside` verdict of the frozen rule, the two classes that matter are
`{1714(owner 14), 1555(owner 13), 1743(**owner none**), 1644(owner 14)}` and
`{1739(14), 1741(**none**), 1654(14), 1639(14), 1645(14), 1715(14), 1556(13)}`:
one **unowned** frozen cell per class makes the whole class external, and 13/14
are `launch`'s and its inner lambda's own functions. So the class is neither a
capture nor the source's own code — it is the source's own cells joined to a
top-level cell.

The verdict is class-wide, but the **outer instance is per cell** by design
(`FrozenClassRule::outer`: "the members of a class keep their own operator and
their own answer; the class topology is rebuilt by the walk's own regroup"), and
the regroup unifies only **plain cells** (`clone_is_a_plain_cell`). A follow-up
experiment included the outer instances of value-carrying members too: the two
classes really were re-unified, but the probe still answered `none`. Removing
that experiment also left the later 116-pass diagnostic candidate unchanged, so
the outer-instance key is **not** the root cause.

The freeze was checked at the source-to-static boundary: the unowned cells
already had `function=None` in the dynamic module, so the freeze is not dropping
an owner. The direct cutoff is instead the external launch **operation** cell:
its reference stops the clone walk before the parameter-read operands, while
the parameter itself is cloned and bound. The measured trace, the dynamic
control, and the unlanded operation-cloning experiment (compute 116/124,
lowlevel and language green) are recorded in
[replay-apply](replay-apply.md#measured-an-external-computation-cell-cuts-off-the-operand-walk).
The repair must cover that computation-return case without splitting the
external class; the earlier candidates (a), (b), and (c) do not address it.

## Dependent lengths

A decidable array length is built directly. A length that depends on an
enclosing lambda parameter is deferred through the constructor result's own
length read: the instance's count pins that cell, and a later argument mismatch
is a **runtime** error, not a check-time one. A top-level `_` remains guarded;
`[T, _]` does not infer a length. A layout-only constraint can use ordinary
`Array` annotation.

## Measured status

After the direct-template migration on `dev@ffef59c8`:

- `cargo test -p lichen-highlevel --test highlevel`: 136 passed.
- `cargo test -p lichen-language --test language`: 275 passed.
- `cargo test -p lichen-lowlevel --test basic`: 164 passed, 1 ignored.
- The full `lichen-compute` suite is **181 passed, 59 failed, 1 ignored** after
  the graph-argument fix, the recording-drive removal and the replay reuse (the
  direct-template baseline was 137 / 103 / 1; the field-position and hover fixes
  reached 147 / 93, and the buffer/argument fixes reached 156 / 84). The language-server
  hovers, the render tests and the serve schema probe are green. The remaining
  failures are the compute graph families, not evidence against the source
  model.
- Three defects were measured and fixed in the recording path this round:
  - A graph-run argument that is a `Buf` wrapper flattened its `.native`,
    `.type` and `.length` fields into three arguments, so a recorded graph that
    took two values was run with three. A buffer is now one argument — its
    payload (`flatten_leaves`). This flips nine tests and accounts for the
    whole `RunArity` family.
  - The extent (`s.n`) is a read of the recorded function's parameter; its
    `field_sources` entry is recorded on the candidate's class root, while
    `recorded_leaf` looked only at the exact node. The reader now consults the
    class's representative (`field_source_of`), which resolves the count in
    every recording.
  - The absorbed buffer's wrapper is decided while its payload is open: the
    wrapper value is the field read's expr, and the applied projection names
    the buffer the read means. `recorded_buffer_payload` now reads that
    projection whenever either side is open. `argument 1 of this dispatch is
    not a buffer` falls from 60 to 4.
- The remaining families are the graph's return ("this function's return is
  not a value a graph can hand back"), the remaining absorbed buffers, the
  buffer class mismatch, the Int/Float mixes and a few one-off refusals. The
  region derivation is no longer a family: the replay reuse gives a replayed
  call the node its source call recorded. The captured path for a
  body read still shows it stamped **undecided** during the checker's deep
  pass (`Checker::build_with` → `wire_apply_result` → `apply` →
  `evaluate_node_deep` → `run_deferred` → `Index` → `TableGet`) before the
  producer's recorded result exists; the read's class is the template's (an
  external verdict), so no per-call clone reaches it. The next handle is still
  the general dynamic-apply ownership/replay (`apply-clone-ownership` §6).
- The SPEC states `s.<name> = s.value[s.value.name_table[name] + 2]`, so a
  read's target must be evaluated before its subscript. Reordering the `Index`
  evaluator that way keeps lowlevel 164, language 276, highlevel 136 and
  serve 11 green but does not flip a test by itself, so it is held for the
  same slice.

- `examples/array.lichen` prints `[1, 2, 3]: Array`, `examples/tuple.lichen`
  prints `(1, Int): Tuple`, and `examples/struct.lichen` prints its declared
  shape. The README sync test passes.
- The array instance path is measured: `A := [@any : Int, 2]; A[1, 2] : Array`
  reads `[1, 2]`. `A[f 1, f 2]` with a calling element is still a known gap:
  the element template's value is specialized by the first call.

## Known remaining gaps

- A direct array template whose element is a call still specializes the
  element template: `A := [@any, 2]; A[f 1, f 2]` reports an element conflict.
- A kernel body that reads a field through a template-valued parameter —
  `k.in.x`, where `k.in` is a template rather than an instance — now resolves:
  the chain starts at the shared template, so the field path from the parameter
  to that base is prepended, the spec's `name_table[name] + 2` selector is read
  through its name side, and a struct position step is physical. The serve
  schema probe passes; `examples/attention.lichen` now reaches the next
  refusal, an `Int`/`Float` mix in the attend body.
- `examples/rmsnorm.lichen` fails in parallel lowering: a kernel body reaches a
  node with neither a value nor an operation, and a read's buffer argument is
  not an input buffer.
- `examples/struct_recursion.lichen` did not finish within 120 seconds and was
  stopped as nonterminating.
- `crates/lichen-compute/examples/reduction_interface.rs` still spells a retired
  compound form and does not parse.
- The Rust `recursion` example needs the optional `device` feature; it was not
  run because no backend is started.
- `a_write_inside_a_conditional_is_refused` still reports an output-class error
  instead of the conditional-write cause.
- A parameter annotation on a recursive array constructor still loses the array
  result type: `nats := (n : Int) => T[n, @shallow nats (n + 1)]` is refused
  with `expected Array, found Int`; the unannotated `nats := n => …` runs. This
  is the same source shape the book used to illustrate laziness, so the book
  now spells the working form.

## Verification boundary

The source model is measured when plain `=` performs no application, `<:` is
retired in source, every compound instance uses its template value directly
without cloning an outer capture, and the grammar and language suite no longer
accept the retired `tuple{…}`, `array{…}`, `struct{…}`, or source `@clone`
spellings. The remaining consumer is the compute layer's parallel lowering: its
scalar `compute.jit` path is measured at `6: Int`, while the buffer-valued
kernels listed above still fail. The one source file that still requires a
retired compound spelling is `reduction_interface.rs`, named above.

# Constructor templates

> Status: **current for the source language; the compute migration is
> incomplete.**
> Points at: `crates/lichen-language-parser` (the template syntax),
> `crates/lichen-language/src/compile.rs` (the two applications),
> `crates/lichen-highlevel/src/checker` (value construction and reads),
> `crates/lichen-lowlevel/src/apply.rs`, `function.rs`, and
> `static_module/apply.rs` (the apply's scope and its frozen mirror),
> `crates/lichen-lowlevel/src/equality.rs` (a materialized result's owner), and
> `crates/lichen-compute/src/compute.rs` / `compute/body.rs` (the compute
> surface).

Compound layout lives in a value, not in a shape-bearing type. `Tuple`, `Array`,
and `Struct` are the shared layout types; a template is a function whose
application produces a fresh requirement. The source of truth is [the language
spec](../language-spec.md); this note records the migration decisions, the
reasons for them, and the measurements that now hold.

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
  instantiated. Function scope, closed-cell sharing, and the per-application
  remap are the ownership model; graph reachability is not.
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

## Implementation

The source-to-IR step performs the application the syntax names: `<:` inserts
`T _` before unification, and an instance inserts the constructor application
before filling the returned fields or element requirement. The apply's own
scope supplies freshness. `ApplyCtx` clones the callee's template, shares the
cells the template captured, mints a fresh closure per call, and rebuilds the
class topology with `apply::regroup_clones` / `unify_clone_groups`. `Node::origin`
records where a clone came from; it is not a keep-alive edge, and no pass infers
ownership from it. `LowOperator::Unify` evaluates the actual value once before it
evaluates the requirement, so a deferred unify compares the value and not an
unevaluated apply.

The constructor result is an ordinary value: `[name table, nominal id, fields…]`
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
the regroup unifies only **plain cells** (`clone_is_a_plain_cell`). A class whose
members carry values is therefore not re-unified, and two members' outer
instances stay two classes. At the dynamic apply of the minted closure both are
unowned, so both are referenced and nobody clones them; the parameter check had
already bound the parameter's *own* instance.

Three candidate repairs, none taken. (a) Judge ownership as well as the class
summary — tried and measured: the unowned member stays unowned, so the
`inside` OR changes nothing on this program and was reverted. (b) Key the outer
instance by class instead of by cell, so one external class is one instance —
needs the regroup to carry value-carrying members too, which is where the
union-by-size rule decides the class's answer. (c) Make the parameter follow the
class verdict, so the argument binds the shared cell the body reads, accepting
one parameter cell per kernel call. A **baked** clone's unowned owner was tried
as the cause earlier and changed nothing (reverted).

## Dependent lengths

A decidable array length is built directly. A length that depends on an
enclosing lambda parameter is deferred through the constructor result's own
length read: the instance's count pins that cell, and a later argument mismatch
is a **runtime** error, not a check-time one. A top-level `_` remains guarded;
`[T, _]` does not infer a length. A layout-only constraint can use ordinary
`Array` annotation.

## Measured status

At `ea08077b`, after the function-template and computed-ownership squash
(`bd360fac`):

- `cargo test -p lichen-highlevel --test highlevel`: 136 passed.
- `cargo test -p lichen-language --test language`: 275 passed.
- `cargo test -p lichen-lowlevel --test basic`: 164 passed, 1 ignored.
- The grammar-consistency check: 2 passed.
- `examples/array.lichen` prints `[1, 2, 3]: Array`, `examples/gcd.lichen`
  prints `6: Int`, and `examples/compute_jit.lichen` prints `6: Int`.
- The array instance path is measured: `A := _x => [@any : Int, 2]; A[1, 2] :
  Array` reads `[1, 2]`. The previous note's `expected Array, found Array`
  refusal no longer reproduces.
- The full `lichen-compute` suite is 111 passed, 129 failed, 1 ignored (it was
  88 passed, 152 failed before the migration). The compute failures are the
  remaining work, not evidence for the source model.

## Known remaining gaps

- A kernel body that reads a field through a constructor-valued parameter —
  `k.in.x`, where `k.in` is the constructor `In` rather than an instance — is
  refused with `expected Struct, found ?a => Struct`. The book's compute
  programs show this first refusal.
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

The source model is measured when `<:` applies exactly once, plain `=` applies
never, every compound instance applies its constructor once without cloning an
outer capture, and the grammar and language suite no longer accept the retired
`tuple{…}`, `array{…}`, `struct{…}`, or source `@clone` spellings. The remaining
consumer is the compute layer's parallel lowering: its scalar `compute.jit` path
is measured at `6: Int`, while the buffer-valued kernels listed above still
fail. The one source file that still requires a retired compound spelling is
`reduction_interface.rs`, named above.

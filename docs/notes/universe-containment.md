# "Contains the universe" is not "is the universe"

> Status: **§2 and §3 landed.** The printer's `is_universe` now tests the whole
> `[Type, ↺]` shape, so a frozen kind is no longer mistaken for the universe. Two
> frozen kinds with different markers no longer unify: §3 records that the lowlevel
> walk was **never** the thing that merged them, and that the defect was a frozen
> field read declining to write its type into its own cell.
>
> Points at: `crates/lichen-render/src/render.rs` (`is_universe`) and
> `crates/lichen-highlevel/src/checker/structs.rs` (`slot_read`).
>
> Companions: [type-of-in-std](type-of-in-std.md) (the `.sig` mis-render §2
> explains), [type-rendering-inconsistent](type-rendering-inconsistent.md) (a
> distinct spelling defect: which *node* a diagnostic reports).

## 1. The flaw

The universe is the fixpoint `K = [Type, ↺]`: a 2-element array whose **head is
the `Type` marker** and whose tail is itself. A predicate that tests only the
tail — "a 2-element array one of whose elements points back at its own class, or
statically through the frozen self-loop" — cannot tell the universe from a kind
`[marker, K]` or an atomic type `[int, K]`, which have the same silhouette.

The static arm is load-bearing: when a unification materializes a static
universe, the copy keeps its tail a static ref (`[head, staticK]`), and the
dynamic self-loop test alone cannot see the cycle. So the fix is not "delete the
static arm" but "**the static arm must check the whole shape**" — the tail is the
static universe **and** the head is the universe's own head.

The trigger condition is narrow and worth stating: only types whose `K` slot is
the **static** universe — type values computed inside a frozen module (an
imported package, a native plugin source) and then read in the importing module.
A kind built locally holds the dynamic universe, whose class is not the kind's
class, so the honest representative test already answers no.

The same encoding also costs the walks. The universe `K = [Type, ↺]` is reached
**twice** by any walk that follows type structure, and both such walks handle it
rather than loop: the deep pass cuts the re-entry and **assumes the node concrete**
for the readers that reach it while its own frame is still computing it — the
coinductive step that lets a cyclic value be proven at all. The assumption fills a
missing verdict and never overrides one, and is cleared where the real verdict is
written. `evaluate_pattern_argument` instead keeps a `seen` set of `(pattern,
argument)` pairs, because a typed pattern's spine reaches the universe twice and must
be walked once. Cloning the universe per apply is a unification *conflict* rather
than a slowdown, which is why the checker refuses it outright instead of paying for
it.

## 2. The printer mis-spelled a correct arrow as `TypeStruct` (fixed)

`compute.jit`'s `.sig` field — the standard library's `type_of f` read in a
struct field's type position — rendered `TypeStruct` instead of the signature.
**The graph was correct**: the field-type node's committed value is the arrow
pair `[[Int, Int], [TypeFunction, K]]`, with `K` the *static* universe (the gate
`f : _ -> _` builds the arrow inside the frozen module). Nothing pinned anything
wrong; the printer misclassified it.

Two structural guesses compounded: `is_universe` said "contains the static
universe" (the kind `[TypeFunction, K_static]` does), and `marker_is_struct`
guessed from a 2-element array that the arrow's `[Int, Int]` shape was a struct
marker. Together they took the arrow *type value* for a struct *kind*, so the
cascade's first branch printed the literal `"TypeStruct"` and the honest
`[shape, [marker, K]]` branch — which would print `Int -> Int` — was never
reached. Without the gate, the argument's arrow is built in the importing module
(dynamic `K`), `is_universe` answers honestly, and the same program renders
`.sig Int -> Int`.

**The fix** makes `is_universe`'s dynamic arm the honest `[Type, ↺]` shape test:
the node's value is a 2-element array, its head is the program's `Type` marker
(the renderer knows the marker), and its tail is the node's own class — or, for a
materialized static universe, the static arm that already checks the whole shape.
A self-cycle whose head is not the `Type` marker is not the universe, whatever
else it resembles.

Measured with exactly that change: the three acceptance tests run with their
`#[ignore]` removed (`a_kernel_value_and_type_render_by_name`,
`a_tuple_domain_kernel_type_renders_as_a_function`,
`compute_kernel_bindings_render_by_name_not_raw_layout`), the whole `compute`
suite is green, and the minimal repro flips from
`… .sig TypeStruct` to `… .sig Int -> Int`:

```lichen
# mylib.lichen — registered native, i.e. frozen
let type_of = x => {t = _; x: t; t}
jit = f => {
  f : _ -> _
  (struct{.native _, .sig (type_of f)})(.native 0, .sig _)
}

# main
--- lib = import "mylib.lichen" ---
k = lib.jit (y => y + y)
k
```

## 3. The frozen-kind hole, and where it actually was

### 3.1 The hole

Two frozen kinds with *different markers* unified silently:

```lichen
# a.lichen (frozen)
arrow_t = Int -> Int
# b.lichen (frozen)
tuple_t = tuple{Int, Int}

# main
---
  a = import "a.lichen"
  b = import "b.lichen"
---
a.arrow_t == b.tuple_t
```

Measured before the fix: **accepted**, evaluating to `0: Int` — the arrow type and
the tuple type unified without error, and the equality then answered false at
runtime. This is a soundness hole, not a rendering quirk: any program that unifies
two frozen-module types whose shapes happen to match skips the kind comparison
entirely.

### 3.2 Where it was — not in the walk

**Correction to the earlier reading of this section.** It claimed that
`unify_inner`'s array arm merges two classes without descending whenever both sides
pass `is_self_referential`. **No such cut exists.** `is_self_referential` has no
caller in `unify_inner`; its only callers are itself (recursing into a static
item), `function_type_function`, and `shape::struct_term_parts`. What the walk
actually does is honest throughout: an array's element 0 is compared
`value_pair_equal`, so two different kind markers are a recorded conflict. The
"two absences" branch is reached only where at least one side names no node, and a
class pair of two valueless nodes takes the `(None, None)` merge — correct for a
cell that is genuinely undecided.

Traced, the `==` check is `check_unify a=NodeId(34v1) b=NodeId(52v1) kind=BinOp`,
and **both** cells read `ABSENT` while naming nodes, so the `(None, None)` arm
merged them and the walk was never asked about the kinds at all. `TypeFunction`
appears **zero** times in a full trace of the run; the two kinds are never compared
anywhere because neither of them ever exists.

### 3.3 The cause and the fix

`slot_read` decides a field read's type eagerly when the key is a literal, by
reading `shape::field_type` out of the field list. It then discarded a **frozen**
answer:

```rust
.and_then(|ty| match ty {
    AnyNodeId::Dynamic(node) => Some(node),
    AnyNodeId::Static(_) => None,   // a frozen field type kept the lazy form
});
```

So `a.arrow_t`'s type cell was not the field's type but an unevaluated chain of
three `Index` applies — `NodeId(34v1)`, created by `add_node` with an operation and
no value. `shape::field_type` had resolved it, to
`Static(ModuleKey(312), LocalNodeId { index: 27 })`; nothing read the answer. This is
a **missing write**, not an ordering problem: the cells are still `ABSENT`
immediately after the unify and never receive a value at any later point in the run.

The fix is the uniform one — the frozen read writing its type into its cell exactly
as a dynamic field type does, by the same `as_dynamic` the `ExprKind::Static`
export read already uses. `==` then needs no change: its operands' type cells hold
real types, the existing array descent runs, and the markers conflict.

**The rendered message** is `expected TypeTuple, found Int -> Int`, not
`expected TypeTuple, found TypeFunction`: `DiagKind::BinOp` renders the *root* type
nodes (`printer.node(d.b)` / `printer.node(d.a)`), and a function kind prints as
its arrow. The underlying `UnifyError` pair is `TypeFunction` vs `TypeTuple`, which
is the comparison §3.1 called for.

### 3.4 The three compute tests do not break

An earlier draft of this section predicted that the lowlevel-side fix would break
`a_gpu_program_chains_two_kernels_on_a_device`,
`a_float_fragment_agrees_across_the_two_backends` and
`an_integer_fragment_agrees_across_the_two_backends`, each with
`expected raw[Int, ?], found Int`. **Measured with the landed fix: all three report
`ok`**, and the whole compute suite holds its baseline — 203 passed / 15 failed /
2 ignored, with the failure *name* set identical to a pristine `dev` run. The
prediction came from a different mechanism (`is_self_referential`'s static arm), and
the argument it rested on — a `[?, K_frozen]` argument type reached at
`apply_parameter_check` — is not what this defect was.

# "Contains the universe" is not "is the universe"

> Status: **§2 landed; §3 open.**  Two predicates answer "is this node the
> universe `K = [Type, ↺]`?" by asking only whether the node *contains* the
> frozen universe among its elements.  A kind `[marker, K]` built inside a
> frozen module holds the static universe at slot 1, so it passes both tests —
> with two measured consequences: the printer mis-spelled a correct
> `Int -> Int` field type as `TypeStruct` (§2, **fixed**: `is_universe` now
> tests the whole shape, and the three `type_of` acceptance tests run
> un-parked), and unification merges two frozen kinds **without comparing
> their markers** (§3, **open**).  The lowlevel twin is verified against the §3
> repro but breaks three `compute` tests that ride on the unsound merge, so
> landing it needs the compute-side question in §3.4 answered first.
>
> Points at: `crates/lichen-render/src/render.rs` (`is_universe`),
> `crates/lichen-lowlevel/src/equality.rs` (`is_self_referential`, and
> `unify_inner`'s "two self-referential universes" cut that consumes it).
>
> Companions: [type-of-in-std](type-of-in-std.md) § *The defect the removal
> exposed* — the `.sig` mis-render this note explains; its earlier "the checker
> pins the wrong value" reading is **withdrawn** (the graph was never wrong).
> [defer-pending-type-forms](defer-pending-type-forms.md) — the deferral
> machinery is *not* involved: no `defer_pending` verdict fires anywhere in the
> §2 repro (measured).  [type-rendering-inconsistent](type-rendering-inconsistent.md)
> — a distinct spelling defect (which *node* a diagnostic reports); this note is
> about a predicate misclassifying the right node.

## 1. The flaw

The universe is the fixpoint `K = [Type, ↺]`: a 2-element array whose **head is
the `Type` marker** and whose tail is itself.  Two predicates test only the
tail:

- the renderer's `is_universe` (`lichen-render/src/render.rs`), dynamic arm:
  true when **any** element is in the node's own class *or is a static
  self-referential universe*;
- the lowlevel's `Module::is_self_referential` (`lichen-lowlevel/src/equality.rs`),
  dynamic arm: the same shape ("a 2-element array one of whose elements points
  back at its own class … or statically through the frozen self-loop").

Neither checks the head — the only slot that distinguishes the universe
`[Type, ↺]` from a kind `[marker, K]` (or from an atomic type `[int, K]`, which
has the same silhouette).

The static arm is load-bearing: when a unification materializes a static
universe, the copy keeps its tail a static ref (`[head, staticK]`), and the
dynamic self-loop test alone cannot see the cycle.  So the fix is not "delete
the static arm" but "**the static arm must check the whole shape**": tail is the
static universe *and* head is the universe's own head.

Trigger condition: only kinds/types whose `K` slot is the **static** universe —
i.e. type values computed inside a frozen module (a package import, a native
plugin source) and then read in the importing module.  A kind built locally
holds the dynamic universe, whose class is not the kind's class, so the honest
representative test already answers no.

## 2. Manifestation A — the printer mis-spells a correct arrow as `TypeStruct`

This is the mechanism behind [type-of-in-std](type-of-in-std.md) § *The defect
the removal exposed*: `compute.jit`'s `.sig` field — the standard library's
`type_of f` read in a struct field's type position — rendered `TypeStruct`
instead of the signature.  **The graph is correct**: a class dump of the `.sig`
field-type node shows its committed value is the arrow pair
`[[Int, Int], [TypeFunction, K]]`, with `K` the *static* universe (the gate
`f : _ -> _` builds the arrow inside the frozen module).  Nothing pins anything
wrong; the printer then misclassifies it.

### 2.1 The misclassification chain

`TypePrinter::elements` (`lichen-render/src/render/type_printer.rs`) cascades;
branch 1 asks `shape::is_struct_kind(node)` — "is this node a bare struct kind
`[id, K]`?" — via `kind_is_struct(node's items)`:

```text
kind_is_struct([shape, kind]) =
       items.len() == 2
    && is_universe_any(items[1])     ← the kind [FT, K_static] "contains" the
    && marker_is_struct(items[0])       static universe → misread as the universe
                                      ← the shape [Int, Int] is a 2-element
                                        array → `marker_is_struct`'s structural
                                        guess (since replaced by the
                                        `TypeStruct` tag check)
                                        reads it as a struct marker
```

Both guesses fire, so the arrow *type value* `[[Int,Int],[FT,K]]` is taken for a
struct *kind* and branch 1 prints the literal `"TypeStruct"`.  Branch 4 — the
honest `[shape, [marker, K]]` compound arm that would print `Int -> Int` — is
never reached.

Why the gate decides it: without `f : _ -> _`, the argument's arrow is built in
the *importing* module (dynamic `K`), `is_universe` answers honestly, and the
same program renders `.sig Int -> Int` (measured).  With the gate, the arrow —
hence its kind — is frozen, and the misfire triggers.  This is why every
wrapper-spelling experiment in type-of-in-std § *What was tried* failed: they
all moved *when* the read happens, never *where the arrow's K comes from*.

### 2.2 The fix (renderer) — **landed**

`is_universe`'s dynamic arm is now the honest `[Type, ↺]` shape test — the
renderer knows the marker (`P::Value::type_marker()`), so it checks the head
directly:

```rust
fn is_universe<P: HighProgram>(module: &Module<P>, node: NodeId) -> bool
where
    P::Value: ValueType,
{
    let Some(rep) = representative(module, node) else {
        return false;
    };
    let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|value| value.as_enum())
    else {
        return false;
    };
    // SAFETY: `array` is the payload of the value read from the live node `node`.
    let items = unsafe { array.items() };
    if items.len() != 2 {
        return false;
    }
    if module.node_value(items[0].node) != Some(P::Value::type_marker()) {
        return false;
    }
    match items[1].node {
        AnyNodeId::Dynamic(item) => representative(module, item) == Some(rep),
        AnyNodeId::Static(_) => is_universe_any(module, items[1].node),
    }
}
```

(The static arm of the free `is_universe_any` already checks the head —
`items[0] == type_marker && items[1] == self` — so it stays as it is.)

Measured with exactly this change, nothing else:

- the three acceptance tests pass with their `#[ignore]` removed (they now run
  un-parked): `a_kernel_value_and_type_render_by_name`,
  `a_tuple_domain_kernel_type_renders_as_a_function`
  (`crates/lichen-language/tests/compute.rs`), and
  `compute_kernel_bindings_render_by_name_not_raw_layout`
  (`crates/lichen-language-server/tests/statement_values.rs`);
- the whole `compute` suite is green (53 passed);
- `lichen-lowlevel` (143), `lichen-highlevel` and the rest of
  `lichen-language`'s suites are green;
- the minimal repro flips from
  `(0, parameterized): struct<.native raw[Int, Type], .sig TypeStruct>` to
  `… .sig Int -> Int`:

```lichen
# mylib.lichen — registered native, i.e. frozen
let type_of = x => {t = _; x: t; t}
jit = f => {
  f : _ -> _
  (struct<.native _, .sig (type_of f)>)(.native 0, .sig _)
}

# main
--- lib = import "mylib.lichen" ---
k = lib.jit (y => y + y)
k
```

The landing was mechanical and is done: the change is in
`crates/lichen-render/src/render.rs`, the three `#[ignore]` attributes and their
"open defect" comments are gone, and
[type-of-in-std](type-of-in-std.md) § *The defect the removal exposed* is marked
fixed.

## 3. Manifestation B — unification merges frozen kinds without comparing markers

### 3.1 The hole

`unify_inner`'s array arm (`lichen-lowlevel/src/equality.rs`, the "two
self-referential universes are the same structural value" cut) merges the two
classes **without descending** whenever both sides pass `is_self_referential`.
With the permissive predicate, every frozen kind `[marker, K_static]` passes —
so two frozen kinds with *different markers* merge silently:

```lichen
# a.lichen (frozen)
arrow_t = Int -> Int
# b.lichen (frozen)
tuple_t = <Int, Int>

# main
---
  a = import "a.lichen"
  b = import "b.lichen"
---
a.arrow_t == b.tuple_t
```

Measured on `dev` (`6059214`): **accepted**, evaluates to `0: Int` — the arrow
type and the tuple type unified without error (the equality then answers false
at runtime).  The honest descent compares `[TypeFunction, K]` against
`[TypeTuple, K]` element-wise and must record `expected TypeTuple, found
TypeFunction`.

This is a soundness hole, not a rendering quirk: any program that unifies two
frozen-module types whose shapes happen to match (or whose mismatching parts are
undecided cells) skips the kind comparison entirely.

### 3.2 The verified fix (lowlevel)

`is_self_referential`'s dynamic arm: keep the honest dynamic self-loop test, and
make the static case check the whole shape — tail is the static universe **and**
head equals the universe's own head.  The lowlevel does not know the program's
markers, so the head is compared *by value against the static universe's head*
(the one marker it can reach without vocabulary knowledge):

```rust
Dyn(node) => {
    let rep = self.equality_representative(node);
    let Some(LowValue::Array(array)) =
        self.nodes[node].value.and_then(|value| value.as_enum())
    else {
        return false;
    };
    // SAFETY: `array` is the payload of `node`, a live node of this module,
    // so its home block has not been dropped.
    let items = unsafe { array.items() };
    if items.len() != 2 {
        return false;
    }
    // SAFETY: as above — `node` is a live node of this module.
    if items.iter().any(|item| match item.node {
        Dyn(item) => self.equality_representative(item) == rep,
        AnyNodeId::Static(_) => false,
    }) {
        return true;
    }
    // A materialized static universe keeps its tail a static ref:
    // `[head, staticK]`.  It is the cycle only if its head is the
    // universe's own head.
    let AnyNodeId::Static(tail) = items[1].node else {
        return false;
    };
    if !self.is_static_universe_id(tail) {
        return false;
    }
    let Some(LowValue::Array(universe)) = self.static_read(tail).as_enum() else {
        return false;
    };
    // SAFETY: `universe` is a static payload read through `tail`, whose home
    // module is registered — the registration pins its arena.
    let Some(head) = unsafe { universe.items() }.first() else {
        return false;
    };
    self.node_value(items[0].node) == self.node_value(head.node)
}
```

Measured with this change: the §3.1 repro is rejected with
`expected TypeTuple, found TypeFunction`; `a.arrow_t == a.arrow_t` still
evaluates to `1`; `lichen-lowlevel`'s 143 tests stay green.

(`is_static_universe_id` itself only checks the self-loop, not the head; a
*static* self-referential 2-array is only ever the frozen universe, so it stays
as it is.)

### 3.3 The blocker — three compute tests ride on the unsound merge

With the lowlevel fix (and only it), these fail:

- `a_gpu_program_chains_two_kernels_on_a_device`
- `a_float_fragment_agrees_across_the_two_backends`
- `an_integer_fragment_agrees_across_the_two_backends`

(`crates/lichen-language/tests/compute.rs`; all three pass on `dev`, so this is
not a GPU-availability artifact.)  Each fails at check time with
`expected raw[Int, ?], found Int`, the underlying `UnifyError` being
`TypeBuffer` vs `Type` markers.

### 3.4 What is measured about the failure, and what is not

Instrumented facts (a `RECORD-ERROR`/`PARAM-CHECK-FAIL` probe in
`equality.rs`/`apply.rs`, since removed):

- the conflict fires in `apply_parameter_check` (the post-clone parameter
  unify) of a **static** function apply:
  `StaticFunctionRef { module: ModuleKey(0), index: StaticFunctionId(13) }` in
  the frozen compute module.  By `compute.lichen`'s function-creation order
  (`type_of`, `jit`, `launch` ×2, `call` ×2, `parallel` ×2, `plrun` ×2,
  `range`, `read`, `write`, `collect`, …) index 13 is **`collect`'s parameter
  lambda** `b => $collect(b)` — consistent with the declared side, whose type
  slot is a buffer type `[?, [TypeBuffer, K]]` with everything frozen/static.
- the actual argument's type slot is `[?, K_frozen]`: shape undecided, **kind the
  bare frozen universe** — not a marker-kind.  The honest descent therefore
  reaches `TypeBuffer` (declared kind's marker) vs `Type` (the universe's head)
  and records the conflict; the permissive predicate used to merge the two
  sides at the cycle cut before any marker was compared.
- the program shape involved is the `parallel`/`plrun` chain
  (`out = compute.plrun k2 (3, (inbuf,))` then `compute.collect out`), so the
  argument is a `plrun` result whose type should have been Buffer.

**Not known:** why that argument's type carries the bare frozen universe in its
kind slot.  Two readings, not distinguished:

1. something upstream (the `$plrun` native's declared codomain, or the lazy
   `k.sig`-style reads in the `parallel` wrappers) leaves the result's type
   half-decided as `[?, K]`, and the program only ever worked because the hole
   swallowed the mismatch; or
2. `[?, K]` here is a legitimate *undecided* type (a cell that should still be
   free to refine into the buffer type), and the honest comparison needs a
   refinement rule the lowlevel does not have — i.e. the cycle cut was doing
   accidental duty as "an undecided type unifies with anything".

Answering this is the prerequisite for landing §3.2.  The renderer fix (§2.2)
did **not** depend on the answer and landed alone.

## 4. Suggested order of work

1. ~~Land §2.2, un-park the three acceptance tests, update
   [type-of-in-std](type-of-in-std.md) (defect → fixed, mechanism corrected)
   and the `docs/README.md` index rows.~~ **Done.**
2. Investigate §3.4 on the `parallel`/`plrun`/`collect` chain (instrument the
   argument's type at the `collect` apply; compare against `$plrun`'s declared
   codomain in `lichen-compute`).
3. Land §3.2 once the compute side is honest, with a regression test for the
   §3.1 repro (two frozen kinds with different markers must not unify).

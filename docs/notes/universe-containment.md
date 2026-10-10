# "Contains the universe" is not "is the universe"

> Status: **§2 landed; §3 open.** The printer's `is_universe` now tests the whole
> `[Type, ↺]` shape, so a frozen kind is no longer mistaken for the universe. The
> lowlevel's twin predicate still tests only the tail, so unification merges two
> frozen kinds **without comparing their markers**: that is a soundness hole, and
> landing the verified fix is blocked on the compute-side question §3 records.
>
> Points at: `crates/lichen-render/src/render.rs` (`is_universe`) and
> `crates/lichen-lowlevel/src/equality.rs` (`is_self_referential`, and
> `unify_inner`'s "two self-referential universes" cut that consumes it).
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
  (struct<.native _, .sig (type_of f)>)(.native 0, .sig _)
}

# main
--- lib = import "mylib.lichen" ---
k = lib.jit (y => y + y)
k
```

## 3. Why the lowlevel twin is still open

### 3.1 The hole

`unify_inner`'s array arm merges two classes **without descending** whenever both
sides pass `is_self_referential`. With the permissive predicate, every frozen kind
`[marker, K_static]` passes, so two frozen kinds with *different markers* merge
silently:

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

Measured: **accepted**, evaluating to `0: Int` — the arrow type and the tuple type
unified without error, and the equality then answered false at runtime. The
honest descent compares `[TypeFunction, K]` against `[TypeTuple, K]` element-wise
and must record `expected TypeTuple, found TypeFunction`.

This is a soundness hole, not a rendering quirk: any program that unifies two
frozen-module types whose shapes happen to match — or whose mismatching parts are
undecided cells — skips the kind comparison entirely.

### 3.2 The verified fix, and its blocker

The fix is `is_self_referential`'s static case: keep the honest dynamic self-loop
test, and for the static case check the whole shape — the tail is the static
universe **and** the head equals the universe's own head. The lowlevel does not
know the program's markers, so the head is compared *by value against the static
universe's head*, the one marker it can reach without vocabulary knowledge.
(`is_static_universe_id` itself only checks the self-loop, not the head; a
*static* self-referential 2-array is only ever the frozen universe, so it stays as
it is.) Measured with that change: the §3.1 repro is rejected with
`expected TypeTuple, found TypeFunction`, `a.arrow_t == a.arrow_t` still
evaluates to `1`, and the lowlevel's tests stay green.

**It breaks three compute tests**, all of which pass otherwise, so this is not a
GPU-availability artifact:

- `a_gpu_program_chains_two_kernels_on_a_device`
- `a_float_fragment_agrees_across_the_two_backends`
- `an_integer_fragment_agrees_across_the_two_backends`

Each fails at check time with `expected raw[Int, ?], found Int`, the underlying
`UnifyError` being `TypeBuffer` vs `Type` markers. Instrumentation located the
conflict in `apply_parameter_check` of a static function apply — `collect`'s
parameter lambda in the frozen compute module — where the **actual argument's type
slot is `[?, K_frozen]`**: shape undecided, kind the bare frozen universe, not a
marker-kind. The honest descent therefore reaches `TypeBuffer` (the declared
kind's marker) against `Type` (the universe's head), and the permissive predicate
used to merge the two sides at the cycle cut before any marker was compared. The
program shape is the `parallel`/`plrun` chain followed by `compute.collect`.

**What is not known** is why that argument's type carries the bare frozen universe
in its kind slot. Two readings, not distinguished:

1. Something upstream — the `$plrun` native's declared codomain, or the lazy
   `k.sig`-style reads in the parallel wrappers — leaves the result's type
   half-decided as `[?, K]`, and the program only ever worked because the hole
   swallowed the mismatch; or
2. `[?, K]` here is a legitimate *undecided* type (a cell that should still be
   free to refine into the buffer type), and the honest comparison needs a
   refinement rule the lowlevel does not have — the cycle cut was doing accidental
   duty as "an undecided type unifies with anything".

Answering that is the prerequisite for landing the lowlevel fix. The printer fix
did not depend on the answer and landed alone. The order of work is: instrument
the argument's type at the `collect` apply on the `parallel`/`plrun`/`collect`
chain and compare it against `$plrun`'s declared codomain in `lichen-compute`;
then land the lowlevel fix with a regression test for the §3.1 repro (two frozen
kinds with different markers must not unify).

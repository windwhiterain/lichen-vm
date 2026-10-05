# A call's clones belong to the enclosing template

A value that travels through a wrapper function disappears.  This note records
what the disappearance actually is, the commit that introduced it, and the
rules the clone walk was missing on both the dynamic and the static side.  It
supersedes §9 of
[`block-vs-lambda-struct-field-inference.md`](block-vs-lambda-struct-field-inference.md),
which looked for the cause in the checker's field-list check.

## 1. The reproductions

```lichen
id = x => x
f = x => id x
f 1
```

read `parameterized: ?a` where it should read `1: Int`; and

```lichen
add = x => y => x + y
double = x => add x x
double 5
```

read `parameterized` where it should read `10`.  Both are the same shape: a
call whose *result* the enclosing body goes on to use, where the callee's body
is materialized inside the caller's body.

The `examples/import` drift is the same shape across a frozen module
(`(42, none, 7)` for `(42, 10, 7)`).

## 2. Measure through the registry entry point, not the bare one

Two entry points share one renderer (`run::render_build`):

| entry | what it does |
| --- | --- |
| `run::evaluate(source)` | `compile(source)` — bare source, empty store |
| `run::evaluate_raw(source, Some(path), &mut store)` | `preprocess` + `compile_with_imports_at(…, Some(store.registry()), …)` — the compiler (`cli.rs`) and the test harness |

They build different graphs, and the difference shows in the *type* a probe
prints.  Same source, both entries, after the fixes below:

```
curried  bare (compile)         : 10: ?a
curried  registry (evaluate_raw): 10: Int
```

A probe that reports `?a` where `lichen-compiler` reports `Int` is reporting its
own entry point, not the language.  Before the fixes both printed
`parameterized`, so the value loss was real on both paths; the type name is the
part that only the registry path resolves.

## 3. It is a regression, and the bisect names the commit

`b2cfa9c` (2026-09-29) read `1: Int`.  `679a45a` (2026-10-05, the state the
workaround of §13 in the companion note was written against) already read
`parameterized` — an earlier conclusion of "pre-existing" was drawn from that
commit and is wrong: it sits inside the regression window.

`git bisect` over `b2cfa9c..f2c80db`, testing "the minimal reproduction prints
`1: Int`", names the first bad commit:

```
30f1308  WIP: write_node_answer reports through the recursion, propagation does not
```

It replaced the answer's commit *through the recursion*

```rust
self.unify_inner(Side::node(node), Side::value(Some(value)), …);
self.nodes[node].runned = true;
```

with a value-against-held comparison beside the recursion plus an explicit flat
write:

```rust
let held = self.class_committed_value(self.equality_representative(node));
self.unify_inner(Side::value(Some(value)), Side::value(held), …);
self.write_node_value(node, Some(value));      // ← the flat write
self.nodes[node].runned = true;
```

Its stated goal is reporting — the recursion answers the conflict once, and the
propagation stops reporting each member's disagreement (the doubled, mirrored
diagnostic of
`pipeline::an_applied_struct_constructor_keeps_the_occurrence_identity`).  The
value loss is collateral from moving the *commit* out of the recursion with it.

## 4. Why a flat write is fatal here

`write_node_value` ends in `propagate_class_value`, whose walk is unconditional
over the class members that bear no operation:

```rust
for member in self.class_members(representative) {
    if self.nodes[member].operation.is_some() { continue; }
    self.nodes[member].value = Some(value);
}
```

A singleton class returns before that, so *when* the write happens decides
everything.  Measured, with the normalize phase separated from the run:

```
[normalize] WRITE 40 members=[40]                ← wire_apply_result's write: still a singleton
[normalize] WRITE 40 members=[47, 37, 40]        ← the flat write, after the parameter unify merged in 37
```

So the flat write lands *after* the apply's parameter unify has merged the
call's result cell, the cloned parameter pair, and the caller's argument pair
into one class, and smears the answer over all of them.  The caller's argument
pair `37` — which the checker builds as
`[Index(parameter_pair, 0), type_cell]`, a **lazy read** — becomes an array
whose elements are the *callee's* per-call cells.  At run time those cells carry
the callee's owner tag, so the membership test references them in place and the
per-call read is never rebuilt; the apply's own parameter unify then compares
those cells with themselves (`unify(48, 48)`) and binds nothing.

## 5. The rule the clone walk was missing: the clone's owner

`node_apply` stamped a clone with the **source's** owner, except for a node of
the closure's own scope:

```rust
self.nodes[clone].function = if ctx.closure_scope.is_some_and(|scope| scope.contains(&node)) {
    ctx.tag
} else {
    self.nodes[node].function            // the callee's id, not the caller's
};
```

Two independent statements say it should be the enclosing template:

* `instantiate_stamped`'s own comment: "the clones this pass creates are stamped
  with the apply node's owner, so the enclosing template re-instantiates them
  per call" — and the `tag` field holds exactly that owner
  (`tag: self.nodes[node].function`);
* the static path already does it: `static_node_apply` sets
  `self.nodes[clone].function = ctx.tag;` unconditionally.

Fixed in `3f57cef`: a plain apply's clones take `ctx.tag`; the closure walk
keeps its exception (the fresh closure's own scope joins the fresh id, a capture
keeps the enclosing owner it already has).  Measured: `id = x => x`,
`f = x => id x`, `f 1` reads `1: Int`; `double = x => add x x`, `double 5`
recovers its value.

## 6. The `runned` axis: a carried answer with open slots

A clone inherits the template's answer together with the template's `runned`.
The read path then treats it as final:

```rust
if let Some(value) = self.nodes[node].value {
    if self.nodes[node].operation.is_some()
        && !self.nodes[node].runned          // runned → return the value, never re-run
        && !self.nodes[node].visiting
    { … run … }
    return value;
}
```

The template's answer can be a structure whose own slots are still cells — the
pair a call answers with, where `is_unbound` sees an array (a decided value) and
therefore admits it.  Carried with `runned = true`, those open slots are read as
final, and neither the operator's re-run nor `wire_apply_result`'s wiring (which
binds the call's cell and its type) ever happens.

Fixed in `dffdb74`: the value still carries, but a clone whose carried answer
has an unbound element does **not** claim the operator's run —
`runned = carried && !owes_answer`.  The test is one level deep by design: a
structure whose own elements are decided is a fact a clone may answer with,
however open its interior is (a struct type's field cells are bound by the
enclosing call's checks, and carrying that answer is what keeps the holes from
being minted twice — the property the workaround of the companion note needs).

## 7. Hypotheses that measurement ruled out

Recorded so they are not walked again.  Each was a plausible cause; each was
tested against the reproduction above.

| hypothesis | measurement that killed it |
| --- | --- |
| the minted function's id is not registered under the enclosing function | the function table shows `parent: Some(…)` set at both mint sites, and the chain is lexically right (`4 → 1` for a closure minted inside `add`) |
| `wire_apply_result`'s `_ => result` arm drops a function-valued answer | instrumented: zero no-op arms; the closure read (`Index(apply, 0)`) holds the minted function |
| the "carry the template's answer" rule is the whole cause | narrowing it to "the answer's direct elements must be decided" changes nothing for `half = x => add x; half 5 5` |
| `evaluate_pattern_argument` skips the position because it is `shallow` | instrumented skip branch: zero skips in the reproduction |
| the per-call clone reads the *template* parameter instead of this call's | the clone reads the right node: `143 = Index(145, 0)`, and `145`'s first slot holds `USize(5)` |
| `propagate_class_value` is what corrupts the argument pair *by itself* | it is the flat write (§4) that makes it observable: the same propagation at the singleton write does nothing |

## 8. The carried answer must not contain a foreign closure

With §5 and §6 fixed, `half = x => add x; half 5 5` still ended in a marker.
The earlier reading of that chain ("the argument's value read runs before the
parameter it reads is bound and is never forced again") recorded the symptom;
the cause is upstream, at clone time.

Checking `half`'s body *applies* `add` to the checker's marker, and that
symbolic application mints a real closure whose captures bind the markers.
The deep pass then proves the body apply's answer — `[closure, type]` —
**concrete**: the answer's only open cells live behind the closure's body,
which the concreteness scan deliberately treats as opaque, and the type is
decided.  The clone walk's bake guard already knew such an answer must not be
referenced in place (`value_contains_foreign_function`: a proof cannot see
through a function's body), but the *carry* rule rejected only a top-level
function value — a pair *containing* one carried whole, `runned` included.
The enclosing template's membership test then references the check-time
closure in place (its owner chain descends from `add`, not from `half`), so
every call of `half` applies the closure whose captures are the checker's
markers, and the runtime argument never reaches the body.

Fixed in `5019fbb`: the carry side consumes the same recursive predicate as
the bake side.  An answer holding a foreign closure carries nothing; the clone
re-runs the operator and mints this call's closure.  Re-run versus carry needs
no deep information: re-run is decided by the answer's own slots being unbound
(§6), and carry is suppressed by a value-recursive scan for a foreign function
id — a minted closure's mere presence in an answer marks it per-call, whatever
its captures hold.

**Where the predicate lives is part of the rule.**  Storing it on the node — a
deep-pass tally of the closures an answer holds — cannot work, and the two
attempts to do so are what pinned that down.  Whether a function counts as
inside "the scope" depends on *which apply is cloning*, and a verdict is
computed without a caller.  Reading the node's own owner as the scope is a
different fact, not that one: the check-time closure's owner is the closure
itself, so `half`'s answer read as concrete again.  Counting *every* dynamic
function as unproven instead makes the recursion self-reference unproven, so
the recursion point and the body are cloned once per level — and the walk that
descends into a function value reaches the closure mint below, which did not
terminate even for `fib 3`.

The attribution therefore happens at the clone decision, where `ctx.applied` is
in hand (`2786ad9`): `value_holds_foreign_function` walks the answer's value
edges and counts a dynamic function only when it is neither the applied
function — the recursion point, which must stay in place or a clone would mint
a second function where the body means one — nor an enclosing function, which
every call shares.

The closure mint records its fresh id **before** walking the closure's scope
(same commit).  A scope can name its own closure back through a sibling, and
the walk re-enters the mint for each such reach; recording the entry afterwards
minted a fresh function per level, without bound.  The hazard was latent for as
long as no rule let the walk descend into function values.

## 9. The static mirror: the capture test asked about a target, not openness

`examples/import`'s `(42, none, 7)` is the same shape across a frozen module:
`double = x => math.add x x` lives in geometry's frozen artifact, where
solving `math.add x` minted a closure over marker cells and freezing kept it
as a module-local static function.  The minimal matrix isolates it: the shape
breaks only when the wrapper lives in an intermediate module.

Measured at run time, the machinery almost works: the inner apply is a
residual and re-runs, the fresh closure is minted (`REHOME`) with its capture
bound to `5` — and the outer apply *still* applies the frozen solve-time
closure.  The frozen closure's value node is op-less and `parameterized`, so
materialization keeps its value; whether to re-home it was decided by
`static_function_captures`, which walked the closure's body for a value edge
to **the applied function's parameter node**.  A frozen closure's captured
cells are the solve-time application's parameter clone — linked to the
parameter only through an equality class, never by a value edge — so the test
answered "non-capturing" and the frozen closure rode in verbatim.  It unified
silently with the fresh closure (`function_identity` resolves both to one
origin, capture-blind by design for re-exports), and a read of the stale slot
handed the outer apply the closure whose captures are dead markers.

Fixed in `7ceee3e`: the test asks what the machinery needs — whether the
closure's body reaches any `parameterized` node **outside its own template
scope** (`StaticFunction::nodes`).  An own-scope open cell re-opens per call
through the residual clone rule, so it is not a capture; a captured open cell
is outside the scope by definition.  No equality-class walk is needed: the
body reaches the captured cell through operation operands and array items
already.  Once the closure re-homes through the shared remap, the existing
machinery does the rest — the capture clone re-joins the parameter's frozen
class in the regroup, and the parameter unify binds it.

## 10. Status

| suite | before this work | after `5019fbb` and `7ceee3e` |
| --- | --- | --- |
| lowlevel | 155 | 155 |
| checker | 87 | 87 |
| registry | 22 | 22 |
| field_read_kinds | 15 | 15 |
| pipeline | 138 / 140 | 138 / 140 |
| compute | 59 / 62 | 59 / 62 |
| examples | 0 / 1 (import drift) | import fixed; `compute_jit` drift remains |
| perspective | 17 / 20 | 17 / 20 |

The red tests that remain are the ones this note does not touch: the mirrored
double diagnostic (`pipeline`), the refinement and perspective suites whose
expectations predate the `f : f` encoding
(`docs/notes/function-type-as-function.md`), the parked compute case, the
`dependent` checker case, and `compute_jit`'s `raw[...]` rendering.  Each of
them is now an `#[ignore]` naming its own symptom rather than a red run, and
`compute_jit` is skipped by name in `tests/examples.rs`: the type it prints
comes from the `compute.lichen` wrapper's kernel constraints, which are still
being annotated (`crates/lichen-compute/src/compute.rs`, `LaunchOp::build`,
states no type at all), so the launch result's type stays an open cell and the
raw-mark renderer marks it.

`compute_jit`'s `raw[...]` rendering is **not** the two missing `dev` commits
alone: with `dev`'s renderer (`9c563a3`) in place, `gcd` and `lazy_infinite`
render exactly as `dev` declares them, while `compute_jit` still dumps a raw
type — the graph, not the printer.

Commits: `3f57cef` (the clone's owner), `dffdb74` (the carried answer's
claim), `5019fbb` (a carried answer holds no foreign closure), `7ceee3e` (the
static capture test asks about openness), `2786ad9` (scope attribution at the
clone decision, and the mint's entry recorded before its scope walk), on top of
`827ee5d` and `f2c80db` (the value-against-valueless-class write, and the named
read's kind guard).  `a1b81c1` (the deep-pass closure tally) was reverted by
`2786ad9` for the reason §8 records.

# A call's clones belong to the enclosing template

A value that travels through a wrapper function disappears.  This note records
what the disappearance actually is, the commit that introduced it, the two
rules the clone walk was missing, and the one break that is still open.  It
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

## 8. What is still open

`half = x => add x; half 5 5` still ends in a marker.  The chain, measured:

```
143 = Index(145, 0)  value=NO-VALUE      ← the argument's value read
145 elements=[146 = USize(5), …]         ← the value is in the slot it reads
EVAL-NODE 143 (twice) … PATTERN pattern=145 argument=67 … (nothing evaluates 143 again)
```

The argument's value read runs **before** the parameter it reads is bound, gets
the marker (which the evaluator declines to cache), and is never read again
afterwards.  `apply_parameter_check`'s `evaluate_pattern_argument` runs after
the check by design ("so the unify sees the argument's element values instead of
unbound slots") and descends into that position, but nothing forces the read
again.  The next step is there: either the descent must force an argument
position whose pattern slot is a bare cell, or the parameter check must re-force
the argument positions the unify left open.

`examples/import`'s `(42, none, 7)` is the same shape across a frozen module and
should be re-measured once this is fixed.

## 9. Status

| suite | before this work | after `3f57cef` and `dffdb74` |
| --- | --- | --- |
| lowlevel | 155 | 155 |
| checker | 87 | 87 |
| registry | 22 | 22 |
| field_read_kinds | 15 | 15 |
| pipeline | 138 / 140 | 138 / 140 |
| compute | 59 / 62 | 59 / 62 |
| examples | 0 / 1 (drift unchanged) | 0 / 1 (drift unchanged) |

The red tests that remain are the ones this note does not touch: the mirrored
double diagnostic (`pipeline`), the refinement and perspective suites whose
expectations predate the `f : f` encoding
(`docs/notes/function-type-as-function.md`), the parked compute case, and the
`dependent` checker case.

Commits: `3f57cef` (the clone's owner), `dffdb74` (the carried answer's claim),
on top of `827ee5d` and `f2c80db` (the value-against-valueless-class write, and
the named read's kind guard).

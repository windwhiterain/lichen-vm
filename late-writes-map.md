# Late writes under a plain deep gate — the elimination map

> Slice: measurement only. **No behaviour change lands here.** The gate below is a
> scratch edit that is reverted before the commit; what this file holds is the
> per-test trace of every write that arrives *after* the deep verdict, the site
> that wrote the verdict, and the sanctioned site that should have produced the
> write instead.
>
> Worktree: `.worktrees/dlw`, branch `feature/deep-late-writes`, base `dev@f360c72d`.
> Build: private target `C:\resource\lichen-vm\.worktrees\dlw\target`, `-j 1`,
> `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`.

## 0. The goal, in the maintainer's terms

`unify` is separated from `evaluate`: the checker does the initial unify
(`expr: expr`), normalization happens at the moment a function is born, apply does
runtime unify, and **beyond those three sites no unify is produced dynamically**.

The payoff wanted: `evaluate` / `evaluate_deep` gate purely on their stamps —
`evaluated` = the value-epoch stamp, `evaluated_deep.is_some()` — so **a node is
evaluated and decided exactly once, never re-decided**.

The `settled` bit of [incremental-evaluation](docs/notes/incremental-evaluation.md)
§4 was **rejected** ("加了 `settled` 等于没重构"). So the late writes are defects to
eliminate at their source, not state to be tolerated.

## 1. The gate under experiment

The experiment: give the deep walk the plain cut that the rejected `settled` design
would have given, but with the gate reading **only** `evaluated_deep.is_some()` —
no second bit, no new state.

Inserted in `crates/lichen-lowlevel/src/evaluation.rs`,
`evaluate_node_deep_inner`, after the static-leaf and cycle-cut arms and before
`deep_depth` is charged (the position
[incremental-evaluation](docs/notes/incremental-evaluation.md) §4.4 names):

```rust
// SCRATCH: the plain deep gate.
if self.nodes[node].evaluated_deep.is_some() {
    return self.nodes[node].value;
}
```

This is the whole of the "decided exactly once" property: once a node holds a
verdict, the deep walk never re-enters it, so no verdict is ever revised.

## 2. The single choke point for the trace

Every value-slot write in the module goes through one function,
`Module::set_node_value` (`crates/lichen-lowlevel/src/equality.rs:287-290`), which
bumps the value epoch and then assigns `Node::value`. Its callers, by
[stalled-rerun-epoch](docs/notes/stalled-rerun-epoch.md) §4, are exactly:

| caller | `file:line` | what it is |
|---|---|---|
| `write_node_value` | `equality.rs:257` | the public slot write; also calls `propagate_class_value` |
| `write_node_value_if_absent` | `equality.rs:293` | the same, refusing an occupied slot |
| `write_node_answer` | `equality.rs:319` | an operator's computed answer, unified against the class |
| `propagate_class_value` | `equality.rs:345` | one value given to **every** member of a class |
| `fill_class_holes` | `equality.rs:111` | the `add_equality` merge giving the value to empty members |
| `relocate_node_value` | `equality.rs:307` | GC's move of a value, not a change of it |

So the trace point is `set_node_value`: a write to a node that already holds
`evaluated_deep.is_some()` **is** a late write, by definition, and the trace records
which of the six callers it arrived through.

## 3. Results

*(filled in below as the runs land)*

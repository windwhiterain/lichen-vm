# What closing the depth mark's type cell actually does

> **Status:** the mechanism [shallow-depth-closure](shallow-depth-closure.md) §6.1 item 1
> left open — *"the mechanism is not established … anyone landing this should expect to
> re-derive it"* — re-derived here **by measurement**, not by reading §3's prediction
> forward. §3 predicted that no closure over types could supply the missing **value**,
> because the closure acts on a **type**; §3 was wrong, and this note says which of the
> two halves of the closure does the work and why.
>
> Measured in the worktree `.worktrees/shhx` on `feature/shx`, from `dev` at `d2f280c4`,
> re-verified on `dev` at `7323cf6b`, through `lichen-language`'s own `testing::evaluate`
> (the faithful runner for a runtime value, per [test-harness](test-harness.md) §"Verify")
> with an instrumented `crates/lichen-highlevel/src/checker.rs` shallow walk. **Every
> probe was reverted before any commit**: an `eprintln` in the `n` arm of
> `check_shallow_array_term` and in the walk at
> `crates/lichen-highlevel/src/checker.rs:557-562`, and a
> `crates/lichen-language/tests/language/zz_shx_probe.rs` that printed each element
> node of the root tuple. No test was added.
>
> Companions: [shallow-depth-closure](shallow-depth-closure.md) §§1.1–§1.3 and §5, the
> measurement this completes — **not restated here**; [index-read-unify](index-read-unify.md)
> §7.5, the bare-form closure this extends.

## 1 The node the closure acts on

The depth arm registers `(type_apply, self.state[el].ty)` — the call's **own declared
type node**, which is also, by `wrap_shallow`'s construction, the **type slot of the
marked pair**. For the one program in the tree
(`crates/lichen-language/tests/language/pipeline.rs:2426`) the probe printed:

```
PROBE n arm: el=ExprId(6) depth=1 ty=Some(NodeId(35)) type_applies=true wrapped=NodeId(43)
             wrapped_items=[(NodeId(42), true), (NodeId(35), false)]
```

So the marked node `43` is the pair `[42, 35]` `wrap_shallow` builds — `42` is the call's
value and is the **shallow-marked** position, and **`35` is the type slot**, unwrapped and
undecided. The cell the closure is pointed at and the cell a read of the mark must go
through are **the same node**. That identity is the mechanism.

## 2 Which half of the closure does the work

The walk at `crates/lichen-highlevel/src/checker.rs:557-562` does two things per
registration: `evaluate_node_deep(type_apply)` and `close_shallow_tail(tail)`. The probe
ran the same program three ways, each removing one of them:

| variant | `tail=NodeId(35)` at entry | after the walk | after `close_shallow_tail` | `an_at_shallow_depth_one_on_a_recursive_tail_terminates` |
|---|---|---|---|---|
| neither (`dev`) | `op=false val=false` | — | — | **FAILED** |
| registration + `close_shallow_tail` only | `op=false val=false` | — | `op=false val=false` | **FAILED** |
| registration + `evaluate_node_deep` only | `op=false val=false` | `op=false val=true` | `op=false val=true` | **ok** |
| both (`SHX_PROBE=full`) | `op=false val=false` | `op=false val=true` | `op=false val=true` | **ok** |

**`evaluate_node_deep(type_apply)` is the whole of the effect; `close_shallow_tail`
contributes nothing, and cannot here.** Its guard is `shape::field_type(&module, tail, 1)`,
which reaches a field list through the node's shape; the tail cell has **no operation**
(`op=false`) and therefore no shape, so `close_shallow_tail` returns at its first `else`.
That `after-close` is byte-identical to `after-evaluate` is the measurement of it.

The walk **decides the cell**: `val=false` → `val=true`, holding a two-element array —
the tuple type `[shape, kind]`. The type application *is* the operation that computes the
call's result type, so deep-evaluating it fills in the very cell the depth arm left open.
This is why §3's prediction failed: §3 read the closure as a **type** operation and looked
for a **value**, and the causal line runs through the cell being *occupied*, not through a
value being computed.

## 3 What the missing value was

The test's helper demands `module.node_value(element)` per element
(`crates/lichen-language/src/testing.rs:129-132`). The probe printed the root tuple's
element nodes directly:

| | `dev` | with the registration |
|---|---|---|
| `[0]` = `inf(1)(0)` | `[USize(1), <lazy pair>]` | `[USize(1), <lazy pair>]` |
| `[1]` = `inf(1)(1)(0)` | **`NodeId(124)` has no value** | a two-slot type structure |
| `[2]` = `inf(1)(1)(1)(0)` | **`NodeId(177)` has no value** | `TypeValue(TypeTuple)` |

An empty result, not a `raw` marker and not an error: with `35` empty the marked pair
`43 = [42, 35]` has **no shape**, so a projection through it selects no slot and yields
**no value at all** — and `node_value` answers `None`. Once the walk fills `35`, the same
projection resolves and every element node comes to hold a value, so `array_values` stops
panicking and the test reaches its assertion.

## 4 What this does **not** establish

1. **The reads are still not the program's arithmetic answer.** The three elements hold
   `[1, <lazy pair>]`, a two-slot **type** structure, and the `TypeTuple` **kind marker**,
   where the bare-mark control on the same program holds `USize(1)`, `USize(2)`,
   `USize(3)`. The change restores *a value per element* — which is all the test asserts,
   `.len() == 3` — and not the numbers. Whether the depth mark *should* answer `2` and `3`
   is [shallow-depth-closure](shallow-depth-closure.md) §6.3's open question, and this
   note does not settle it: what the values *are* is a deeper question than the one this
   slice was asked.
2. **The two spellings still differ in what their reads return**, and P1 narrows the gap
   without closing it: it makes the depth mark's tail cell decided like the bare mark's,
   but the bare mark's element slot is the call's own pair and the depth mark's is a
   rebuilt one, whose resolved reads come off the type side.
3. **`close_shallow_tail` remains dead code for `@shallow(n)`.** This measurement is the
   evidence that the *registration* is what the depth form needs; the closure that reads
   the name is still a structural no-op on an op-less cell.

# Which write fills a `Buf` element cell? — measurement

Worktree: `C:\resource\lichen-vm\.worktrees\classfill`, branch `feature/classfill`, base `dev` @ `fc0761c4`.

The question the brief asks: **in the passing program, which write fills the element cell that
`walk_role` reads?** Everything here is measured. The probe and the `#[track_caller]` recorder that
produced the measurement were **removed before this note was committed** — the investigation they served
is closed — so §2's *What proved it* names every write side and the trace positions, which is what makes
the measurement re-derivable. The defect itself stays pinned by the ignored assertions in
`crates/lichen-compute/tests/compile_order_class.rs`.

## 1. The cell is not the element node — it is that node's pair **value half**

`crates/lichen-compute/src/compute.rs:2013-2017` reads

```rust
parameter_type_ref(module, field)
    .and_then(|ty| field_type(module, ty, 1))
    .map(|element| low_type_of_slot(module, element))
```

For `attend`'s `.v (compute.Buf Float)`, `field_type(..., 1)` is `NodeId(2404v1)`, and that node
**holds its value in both programs**. The probe prints it:

| program | `2404` cell | `2404` class | `2405` (pair item 0) cell | `2405` class | decoded |
|---|---|---|---|---|---|
| CONTROL (passing), at `attend`'s walk | `Array(..)` | `Array(..)` | `Some(TypeValue(TypeFloat))` | `Some(TypeFloat)` | `Float` |
| STEP (failing), at `attend`'s walk | `Array(..)` | `Array(..)` | **`None`** | **`None`** | `Unknown` |

So the lazily-filled cell is `NodeId(2405v1)` — the **value half** of the element type's
`[value, type]` pair — reached by the pair indirection at
`crates/lichen-highlevel/src/shape.rs:866-876`. `low_type_of_slot` reads the **cell**
(`module.node_value`, `shape.rs:862`), and the cell is empty.

**This also refutes the class route for this defect, with a measurement**: at the failing read
`class_value(2405)` is `None` too (probe line for `buf path=[3, 1]`). The class is *not* decided
yet, so "read the class instead of the cell" cannot work here — consistent with the brief's note
that `class_value`/`class_low_type` were empty.

`.s (compute.Buf Float)` in the same `QSV` reads `Float` even in the failing program, because its
value half `NodeId(1449v1)` was already filled (probe: `FILL NodeId(1449v1)`, trace position 4631
of the STEP run, well before `compile_parallel_fragment` #0 at 8384). That is why the failing
fragment is `in [Float, Int]` and not `in [Int, Int]`.

## 2. The write that fills it

**`Module::fill_class_holes`, `crates/lichen-lowlevel/src/equality.rs:111-118`, called from
`Module::add_equality` at `equality.rs:98-100`.**

```rust
// equality.rs:89-102, Module::add_equality
let representative = disjoint::union(&mut self.nodes, a, b);
...
if let Some(value) = left_value.or(right_value) {
    self.fill_class_holes(representative, value);   // <- equality.rs:99
}
```

`fill_class_holes` writes `Some(TypeValue(TypeFloat))` into every class member whose slot is
empty — including `2405`. The class is one large union-find class of 35+ `Float` type cells
(representative `NodeId(448v1)` in CONTROL, `NodeId(1433v1)` in STEP — the same member set, a
different union winner).

`add_equality` here is reached from `unify_inner`'s **"one side knows a value"** arm:

```rust
// equality.rs:722-727
(Some(_), None) | (None, Some(_)) => {
    // The merge already gave every member the value the deciding side kept.
    self.add_equality(ra, rb);   // <- equality.rs:725 (728 after probe instrumentation)
    true
}
```

### What proved it

A `#[track_caller]` recorder on **every** write side of `Node::value`, with a single ordered log
shared with the reads:

| site | instrumented |
|---|---|
| `Module::add_node` | `module.rs:116-138` (allocation with a concrete value) |
| `Module::write_node_value` | `equality.rs:281` (covers `write_node_value_if_absent`, `write_node_answer`) |
| `Module::propagate_class_value` | `equality.rs:369` |
| `Module::relocate_node_value` | `equality.rs:331` |
| **`Module::fill_class_holes`** | `equality.rs:111` — **the writer** |

plus a counter at the top of `compile_parallel_fragment` and one `note_read` per `walk_role`
buffer leaf, so every read is attributed to the kernel compile it belongs to.

Running it on both programs (probe: `cargo test -p lichen-compute --test classfill_probe -- --nocapture`):

```
CONTROL (root is `(scores, attend)`, passing)
  7350  == compile_parallel_fragment #0  fid=FunctionId(1v1)  cfg_pair=NodeId(142v1)   <- scores
  7355      -- after lower_index_function
  7422  MERGE NodeId(2405v1)(decided=false) ~ NodeId(5262v1)(decided=true)
         unify stack: ["crates\lichen-lowlevel\src\apply.rs:139"]
  7447  MERGE ... unify stack: ["crates\lichen-lowlevel\src\apply.rs:78"]
  7854  == compile_parallel_fragment #1  fid=FunctionId(3v1)  cfg_pair=NodeId(1135v1)  <- attend
  7856    buf path=[3, 1] element_type_node=2404v1 element_class=Some(Float)

STEP (has `built = compute.graph step`, failing)
  7961  == compile_parallel_fragment #0  fid=FunctionId(3v1)  cfg_pair=NodeId(1135v1)  <- attend
  7965    buf path=[3, 1] element_type_node=2404v1 element_class=None
  8736  == compile_parallel_fragment #1  fid=FunctionId(1v1)  cfg_pair=NodeId(142v1)   <- scores
  8865  MERGE NodeId(2405v1)(decided=false) ~ NodeId(6384v1)(decided=true)
         unify stack: ["crates\lichen-lowlevel\src\apply.rs:139"]
```

**Same write, both programs. The only difference is when it runs.** In CONTROL it runs after
`scores` compiles and before `attend` compiles; in STEP it runs after `attend` has already read.

Both programs also show the *other* writers, which are **not** the answer:

* `write NodeId(2405v1) <- crates\lichen-lowlevel\src\static_module\apply.rs:140` — this is
  `static_node_apply` (`static_module/apply.rs:98-162`), the apply-clone of the frozen `Buf`
  field-list nodes. It writes **`None`**: `static_module/apply.rs:135-140` maps the frozen
  node's value only `if carried`, and a definition carries no answer (the carry rule,
  `apply-clone-ownership.md` §2). So the clone *arrives empty*.
* `seed_template_term_low_types` / `infer_template_low_types` (`compute.rs:2416-2417`) run
  **after** the role walk at `compute.rs:2403`, and the probe's stage markers (`-- after
  seed_template_term_low_types`, `-- after infer_template_low_types`, `-- after
  lower_index_function`) show neither contains the merge.

## 3. What has to run before `compute.rs:2403`

The merge is `Module::unify` on the kernel's cloned parameter, and the `unify` stack names the
caller: **`crates/lichen-lowlevel/src/apply.rs:78`** (`apply_parameter_check`) and
**`crates/lichen-lowlevel/src/apply.rs:139`** (`wire_apply_result`) — i.e. the VM *applying a
function*, whose parameter unify descends the struct type and merges the element cell into the
decided `Float` class.

So the answer to "what would have to run before `:2392`" is: **an application whose parameter
unify descends this kernel's parameter type** — the one `compute.parallel` already has in hand as
`operands[0].node` (`crates/lichen-compute/src/compute.rs:1170-1177`, just before
`compile_parallel_fragment(module, function)` at `compute.rs:1178`).

**Not implemented here**, and both reasons are measured rather than guessed:

1. **The class route is empty.** At the failing read `class_value(2405)` is `None`, so reading the
   class instead of the cell cannot work (§1).
2. **Forcing the operand does not re-run the apply.** Measured: inserting
   `module.evaluate_node_deep(operands[0].node, None)` immediately before the
   `module.node_value(operands[0].node)` read at `compute.rs:1170` changes **nothing** — both
   ignored assertions fail with exactly the same `in [Float, Int] out [Int]`. The operand node
   already holds its `Function` value, and a node that holds one is not re-run, so the parameter
   unify that would fill the class never happens again. This is the carry rule
   (`static_module/apply.rs:130-140`, `apply-clone-ownership.md` §2) doing exactly what it says,
   and it is the same reason the brief's earlier `evaluate_node` attempt had no effect.

That last point is the real shape of the defect and the next step: **the class is filled by a
unification that the language's own carry rule skips.** A fix has to either make the parameter's
element class decided without waiting for that unify — for example by having the checker, which
builds the `Buf` type term and knows the declared element, seed `Float` into the element value
half's class at construction rather than leaving it to a later unification — or by having
`compile_parallel_fragment` refuse (`RoleUnread::Undecided`) instead of falling through to
`ScalarClass::Int`, which is a behaviour change to `compute.rs:2424-2430` and would need its own
acceptance. **Neither was implemented or measured here.**

## 4. Files

* `crates/lichen-compute/tests/classfill_probe.rs` — the probe; prints, asserts nothing.
* `crates/lichen-lowlevel/src/probe_write.rs` — the recorder (temporary).
* `equality.rs`, `module.rs`, `lib.rs`, `compute.rs` — instrumentation only.

## Reproducing

```
cargo test -p lichen-compute --test classfill_probe -- --nocapture            # CONTROL
$env:PROBE_CASE='step'; cargo test -p lichen-compute --test classfill_probe -- --nocapture
```

The probe's instrumentation is temporary and is **not** part of the fix; it exists so the numbers
above can be re-derived.

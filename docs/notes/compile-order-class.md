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

Running it on both programs (the recorder §5 describes how to rebuild):

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
unification that the language's own carry rule skips.** The route this note first proposed — have the
checker, *which builds the `Buf` type term and knows the declared element*, seed the class at
construction — **is wrong, and measuring that was the next slice's first result**: the checker does not
build the term. It builds **one node** for the `compute.Buf Float` application
(`crates/lichen-highlevel/src/checker/lambda.rs:316`) and it *does* know the class there (the argument's
value node is the shared `Float` node and its cell holds `TypeValue(TypeFloat)`), but the type **term** is
materialised later by the VM — `static_function_apply` → `static_node_apply`
(`crates/lichen-lowlevel/src/static_module/apply.rs:112`) **cloning the frozen `compute.lichen`
template** — and the cell `walk_role` reads is **one of those clones**. So **no site both knows the class
and holds the cell**; they are in different crates and different phases. Two seeding experiments were
built and discarded: putting a `Parameter` element's own binding value cell in its slot's value position
(`crates/lichen-highlevel/src/checker/tuples.rs`) does fire
(`kind=Parameter value=Some(NodeId(290v1))`) but leaves the cell empty at the first walk, and evaluating a
literal argument at check time has no effect (the literal's value node already held `TypeFloat`).

**The measured missing fact**: at **every** `compute.Buf` application (`applied = StaticFunctionId(22)`),
`apply_parameter_check` sees `param = [..=None, ..=None]` and `arg = [..=None, ..=None]` — **the binding
the application exists to make never happens there**. The class arrives only when the kernel's own
parameter unify later descends the struct type and merges the cell into the decided `Float` class. A seed
and that merge *could* disagree, and the machinery has an answer for it — `write_node_answer`/
`unify_inner`'s `(Some, Some)` arm calls `record_error(ra, rb, …)`, a recorded unify error naming both
nodes — but with no seeding site reachable that is untested.

So the two honest candidates are both **lowlevel** changes to the carry rule or the apply ordering, and
neither was chosen: **(i)** carry the parameter binding into the `.element` slot's value position **at
clone time** in `static_node_apply`, or **(ii)** make the `Buf` application's argument **decided before
its `apply_parameter_check`**. Both need a decision that this note does not make.

## 4. One half of that did land: an undecided class is refused, not guessed

`compile_parallel_fragment` now returns `RoleUnread::Refusal` naming what it could not read
(`crates/lichen-compute/src/compute.rs:2414-2432`) instead of falling through to
`unwrap_or(ScalarClass::Int)`. **Its blast radius was measured as zero**: A/B against the commit reverted,
over the whole `lichen-compute` suite, the runs are identical, failing exactly the six pre-existing
`lowlevel` binaries with the same counts. No legitimate program hit the refusal, so it was neither
narrowed nor widened. One sentence changed to say what the refusal now covers —
*"A named refusal: the parameter is not a kernel ABI, **or a class it declares is not decidable**, and
this is why."* (`compute.rs:1899-1900`); the `RoleUnread` invariant block itself needed no change.

**It does not flip the pin above, and that is the interesting part**: at the failing read a write *is*
found, so the default was never what produced the `Int` here. The refusal is a net for the *other* sites
where an undecided class used to become `Int` silently — not a fix for this defect.

## 5. Re-deriving the measurement

The probe and the `#[track_caller]` recorder that produced §1–§3 were **not committed**: their
investigation is closed, and the defect stays pinned by the ignored assertions in
`crates/lichen-compute/tests/compile_order_class.rs`. To re-derive them, instrument the five write sides
of `Node::value` named in *What proved it*, share one ordered log with the `walk_role` reads and a
per-`compile_parallel_fragment` counter, and run the two programs — the same write appears in both, only
its position differs (CONTROL 7422/7447 against a compile at 7854; STEP 8865 against 7961).

# Compute replay handoff

> Status: **field provenance is implemented and measured.** The launch replay
> fix is landed; the parallel read source chain now uses explicit per-instance
> field links. Lowlevel and language are green, and compute is **124 passed / 116
> failed**. Root materialization, graph values, and definition-cycle failures
> remain open. Undecided actual field values are preserved instead of being
> displaced by decided requirements.
>
> Read [value-templates](value-templates.md#constructor-fields-keep-their-source)
> for the field-source contract, [replay-apply](replay-apply.md) for replay, and
> the measurements below for rejected alternatives.

## 1. Objective and current readings

The task is to repair `lichen-compute` without changing the language's ruled
clone semantics.

At `dev@554560d2`:

- `cargo test -p lichen-lowlevel --test basic`: **164 passed, 1 ignored**.
- `cargo test -p lichen-language --test language`: **275 passed**.
- `cargo test -p lichen-compute --test lichen_compute`: **113 passed, 127 failed,
  1 ignored**.
- The four-line scalar launch reproduction answers **`43: Int`** (it was
  `none: Int` before `554560d2`):

  ```lichen
  ---
    compute = import "compute.lichen"
  ---
  a := 42
  k := compute.jit (x => x + a)
  compute.launch k 1
  ```

The compute suite started this investigation at 78 passed / 162 failed. The
large regression was bisected to `14285cd8`, the class-wide scope verdict;
earlier readings were 112/128 before it and 67/173 at it.

## 2. Rulings that must not be re-litigated

- **The clone verdict is class-wide.** A class is cloned only when every member
  lies inside the scope being instantiated. An external class is referenced as
  a whole. Do not replace this with a per-node ownership verdict.
- **A clone and its template must not be unified.** Undecided cells are linked
  to a replay of the apply instead. When the apply's scope is cloned, `runned`
  is reset and the replay evaluates against the actual argument.
- **Apply is unified.** Do not explain behavior in terms of two independent
  cloned pairs called "the type-apply pair" and "the value-apply pair".
  `ParameterCheck` is a mode of one apply path, not evidence of two applies.
- **Native read/write are ordinary operators.** There is no native-vs-builtin
  semantic distinction that excuses a missing clone or replay. `$jit` can
  compile, and an ordinary `Apply` can fail for the same scope reason.
- For native code that **inspects a function's internal construction**, the
  intended source is the apply's **cloned return**, with the **actual
  placeholder argument** as the old parameter. This is not a license to change
  launch/plrun evaluation semantics.
- Do not change an `# Invariant`, the class-wide clone rule, or a decided
  `wontfix:D` item without the maintainer's permission. Check
  [code-audit](code-audit.md) before calling a measured behavior a defect.

## 3. What `554560d2` changed

`crates/lichen-lowlevel/src/function.rs` now handles an external operation
return through replay:

- An internal operation still runs normally.
- An external operation return triggers replay even when there are **zero plain
  open cells** (`open=0`). Previously that returned before a replay existed.
- The replay instance performs `ParameterCheck::Value` for this case, binding
  the placeholder argument.
- The original external-operation result links to the replay instance's
  `applied` return. Plain open results keep the old replay-class trigger.
- During replay, an operation can be rebuilt when its membership is the owner
  chain **or a hit in `Function::nodes`**. This is what let the unowned static
  launch operation back into its minted closure.
- Ordinary apply remains class-wide; the operation exception exists only in
  `self.replaying`.

The important launch trace before the fix was: parameter `599 -> 682`, return
shell `594 -> 685`, external launch operation `595` referenced in place, then
no clone for argument read `597`, so launch read an empty template argument.

The same change repaired both the scalar probe and 35 additional compute tests.
It did not repair the parallel read-position family.

## 4. Current open problem: parallel read provenance

Use this minimal compile-only reproduction:

```lichen
---
  compute = import "compute.lichen"
---
In := _x => {.d (compute.Buf (compute.BufParam{.type Float, .length 1}))}
OutE := _x => {.e (compute.Buf (compute.BufParam{.type Float, .length 1}))}
Par := _x => {.n @any : Int, .in In _}
raw := (lane <: Par) => {
  e := compute.buf (compute.BufParam{.type Float, .length lane.n})
  ((OutE){.e (compute.write (compute.Write{.to e, .at 0, .value (compute.read (compute.Read{.from lane.in.d, .at 0}) : Float)}))})
}
k := compute.parallel raw "cpu" (OutE _)
k
```

It still reports:

```text
compute.parallel: read's buffer argument is not an input buffer of the parallel parameter
(the parameter's inputs are [[1, 0]])
```

The measured boundary is now narrower than "native operators do not replay":

- `lane.in.d` reaches lowering through `Index(Apply, 0)` and nested `Read`/`Buf`
  constructor wrappers.
- A fresh `Buf` return and the applied `.in.d` field are not one equality class,
  and they must not be unified merely to make the position check pass.
- The read seen by `parallel_buffer_pos` is still a template node. One measured
  example was node `1283`, with payload parent `1282`; replay did not visit
  either `1282` or `1283`. These IDs are probe-specific, not stable fixtures.
- Therefore the remaining question is **how Lower resolves a replay clone's
  view/origin to the corresponding instance position**, not whether `runned`
  resets and not whether native read/write are operators.
- The main remaining failure families are the parallel input-position refusal,
  graph return/value-table failures, and empty kernel-body nodes. There are
  also a few definition-cycle, index-placement, and struct-conflict failures.
  Do not treat all 127 as one root cause.

Relevant code:

- `crates/lichen-compute/src/compute/body.rs`: `Lower::buffer_read`,
  `Lower::parameter_path`, `peeled_argument` usage, and `define_in` / view
  handling.
- `crates/lichen-compute/src/compute.rs`: `parallel_buffer_pos`, `param_path`,
  `compile_parallel_fragment`.
- `crates/lichen-lowlevel/src/function.rs`: `node_apply`, `link_replay`,
  `value_apply`.
- `crates/lichen-lowlevel/src/resolve.rs`: `define_in`, `selection_of`, and
  `Selection::Views` recursion.

## 5. Experiments already measured — do not repeat blindly

| Experiment | Reading / reason not to keep it |
|---|---|
| Clone every own operation whenever its class is external | Compute **116/124**, lowlevel/language green, but splits the external class and leaves the dynamic `choose` control failing. Not landed. |
| Extend frozen outer-instance regroup beyond plain cells | Classes really re-unified, but launch stayed `none`; removing it left the 116/124 experiment unchanged. Not the root fix. |
| `$jit` wrapper applies `f` to a placeholder and native inspects the cloned return | `$jit` compiles. This validates the native-inspection idea only; launch execution and parallel provenance are separate. |
| The same pair for `$parallel`, without replay | **73/167**. The read still had no corresponding instance. Reverted. |
| Raw apply container instead of `applied_exprs` cloned return | No improvement over the cloned-return form. |
| Pre-apply a launch runner, then return `x => {x = a; O}` | Still `none`; the computation return is met before the actual argument binds the runner placeholder. |
| Explicit native replay plus native pair | **107/133**, worse than `554560d2`'s 113/127. Reverted. |
| Recursively replay every nested `Apply` | Minimal read refusal unchanged. Reverted. |
| Broaden replay to every operation/open nested cell | Caused lowlevel recursion hangs and `expected Struct, found Struct` package failures. Reverted. |
| Treat an open Struct return as decided/undecided by changing return guards broadly | Package unification conflicts. Do not broaden without a precise correspondence rule. |
| Clone dynamic-reference array/table payloads only during replay | `1282`/`1283` still were not visited. The fast-path payload hypothesis is not the complete cause. Reverted. |
| `Read`/`Write` use `p` directly instead of `(Buf p)` | `BufParam` lacks native and rejects. `p <: Buf` avoids that refusal but did not fix the representative read failures. Reverted. |
| Change `applied_parameters` to prove a different apply generation | Generated misleading pass-based explanations and no repair. Do not reintroduce it as evidence. |

An independent review suggested the payload fast path, but the subsequent
`1282`/`1283` entry trace showed that those nodes were never reached. Treat
"the payload was shared before recursion" as a hypothesis unless a new trace
proves the exact early return.

## 6. Measurements that located the source gap

Start from `554560d2`, not from any experiment above.

1. Recreate the minimal parallel probe and instrument `Lower::buffer_read` and
   `parallel_buffer_pos`.
2. At the read, print the node, its operation and operand, `node_origin`, class
   root, the parameter field root, and each view hop. Follow the hop that first
   takes the resolver from the replay/applied graph back to the template node.
3. Distinguish these two cases before changing code:
   - the replay graph contains a corresponding clone, but `peeled_argument` or
     `define_in` discards it for the template original;
   - no corresponding clone exists, so the missing correspondence is in apply
     replay rather than Lower.
4. Only then choose the smallest repair. If a correspondence table is needed,
   persist the actual apply remap; do not manufacture equality between a fresh
   constructor result and an input field. If external members were never
   cloned, a remap alone will not be enough.
5. Re-run the minimal probe, the representative `silu` and
   `parallel_read_input_buffer` tests, then all three suites.

Useful exact filters:

```bash
cargo test -p lichen-compute --test lichen_compute \
  lichen_compute::silu_exp_nested::an_inlined_transcendental_below_the_marks_root_still_reads_as_the_operator -- --exact

cargo test -p lichen-compute --test lichen_compute \
  lichen_compute::compute::parallel_read_input_buffer -- --exact
```

## 7. Follow-up measurements: where the source is lost

A smaller probe now reuses the scalar (`a := read …; a + a`), which makes the
failure deterministic; the earlier single-use spelling can happen to compile.
The probe is still compile-only and is not a committed test.

The measured chain is no longer just "node 1283 was never replayed":

- The final buffer is a materialized fresh `Buf(p)` result. Its
  `apply_result_owners` entry correctly recovers the constructor apply, but the
  apply's argument is itself a materialized `BufParam` with only `type` and
  `length`; it carries no input `.native` identity. Neither the result nor that
  argument has a `param_path`.
- The return containing this result is already decided, so `link_replay`'s
  early decided-return guard creates no replay. Removing that guard causes
  `expected Struct, found Struct` while importing `compute.lichen`; using
  `ParameterCheck::Value` for the new case does not remove the conflict. This
  experiment is rejected, not a new replay rule.
- `p <: Buf` still constructs a fresh `Buf`. `p : (Buf _)` hits a Struct/Struct
  type conflict. Neither is the source-preserving spelling.
- An experimental `Read := p => {.from p, …}` does preserve the source equality
  class. A class member then has physical slot path `[3, 2]`; following the two
  actual struct values turns it into logical input path `[1, 0]` (two struct
  headers). A class-member path fallback plus this wrapper makes the smaller
  `a + a` probe compile.
- That is not a landable combined fix. The wrapper plus fallback moves
  `parallel_read_input_buffer` past the refusal but leaves its root undecided,
  and makes `silu` produce a kernel whose root item is static. The full compute
  suite falls to **109 passed / 131 failed**. Moving buffer validation into
  `read`/`write` does not change those two outcomes. The class-member fallback
  alone, with the original wrapper, is suite-neutral at **113/127**.

The important boundary for the next step is therefore the difference between
the early return shell Lower reaches and the static apply's actually evaluated
return. In one trace, Lower reads `4390 -> 3766 -> apply 1041`, while the static
apply records evaluated return `4101`, value `4102`. Do not add another source
map until that actual return's payload and provenance have been compared with
the shell. A fresh constructor result and an input field still must not be
unified.

## 8. Landed field-provenance slice

The maintainer selected **keep the wrappers and record provenance** over changing
`Read`/`Write` into source-preserving wrappers. The implementation has four
parts:

1. Struct population records required field value → actual supplied field.
2. A parameter refinement records constant parameter projections on the
   constructor's own template return fields. Concrete user values are never
   back-propagated into a shared template.
3. Clone remaps carry the links. A cloned `Index(Apply, 0)` also links its apply;
   `record_applied_expr` links the corresponding returned value. After the
   parameter check, `bind_field_sources` resolves the projection against the
   actual argument's same slots.
4. Parallel input placement follows the links with a depth bound and normalizes
   physical struct-slot paths against the actual parameter values.

The map is keyed by exact nodes, never equality-class roots, and creates no
unification. It is preserved by GC, static freezing, and artifact format 27.
The code contract is in `Module::field_sources`,
`record_template_field_sources`, `bind_field_sources`, and
`parallel_buffer_pos_inner`; the prose summary is in
[value-templates](value-templates.md#constructor-fields-keep-their-source).

Measured after cleanup:

- the reduced `a + a` parallel probe compiles;
- lowlevel: **164 passed, 1 ignored**;
- language: **275 passed**;
- compute: **124 passed, 116 failed, 1 ignored**, up from 113/127;
- `parallel_read_input_buffer` passes: the second launch reads the first buffer
  and the final host read answers `22: Int`;
- `silu` passes after its test reads the kernel wrapper's `.native` field rather
  than the struct's name-table header;
- both kernel rendering tests pass: an opaque type no longer hides a decided
  native value behind a class variable, and their expectations now follow the
  constructor-signature model rather than the retired direct-`Int` wrapper.

The complete two-kernel reproduction initially answered
`raw none: Struct` because `check_instantiate` unified an undecided actual field
with a decided compound requirement, displacing the actual before runtime. Its
field's type and attributes are still checked, but the undecided value position
is no longer joined into that class; decided actuals retain the whole-slot
check. This is what allowed the second launch and final read to answer `22`.

Do not reintroduce the direct-wrapper or shared callee-template experiments:
the former regressed the suite, and the latter lets unrelated fields overwrite
one another's source. Post-return rebinding, GC-root retention, and making
`buf_payload` follow sources were also measured as irrelevant to this failure.

## 9. Workspace and workflow

- Worktree: `C:/resource/lichen-vm/.worktrees/compute-provenance`
- Feature branch: `compute-provenance`
- Main checkout: `C:/resource/lichen-vm`
- Clean baseline: `554560d2` on both the branch and `dev`.
- Shared target: `C:/resource/lichen-vm/target`. Always pass an explicit
  `workdir` and `CARGO_TARGET_DIR`; check that build paths name this worktree.
- Probes and trace files are temporary. Delete them before committing.
- Do not add tests without the maintainer's permission. Minimal probes are
  acceptable while investigating.
- Commit WIP at small checkpoints. Verify `git diff dev...HEAD --name-only`
  (three dots) and clean `git status` before every merge.
- Land with `git -C C:/resource/lichen-vm merge --ff-only compute-provenance`;
  read the merge exit code before any cleanup. Never use `git stash`,
  `update-ref`, or a soft reset to squash the worktree branch.
- Before the final merge run `comments-check --config comments-test.toml .
  --max-violations 0` and `cargo fmt --all -- --check`. Restore CRLF after fmt.
- No background job or subagent is still running. The read-only replay review
  is complete.

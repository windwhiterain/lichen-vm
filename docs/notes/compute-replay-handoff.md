# Compute replay handoff

> Status: **ready to resume.** `dev` and the `compute-provenance` branch are at
> `554560d2`. The worktree is clean. The launch replay fix is landed; the
> parallel read-provenance problem is still open.
>
> Read [replay-apply](replay-apply.md) for the design and measurements, and
> [value-templates](value-templates.md) for the constructor-template migration
> and the original class-wide regression.

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

## 6. Recommended next measurement

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

## 7. Workspace and workflow

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

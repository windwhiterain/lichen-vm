# What the operator line cannot express, and who owns it

Every entry below was measured by the operator line, and each names the probe that prints it, so an
entry is current until somebody re-runs that probe and sees it change. The owner column names the side
that decides the reading — most of these are not the operator line's to fix.

**Verification points.** The entries were measured while their features were built, re-measured at
`dev@4b60622f`, re-checked at `dev@7ccb2480`, and again at `dev@fe21eaa4` after the `::` chain landed
`the :: chains root at the pair, so a deferred type access resolves`. **Nothing moved**, and the reason
is worth keeping: that fix resolves a **type access** through a deferred chain, while rows two and
three are an unannotated recursion's own cells and row one is the kernel template. Two mechanisms that
look adjacent from the outside and are not.

At `dev@1713ed00` the whole inventory was read in **one run** (six probes, `6 passed`), so that tip is
the baseline every later check compares against; at `dev@f1bce423` the same run answered the same six
readings, and the commit below that tip is about pipeline recordings (`1 / 0`, `float2int` of a `NaN`),
which is a different mechanism from an unannotated recursion's cells.

**The re-read at `dev@c3eefe04` (the slot isomorphism's tip) moved three rows and one probe**, and each
row below now carries that reading beside the one it replaces. The isomorphism is
[array-attribute-view](array-attribute-view.md)'s landing: every pair is full width, a failed apply
unify no longer narrows the checked set, and a requirement a body states about its own parameter is
replayed at each apply. Two rows narrowed to a *check* refusal where they used to answer, one row's
refusal moved from the host lowering to the backend's class walk, and the second row's probe had lost
its only test — so "the same six readings" is no longer true and the list is kept per row.


## Gaps: not expressible, or broken

| gap | the reading | what it blocks | owner |
|---|---|---|---|
| **an `if` inside a fold body** | `zz_if_in_fold_probe.rs`, re-read after the class fix: the program **runs and answers wrongly** — `[0.0, 0.0]` where the fold's two additions make `[2.0, 2.0]` — because the conditional's **selector** is emitted as an *extra* comparison over the previous comparison's result (`Gt(Gt(s(0), 1), 1)`), which is always `0`, so the `if` takes its else arm every time. Measured shape: only an **order comparison** (`<`, `>`) whose operand is the **loop's carried state** doubles (`state_gt_1` body 2 comparisons, `one_lt_state` 2, `state_gt_leaf` / `leaf_gt_state` 2, `two_state_gt` 3); `==` over the state and a leaf operand are clean (`state_eq_2` 2, `leaf_gt_1` 2). The doubling is in the *graph* (one source `>` yields five `Gt` nodes across the template and its clones), so it is the apply-clone/selector path, not the emitter's classification. **Workaround**: compare a leaf (`c.taps > 1`) or spell `s(0) == 2` | a masked window computed inside a reduction; the alternatives are a caller's border or a clamp | the type system (the clone of a comparison whose operand is the carried state) |
| **the fold's class seeding** | **fixed**: the emitter declared a selection's class from the node's own fallback (`node_class`), which answers `Int` for a conditional inside a template-lowered body, and the `+` above it then took `Int`; both are now the arms' class (`conditional()` reads the emitted arms) and the state's (`state_read` records the block param, and a block parameter's class is its incoming argument's). Verified by the body dump: `Bin(Float, Add)`, `Select` classes `Float` | nothing; it was the first half of the row above | — |
| **unannotated recursion loses the type** | `zz_fold_type_probe.rs`: its only test had been deleted, so the documented command's filter named a test that did not exist and libtest printed `0 filtered` — **a pass with no reading**. Restored as a print-only test at `c3eefe04`, all four cases **refuse**: the control (`[straight]`) and the loop alike, at the kernel's own template (`binding the body would fill in at run time`), and the annotated recursion at the check (`this tuple read cannot be checked`). So the row's `raw[…]: ?a` is **not reproducible at this tip**; what is reproducible is that the probe's program no longer runs at all | a loop's answer has no type, so nothing downstream can use it | the type system (`f : f`, `language-spec` §3) |
| **unannotated recursion loses the value**, with a struct state | `zz_fold_struct_probe.rs`: now a **check** refusal, `this field read cannot be checked: the container's type is not decided here — annotate the container's type`, and the annotated spelling refuses too (`this tuple read cannot be checked`, five diagnostics). The row's `raw none` was a *run* reading and the probe has checked rather than run since `65b3bbc1`; the spelling that does run is `examples/decode_loop.lichen`'s — an annotated **parameter** and an annotated call, not an annotated lambda | a fold carrying the kernel's output struct; the buffer state is the way around it | the type system (`function-normalization.md` §1) |

| **a boundary the caller chooses at run time** | not expressible: the condition would have to be data | a boundary that is an argument rather than a leaf or a buffer | the language (control flow on a datum) |
| **the fusion floor refuses every single operator** | `zz_operator_floor_probe.rs`: `NotClaimed` without a claim; with one, `RegionIsNotAPair` **and** `BeyondANode`, whose sentence says the `element` role reads another node's output while it reads a parameter | fusing any operator this line writes, however it is spelled | fusion / the floor's model (`llm-operators.md`, `physics-operators.md`) |

## Traps: they parse or compile, and then surprise you

| trap | the reading | what to do instead |
|---|---|---|
| **index arithmetic is unsigned** | the *trap* is `operators.md` §5's, and it stands: `Int` is a machine-sized unsigned integer, so `max (col - 1) 0` never sees the `0` — `0 - 1` is `usize::MAX`. The probe that used to launch the window and print `4, 0, 1, 2, 3` now prints the **recorded** body and one lane's sub-terms (`[sums]`, `[row_clamp]`, `[col_clamp]`, `[row_scaled]`, `[sum]`), so that list is not a reading this probe makes any more; the row survives on `an_int_is_unsigned_where_the_two_readings_differ` | spell a lower clamp shifted: `(max i 1) - 1` (guide §4) |
| **`floor` is a reserved builtin** | a field named `.floor` does not parse (`expected a name, found 'floor'`) | name the field something else; `examples/mathlib.lichen` uses `.least` |
| **a negative literal in a field or array position** | `[1.0, -2.0]` and `.k -1.0` do not parse | write `(0.0 - 1.0)`; the examples do this throughout |

## Resolved, and how it was confirmed

- **`compute.jit`'s result type.** The parked example declared `raw 6: raw[?a, raw[?b, ?c]]` and the
  run now answers `6: Int`, so the example left `WORK_IN_PROGRESS` (`6acc039b`, with the fix on the type
  system's side at `096b75bb` and `9fad30c7`). It is the one entry here that moved, and it moved only
  because somebody ran the artefact again.

## Keeping this current

Each row names a probe; a row is stale the moment that probe prints something else. **One command
prints every reading in this file**, because a libtest run takes several `--exact` filters and ORs
them:

```text
cargo test -p lichen-compute --test lichen_compute -j 3 -- --exact \
  lichen_compute::zz_fold_struct_probe::probe_the_struct_annotation \
  lichen_compute::zz_fold_type_probe::probe_the_fold_type \
  lichen_compute::zz_if_in_fold_probe::probe_the_body_an_if_in_a_fold_lowers_to \
  lichen_compute::zz_masked_conv_probe::probe_the_masked_window \
  lichen_compute::zz_operator_floor_probe::probe_the_floor_on_a_real_operator \
  lichen_compute::zz_replicate_address_probe::probe_the_replicate_addresses --nocapture
```

Read it against the tables above: `this field read cannot be checked` for row three, a refusal on all
four cases for row two, the comparison counts per spelling for the `if` row (`state_gt_1` body 2 where
the source has one, `state_eq_2` body 2 where it has two because the guard is one of them), `NotClaimed`
and `RegionIsNotAPair` for the fusion row, and the recorded body for the unsigned trap. The notes that
carry the readings (`physics-operators.md`, `writing-a-solver.md`) say what each one means for the
operator library.

**This list rotted once, and the way it rotted is the hazard.** Between `65b3bbc1` and this re-read,
`zz_fold_type_probe` carried **no `#[test]` at all**, so the filter above matched nothing: libtest
printed `0 filtered out` and the command exited **0**. A reading that has stopped being taken and a
reading that has not moved look identical from the exit code, so the filter list is checked against
`--list` whenever a row is read, not only when a run fails:

```text
target/debug/deps/lichen_compute-<hash>.exe --list | Select-String "zz_"
```

**And a stub is not a backend.** `zz_masked_conv_probe` installs the stub, which answers zeros and
does **not** run the class walk, so a program the `cpu` backend refuses prints as `answered [0, 0]`
there. Row one is exactly that shape: on `gpu` under the stub it "answers", on `cpu` it is refused.
Read a probe's stage before its value: a refusal from the host lowering is evidence about the compile,
a refusal from the class walk (or from the assembler) is evidence about the body, and a stub's number
is evidence about neither.

The resolved row is not a probe — it is an example — so it is checked by the suite that runs it:

```text
cargo test -p lichen-compute --test lichen_compute -j 3 -- \
  --exact lichen_compute::examples::every_example_runs_and_prints_what_it_declares
```

**A check is only worth running when the tip moved.** While `git log <baseline>..dev` is empty,
neither command can say anything the baseline does not already say: the same binaries over the same
commit. So the protocol is two steps — look at the log first, run the commands only when it is not
empty — and a maintenance round with no movement is a round with nothing to do, which is the honest
answer rather than a re-measurement of the same build.

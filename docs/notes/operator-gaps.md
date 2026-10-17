# What the operator line cannot express, and who owns it

Every entry below was measured by the operator line, and each names the probe that prints it, so an
entry is current until somebody re-runs that probe and sees it change. The owner column names the side
that decides the reading — most of these are not the operator line's to fix.

**Verification points.** The entries were measured while their features were built, re-measured at
`dev@4b60622f`, re-checked at `dev@7ccb2480`, and again at `dev@fe21eaa4` after the `::` chain landed
`the :: chains root at the pair, so a deferred type access resolves`. **Nothing moved**, and the reason
is worth keeping: that fix resolves a **type access** through a deferred chain, while rows two and
three are an unannotated recursion's own cells and row one is the kernel template. Two mechanisms that
look adjacent from the outside and are not. The next time something does move, the commit that moved it
belongs in the entry.

## Gaps: not expressible, or broken

| gap | the reading | what it blocks | owner |
|---|---|---|---|
| **an `if` inside a fold body** | `zz_masked_conv_probe.rs`: refused, with a read in the arm *and* with a bare `0.0`/`1.0` — `a kernel body reached a node with neither a value nor an operation` | a masked window computed inside a reduction; the alternatives are a caller's border or a clamp | compute kernel templates (`lichen-compute-gpu`, the fold's template lowering) |
| **unannotated recursion loses the type** | `zz_fold_type_probe.rs`: `raw[…]: ?a` | a loop's answer has no type, so nothing downstream can use it | the type system (`f : f`, `language-spec` §3) |
| **unannotated recursion loses the value**, with a struct state | `zz_fold_struct_probe.rs`: `raw none`, **no diagnostic**, still at `dev@4b60622f` | a fold carrying the kernel's output struct; the buffer state is the way around it | the type system (`function-normalization.md` §1) |
| **`(Out)(.y x)` is not a usable seed** | `zz_operator_floor_probe.rs` and the state probes: a value nothing can read | seeding a fold by hand instead of from a real kernel result | the type system |
| **a boundary the caller chooses at run time** | not expressible: the condition would have to be data | a boundary that is an argument rather than a leaf or a buffer | the language (control flow on a datum) |
| **the fusion floor refuses every single operator** | `zz_operator_floor_probe.rs`: `NotClaimed` without a claim; with one, `RegionIsNotAPair` **and** `BeyondANode`, whose sentence says the `element` role reads another node's output while it reads a parameter | fusing any operator this line writes, however it is spelled | fusion / the floor's model (`llm-operators.md`, `physics-operators.md`) |

## Traps: they parse or compile, and then surprise you

| trap | the reading | what to do instead |
|---|---|---|
| **index arithmetic is unsigned** | `zz_replicate_address_probe.rs`: `min (max (col - 1) 0) 4` answers `4, 0, 1, 2, 3` for `col = 0..4` — the wrapped index is positive, so the lower bound never applies | spell a lower clamp shifted: `(max i 1) - 1` (guide §4) |
| **`floor` is a reserved builtin** | a field named `.floor` does not parse (`expected a name, found 'floor'`) | name the field something else; `examples/mathlib.lichen` uses `.least` |
| **a negative literal in a field or array position** | `[1.0, -2.0]` and `.k -1.0` do not parse | write `(0.0 - 1.0)`; the examples do this throughout |

## Resolved, and how it was confirmed

- **`compute.jit`'s result type.** The parked example declared `raw 6: raw[?a, raw[?b, ?c]]` and the
  run now answers `6: Int`, so the example left `WORK_IN_PROGRESS` (`6acc039b`, with the fix on the type
  system's side at `096b75bb` and `9fad30c7`). It is the one entry here that moved, and it moved only
  because somebody ran the artefact again.

## Keeping this current

Each row names a probe; a row is stale the moment that probe prints something else. Four of the six rows
are covered by one command, and a libtest run takes several `--exact` filters and ORs them:

```text
cargo test -p lichen-compute --test lichen_compute -j 3 -- --exact \
  lichen_compute::zz_fold_struct_probe::probe_the_struct_annotation \
  lichen_compute::zz_fold_type_probe::probe_the_fold_type \
  lichen_compute::zz_fold_buffer_probe::probe_the_fold_state \
  lichen_compute::zz_masked_conv_probe::probe_the_masked_window --nocapture
```

`zz_operator_floor_probe` covers the fusion row and `zz_replicate_address_probe` the unsigned trap, on
the same pattern. The notes that carry the readings (`physics-operators.md`,
`writing-a-solver.md`) say what each one means for the operator library.

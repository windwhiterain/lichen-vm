# A self-referential `let` in a kernel body kills the host process

> Status: **current** — every figure below was measured at `dev@12425166` on this machine, by
> building the worktree's own `lichen-compute` and running **one case per process** so that a
> crash names its own case. **This note changed no production file and no test**; the two probes
> it names in §10 are gone.
>
> **What is being diagnosed** is the one thing [division-rounding-gap](division-rounding-gap.md)
> §7 left: *"a `let` whose value reads the name it binds — **overflows the host stack** and the
> probe process dies with `0xC00000FD` on both vendors, before anything reaches a device. The same
> two lines written with a second name run."* That note measured the arithmetic and declined to
> diagnose the front end, so it is **referenced here, not restated**. What follows is the
> front end: the minimal program, the cause with its evidence, whether the program is legal, and
> what a fix would touch.
>
> **The one-sentence result: the program is legal, the language promises it will not overflow, and
> it overflows anyway — because the guard that was supposed to stop the walk fires at a depth the
> default 1 MiB stack cannot reach.** The `#[stacksafe]` annotation that made the walk survivable
> was **deleted in `2101384e`** and the comment that documents it was left behind.

## 1. The minimal reproduction

**Measured.** One line inside a kernel body — a block-wide binding whose value reads its own
name — is enough. The kernel is `compute.parallel` on the **`cpu`** backend, so **no device is
involved at all**:

```lichen
---
  compute = import "compute.lichen"
---
In  = struct{.a (compute.Buf Float)}
Out = struct{.y (compute.Buf Float)}
P   = compute.P (compute.KT Float)(.I In, .O Out)
body = (k : P) => {
  i = compute.range k.n
  qb = 3.0
  rc = rc * 2.0
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (rc ~ "element")))
}
kern = compute.parallel (body ~ "claim:Reduction(element)") "cpu"
a = (compute.buffer ([3.0, 3.0, 3.0, 3.0] : array{Float, 4}) : (compute.Buf Float))
out = (compute.plrun kern ((compute.A In)(.n 4, .I In(.a a))) : Out)
compute.collect out.y : array{Float, 4}
```

| | |
|---|---|
| what the process prints, last line | `thread 'main' (26504) has overflowed its stack` |
| exit code | **`-1073741571`**, which is **`0xC00000FD`** |
| backend | `"cpu"` — the crash precedes any device work, so the "both vendors" in the reporting note is explained rather than contradicted |
| the reported two lines, unchanged | **the same `0xC00000FD`** |
| the reported two lines with a second name (`rc0 = 1.0/qb; rc = rc0 * (2.0 - qb*rc0)`) | **runs**, answers `[0.33333334, 0.33333334, 0.33333334, 0.33333334]: array{Float, 4}`, exit `0` |

**The command is the front end, not the CLI.** `lichen run` fetches a prebuilt toolchain binary
over the network rather than using this checkout's crates, so the faithful local runner for the
same source text is [`lichen_language::run::evaluate_raw`](../../crates/lichen-language/src/run.rs),
which is what the `lichen-compute` test suite itself drives. **What differs from the CLI is the
delivery of the source, not the code that reads it** — and it is the code that reads it that
crashes, as §4 shows.

## 2. The shrink

**Measured, every row on the `cpu` backend with the same driver.** The question each row answers is
in the last column.

| kernel body | exit | what it settles |
|---|---|---|
| `rc = rc * 2.0` | **`0xC00000FD`** | **the minimal shape**: one binding, one self-reference |
| `rc = qb * rc` | **`0xC00000FD`** | the same, reading itself on the left |
| `rc = 1.0/qb; rc = rc * 2.0` | **`0xC00000FD`** | the reported pair minus the Newton arithmetic — **the arithmetic is not needed** |
| `rc = 1.0/qb; rc = rc * (2.0 - qb*rc)` | **`0xC00000FD`** | the reported pair, verbatim |
| `rc = 1.0/qb; rc = (rc * 2.0) : Float` | **`0xC00000FD`** | **an annotation changes nothing** |
| `rc = 1.0/qb; rc = rc + 1.0; rc = rc * 2.0` | **`0xC00000FD`** | three bindings behave like two |
| `qb = 3.0; rc = qb * 2.0` (no self-reference) | `0`, answers | **the trigger is the self-reference**, not the re-binding |
| `rc = 1.0/qb; rc = rc0 * (2.0 - qb*rc0)` | `0`, answers | the contrast the reporting note names |
| `rc = 1.0/qb; let rc = rc * 2.0` | `0`, answers `[0.6666667, …]` | **`let` is the working spelling**, and it answers *correctly* |
| `rc = 1.0/qb; rc = rc` | `0`, **refused by name** | a bare `x = x` does **not** crash — §3 |

**So the smallest shape is one self-referential block-wide binding inside a kernel body.** Neither
the arithmetic, nor a second binding, nor a buffer read, nor a type annotation is required. The
reported two lines are this case with a first binding that nothing ever reads.

**Measured: the same two lines outside a kernel body do not crash.** Run through the plain front
end with no `compute` import, `rc = rc * 2.0; rc` **checks and answers `none: Float`**, exit `0` —
the legal reading of a value the program never decides. **The kernel body is what turns an
ordinary undecided value into a fatal one.**

## 3. The cause

**Two candidates were on the table. Both were measured, and both are wrong as stated.**

**Candidate A — the second occurrence of `rc` resolves to the *new* binding, so the definition is
recursive and evaluation never terminates. RULED OUT, on three counts.** The resolution itself is
real and is *by design*: [`resolve_scope`](../../crates/lichen-language/src/resolve.rs) pre-enters
every block-wide name in one frame, so a second `rc` overwrites the frame entry and its value
reads the new binder. But (i) **one** binding is enough to crash, so the second occurrence is not
the trigger; (ii) the crash is in **checking**, so evaluation never begins (§4); and (iii) the
recursion does terminate, by the depth budget (§5). What survives of A is only this: the value is
genuinely self-referential, and **that** is what the kernel walk re-enters.

**Candidate B — the unifier builds a cyclic type and some walk recurses forever. RULED OUT.** The
type is not the thing that recurses: the checker cuts the type-level cycle with a skeleton pair
(§6), the same two lines outside a kernel print `none: Float` rather than looping, and at a
generous stack the program stops with a refusal that names **a kernel-body walk**, not a unifier,
a printer, an evaluator or a drop. A bare `rc = rc` is refused by name with *"a kernel body
reached a node with neither a value nor an operation"* — a different cause, reached by a different
path, and **not** a crash.

**What is actually happening, measured.** A self-referential block-wide binding makes the IR node
a **cycle**. The kernel-body walk memoises each node — but it inserts into the memo *after* the
walk returns, so a node re-entered before its first visit completes is not yet in the memo and
the walk descends forever. **The only thing that stops it is the depth budget**, which refuses at
**512 levels**:

```rust
// crates/lichen-compute/src/compute/body.rs:284
if self.depth > MAX_KERNEL_BODY_DEPTH {
    return Err(super::kernel_body_too_deep());
}
```

**And 512 levels of that walk do not fit in the stack the process actually has.** Measured by
running the identical program on a spawned thread of a chosen stack size:

| stack | result |
|---|---|
| **1 MiB** — the default main-thread stack on Windows | **`0xC00000FD`** |
| **2 MiB** | **the named refusal** |
| 3, 4, 6, 8, 16, 128, 512 MiB | the same named refusal, unchanged |

**So this is a bounded walk that the default stack outruns by roughly a factor of two** — not an
infinite recursion. The refusal it reaches, with a stack to reach it, is the repository's own:

> `compute.parallel: a kernel body's expression nests more than 512 levels, so lowering it would
> recurse deeper than a compile should spend on its stack … An unmarked recursion is **expanded**,
> and expansion nests every copy inside the last one's else arm, so the walk's depth is the trip
> count: it is the expansion that is too big rather than the program.`

**`RUST_MIN_STACK` does not rescue the CLI.** That variable sets the stack for threads Rust
*spawns* without an explicit size; it does not raise the main thread's, and the front end does this
work on `main`. The only way to give it room is a bigger thread, which is exactly what the probe
did and what the shipped binary does not do.

## 4. Checking or evaluating

**Measured: checking.** The probe runs the stages separately — preprocess and
`compile_with_imports_at`, with **no evaluation at all** — and the crash is unchanged:

| stage | `rc = rc * 2.0` in a kernel body |
|---|---|
| **check only** (preprocess + check, no run) | **`0xC00000FD`** |
| check + evaluate | **`0xC00000FD`** |

**The evaluator never starts**, so the printing / deep-evaluation / drop readings are all out, and
so is the VM's own non-termination budget: that budget lives in
[`Module::budget_exhausted`](../../crates/lichen-lowlevel/src/lib.rs) and is armed during
evaluation, which this program never reaches. §3's rule that the crash is in the **kernel-body
lowering walk** is therefore measured, not guessed — it is the only walk on that path, and the
refusal names it.

## 5. Is it a crash on a legal program?

**Measured and read from the spec: the program is legal, and the language says so in terms that
also promise no overflow.** [`docs/language-spec.md`](../language-spec.md) §Bindings:

> A binding is **block-wide** by default: its name is in scope throughout the block — forward *and*
> backward — so a value may reference its own name, a later binding, or any other binding of the
> block, and may recurse with them. The frontend reserves a placeholder id per block-wide binding,
> enters all the names before compiling any value, then fills each placeholder with its value's
> node — so a self/mutual reference makes the IR a cycle there (the graph contains a back-edge),
> which the checker totalizes by pre-registering a skeleton pair and binding the skeleton's cells
> to the finished pair (**no recursion, no overflow**).

Three things follow, and each decides part of the fix:

1. **`rc = rc * 2.0` is legal.** A value may reference its own name; this is not a program the
   language should refuse. The fix is therefore **not** a new diagnostic on self-reference.
2. **The cycle is expected and the checker is supposed to absorb it.** `rc = rc * 2.0` outside a
   kernel body answers `none: Float`, which is the spec's promise kept. The checker does its half:
   [`check_term`](../../crates/lichen-highlevel/src/checker.rs) pre-registers a skeleton pair for
   any `block_roots` member — *"A cycle forms only through a block-wide placeholder"*, gated on
   `block_roots` alone. **The checker is not where this breaks.**
3. **`let` is the restrictive form and it is the correct spelling.** The same spec: *"`let name =
   expr` is the *restrictive* form: the value compiles before the name enters scope, so the name is
   visible only to later statements"*, and `let a = a` resolves `a` to the **outer** binding.
   Measured: `rc = 1.0/qb; let rc = rc * 2.0` **runs and answers correctly** (§2). The reporting
   note's *"a Newton iteration is expressible; writing it the way one writes it is not"* is a true
   observation about **`=`**, and **`let` is the answer that is already there** — which is worth
   saying before anyone treats the crash as a request for a new refusal.

**So the language has already answered this once, in the wrong place.** The spec promises *no
recursion, no overflow* for the self-reference cycle, and the cycle is well formed; what overflows
is a **consumer of** that cycle on a path the promise did not name.

## 6. The comment that says this is already fixed, and is not

**Read from the source, quoted exactly, and left unchanged.** Two places in the tree assert that
this walk is `#[stacksafe]`:

```rust
// crates/lichen-compute/src/compute.rs:2920-2927
/// How many levels deep [`emit_node`]'s walk may go before it refuses.
///
/// # Invariant
///
/// The budget is on the walk, and the walk's depth is the trip count: an unmarked
/// recursion is expanded into a nested chain, so depth is linear in the trips.
/// `emit_node` is `#[stacksafe]` and this limit is the policy — a constant, never
/// derived from the thread's stack, and unreachable except by expansion.
const MAX_KERNEL_BODY_DEPTH: usize = 512;
```

```toml
# crates/lichen-compute/Cargo.toml:7-10
# `emit_node` is the one recursive walk on the kernel lowering path that nothing
# else grows the stack for, so it is `#[stacksafe]` and depth-budgeted
# (`MAX_KERNEL_BODY_DEPTH` in `compute.rs`).
stacksafe.workspace = true
```

**Both statements are false in this tree, and the measurement above is what they would have
prevented.**

- **`emit_node` does not exist.** It is named in four comments and in this dependency comment, and
  defined nowhere: `git log -S "fn emit_node"` shows it removed by `2101384e`. The walk that
  carries the budget is `Lower::value` in `crates/lichen-compute/src/compute/body.rs`.
- **Nothing in `lichen-compute` is `#[stacksafe]`.** The crate **still depends on `stacksafe`**
  (`Cargo.toml:10`) and **never uses it** — no `use stacksafe::stacksafe;`, no `#[stacksafe]`. The
  only surviving mention of the word in `crates/lichen-compute/src` is inside the prose quoted
  above, plus one more comment in `compute/body.rs`.
- **`2101384e` ("JIT side: the body is SSA, and every backend reads it") is where it went.** Before
  that commit `compute.rs` carried `use stacksafe::stacksafe;` and a `#[stacksafe]`; after it,
  neither is there and only the prose about them remains.

**[code-audit](code-audit.md) records this exact overflow as fixed**, at §"**The overflow was
`emit_node`, and it is now a named refusal.**", and it is right about the mechanism and wrong
about the tree:

> `#[stacksafe]` on `emit_node` is what makes a deep body *survivable* at all: **with the budget
> but no annotation, trip 100 still crashes below the limit.**
> — `code-audit.md:479-480`

**That is precisely the state this tree is in** — the budget present, the annotation absent — and
§3 measures the consequence. **The budget alone does not make the walk survivable; the annotation
was the half that made the refusal reachable.** Its own before/after table is a trip-count probe,
which is the one instrument that does **not** reproduce here: on today's tree a hand-written
finite recursion `f = n => if n <= 0 then 0.0 else 1.0 + f (n - 1)` at **trip 400** lowers
cleanly on a **1 MiB** stack, where the note records ~7 MiB. So the walk got cheaper in the SSA
refactor and the trip probe no longer separates anything — **but the cyclic case still needs more
than 1 MiB to reach its own budget**, which is why the note's figure could not be re-used as the
threshold here.

## 7. What a fix would touch

**No production change is made or proposed here. A fix is the next slice and the user decides
between these.** Nothing below has been edited; §6's quoted comments are left exactly as they are.

- **Restore `#[stacksafe]` on the kernel-body walk — a plain defect fix, and the smallest one.**
  The comment says it should be there, the dependency says it should be there,
  [code-audit](code-audit.md) says it was there and that without it the budget is not survivable,
  and §3 measures that it is not. This touches
  `crates/lichen-compute/src/compute/body.rs` (annotate `Lower::value`, or the `Lower` entry point)
  and nothing else; `stacksafe` is already a dependency. **It restores a stated invariant rather
  than changing one.** It would also make the `512` budget reachable, which is what §5 shows the
  refusal is for.
- **Re-derive the budget from the thread's stack — a contract change, and it is the user's.** The
  constant's own invariant says it is *"a constant, never derived from the thread's stack"*, and
  [code-audit](code-audit.md) §the same records **why**: *"a threshold derived from the thread's
  stack would differ between a debug and a release build of the same program, and a number that
  changes with the build profile is not one a program can be written against — which is the bar
  this item sets."* **That reasoning is sound and this note does not overturn it.** It also means
  `512` and a 1 MiB stack cannot both be right, and lowering `512` to fit would trade a crash for
  a refusal at a trip count a program could once be written against. **This is the user's call and
  it is a semantics decision, not a defect fix.**
- **An in-progress marker in the walk, so the cycle is cut rather than budgeted — a contract
  change.** `Lower::value` memoises after the walk (§3); a marker set before it would make any
  self-referential binding **terminate by construction** instead of by depth. **That changes what
  the language means** — the walk would stop lowering at the cycle and would have to decide what a
  kernel body writes for a value that is its own input — and §5 shows the answer is already
  specified for the `let` case and already implemented outside the kernel path. **The user decides
  whether a kernel body may contain a self-reference at all.**
- **Nothing in the resolver or the unifier.** Both candidates in §3 were measured out, and §5
  shows the checker absorbs the cycle as the spec says it should. **The resolver's "last binding
  wins" frame insert is the documented block-wide convention** and is not the defect; changing it
  would contradict
  [`compile_scope_statements`](../../crates/lichen-language/src/compile.rs)'s own invariant.

## 8. What is measured, what is read, what is inferred, what is unknown

**Measured, in full.** §1's exit codes on both spellings and the contrast case; §2's ten-row
shrink; §3's stack-size table and the refusal text; §4's check/evaluate split; §5's measured
`let` and outside-a-kernel readings; §6's trip-400 result and its non-reproduction of the note's
figure.

**Read from the source, quoted rather than measured.** The spec's §Bindings text and its *no
recursion, no overflow* promise; `resolve_scope`'s pre-entering frame; `Lower::value`'s memo
inserted after the walk; the `512` constant and its check site; the two false comments in §6 and
`2101384e`'s removal of `use stacksafe::stacksafe;` and `#[stacksafe]`.

**Inferred, and not measured here.** That the CLI would die identically to this probe: the
front end does this work on `main`, the default stack is 1 MiB, and §3 measures the crash at
exactly 1 MiB — but no run through `lichen run` was made, because that subcommand fetches a
prebuilt toolchain binary rather than building this checkout. That the walk's per-level stack cost
is a few kilobytes in a debug build, which is what the 1 MiB/2 MiB threshold implies and what was
not separately instrumented. That restoring `#[stacksafe]` would suffice: it is what
[code-audit](code-audit.md) measured for the same walk, and it was not re-measured here because
that would be a production change.

**Unknown, and this note closed none of these.**

- **Whether a finite expanded recursion inside a kernel body still needs a budget at all.** §6's
  trip probe no longer separates, so the budget's current value has no measurement behind it on
  this tree.
- **Whether other consumers of a cyclic IR node have the same walk and the same exposure.** §3
  measured the kernel path only; the checker's own skeleton cut is a different mechanism and was
  not audited for the same in-progress-marker gap.
- **Whether the language wants a kernel body to permit a self-reference at all.** §5 shows the
  spec permits the binding and that `let` is the working spelling; it does not say what a kernel
  body should *write* for one. **That is a decision for the user, and §7's third bullet is where
  it lands.**

## 9. Corrections to the notes this follows

**Each is stated in the note it corrects, in that note's own words.** What follows is the index.

- **[division-rounding-gap](division-rounding-gap.md)** §7's *"overflows the host stack"* and
  *"This is a front-end finding and this note does not diagnose it"*: this note. **It is a
  back-end-of-the-front-end finding** — the front end checks the program correctly and the kernel
  lowering walk overflows after it — and it is **not** a Newton-iteration problem: the minimal case
  is `rc = rc * 2.0`, and the reporting note's second-name contrast is the spec's own `let`.
- **[code-audit](code-audit.md)** §"The overflow was `emit_node`, and it is now a named refusal.":
  the refusal is real and still there, but **`emit_node` is gone and the `#[stacksafe]` that
  `code-audit` measured as the half that made the walk survivable went with it**, so the refusal is
  no longer reachable on a default stack. Its trip-count table does not reproduce on this tree
  (§6), so it cannot be used as the threshold.

## 10. What this slice deleted

**Two probes, and they are gone.** `crates/lichen-language/examples/selfref_probe.rs` — the
scratch driver for the no-`compute` readings in §2 and §5 — and
`crates/lichen-compute/examples/selfref_probe.rs` — the one-case-per-process driver behind every
figure in §1 to §6, carrying the stack-size argument, the check/evaluate split and the trip-count
probe. **Both held no production code and nothing else referenced them.** **No production file and
no test was touched**, `git diff dev...HEAD -- crates/` is empty, **no example file was edited**,
and the two devices were never involved: every row above is the `cpu` backend on the host.

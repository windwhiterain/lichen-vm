# A self-referential block-wide binding: the crash, the ruling, and what is left open

> Status: **the refusal has landed; one crash is closed and one is not.** The user's ruling, in
> their words: *"准确来说，这在图语义中并不合法，相当于自指，应当用 let 表示重绑定，但前端可能还未支持自指的重绑定"* — *"accurately, this is not legal in the graph semantics; it is equivalent to
> self-reference; it should be written with `let` as a rebinding, though the front end may not yet
> support a self-referential rebinding."* **Both halves of the uncertainty are now measured, and
> both were wrong in the tree's favour**: a self-referential block-wide binding is not legal, and
> the front end **does** support `let` rebinding — it runs and answers correctly (§11.2).
>
> **What this slice changed**: `crates/lichen-language/src/resolve.rs` refuses the self-reference
> at the name, `docs/language-spec.md` §Bindings carries the corrected sentence (**the user's
> decision**, quoted before and after in §5), and five tests in
> `crates/lichen-language/tests/language/pipeline.rs` that pinned the old reading now assert the
> refusal. **The user's "may not yet support" is measured false**, and the record of it is §11.2.
>
> **What the refusal closed**: `rc = rc * 2.0` in a kernel body was `0xC00000FD`; it is now a
> named resolve refusal, exit `0` (§11.1). **What it did not close**: a **mutual** value cycle
> — `a = b; b = qb * a`, which the language permits and the spec still promises — still overflows
> at a 1 MiB stack and reaches the 512-level refusal at 2 MiB (§11.3). **That second finding is
> the more important one, and it is the user's to decide.**
>
> Everything below §1–§10 is the diagnosis as it was measured at `dev@12425166`, kept as it was
> written. **Where §5 read the spec and concluded the program is legal, §5 now records that the
> reading was overturned by the user** — the measurement it rests on stands.

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

**This section was written as "no", and the user overruled it.** The reading below is kept as it
was measured, because the measurement still stands and the *conclusion* drawn from it is what
changed. **The edit to the spec is the user's decision, not this note's.**

**As measured then, and read from the spec: the program looked legal, and the language said so in
terms that also promise no overflow.** [`docs/language-spec.md`](../language-spec.md) §Bindings:

> A binding is **block-wide** by default: its name is in scope throughout the block — forward *and*
> backward — so a value may reference its own name, a later binding, or any other binding of the
> block, and may recurse with them. The frontend reserves a placeholder id per block-wide binding,
> enters all the names before compiling any value, then fills each placeholder with its value's
> node — so a self/mutual reference makes the IR a cycle there (the graph contains a back-edge),
> which the checker totalizes by pre-registering a skeleton pair and binding the skeleton's cells
> to the finished pair (**no recursion, no overflow**).

Three things follow, and **the first is the one the user overturned**:

1. **`rc = rc * 2.0` is legal** — *this was the reading, and it was wrong.* A value may reference
   its own name; this is not a program the language should refuse, so the fix is not a new
   diagnostic on self-reference. **The user's ruling is the opposite**: in graph semantics a value
   that names the binding it binds **is** its own input, so it is a self-reference and is not
   legal; a rebinding is written with `let`. **The sentence this reading rests on is the sentence
   the user had corrected** — §5.1 quotes it before and after, and §11.1 is the refusal that
   implements the decision.
2. **The cycle is expected and the checker is supposed to absorb it.** `rc = rc * 2.0` outside a
   kernel body answered `none: Float`, which was the spec's promise kept. The checker does its
   half: [`check_term`](../../crates/lichen-highlevel/src/checker.rs) pre-registers a skeleton pair
   for any `block_roots` member — *"A cycle forms only through a block-wide placeholder"*, gated on
   `block_roots` alone. **The checker is not where this breaks** — and after the refusal it is not
   even reached for this shape, while the **mutual** cycle that the ruling leaves alone still
   reaches it (§11.3).
3. **`let` is the restrictive form and it is the correct spelling.** The same spec: *"`let name =
   expr` is the *restrictive* form: the value compiles before the name enters scope, so the name is
   visible only to later statements"*, and `let a = a` resolves `a` to the **outer** binding.
   Measured: `rc = 1.0/qb; let rc = rc * 2.0` **runs and answers correctly** (§2). The reporting
   note's *"a Newton iteration is expressible; writing it the way one writes it is not"* is a true
   observation about **`=`**, and **`let` is the answer that is already there** — which is worth
   saying before anyone treats the crash as a request for a new refusal. **The user was unsure
   whether the front end supported it; §11.2 measures six `let` shapes, and it does.**

**So the language had already answered this once, in the wrong place.** The spec promised *no
recursion, no overflow* for the self-reference cycle, and the cycle was well formed; what overflowed
was a **consumer of** that cycle on a path the promise did not name.

### 5.1 The spec sentence, before and after — **the user's decision**

**Before** (`docs/language-spec.md` §Bindings, as quoted above and as `dev` carried it):

> … so a value **may reference its own name**, a later binding, or any other binding of the block,
> and may recurse with them. … so a **self/mutual** reference makes the IR a cycle there … (**no
> recursion, no overflow**).

**After** (the same paragraph, corrected):

> … so a value may reference a later binding, or any other binding of the block, and may recurse
> with them. It **may not name the binding it binds**: the name **is** the value's own node in the
> graph, so such a value is its own input, and it is a **resolve diagnostic** at the name. A
> rebinding is written with `let` … The one self-reference the language admits is the **recursive
> call**: a binding named as the head of an apply inside a `=>` body … because that edge is a call
> and not a value read.

**Two further sentences carried the same claim and were corrected with it** — the IR table's *"a
self/mutual reference makes the IR a cycle there"* (now *mutual*, plus the refused use lowering
inert) and §Name resolution's *"a value may reference a binding defined later **or itself**"* (now
*another binding*, plus the recursive call as the one admitted self-reference). **The history is
in §5 above**: the quote that was read, the reading it produced, and the ruling that replaced it.

**What the correction does not say**: it does not make the *mutual* cycle illegal, and it does not
claim that the kernel-body walk survives one. Both are §11.3's open question.

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

## 10. What the diagnosis slice deleted

**Two probes, and they are gone.** `crates/lichen-language/examples/selfref_probe.rs` — the
scratch driver for the no-`compute` readings in §2 and §5 — and
`crates/lichen-compute/examples/selfref_probe.rs` — the one-case-per-process driver behind every
figure in §1 to §6, carrying the stack-size argument, the check/evaluate split and the trip-count
probe. **Both held no production code and nothing else referenced them.** That slice touched **no
production file and no test**, **no example file was edited**, and the two devices were never
involved: every row above is the `cpu` backend on the host.

## 11. The refusal, landed and measured

**Everything in §11 was measured on this machine, in this worktree, one case per process** so that
a crash names its own case, with the worktree's own build and a private `CARGO_TARGET_DIR`. The
driver is named in §11.5.

### 11.1 The refusal

`crates/lichen-language/src/resolve.rs` refuses the self-reference at the name, at
`Stage::Resolve`, before any IR is built. **The resolver is where the language puts a binding's
scope rules** — its own `# Invariant` says *"The resolver is the only authority for the frontend's
scope rules"* — so that is where the rule lands, and the refusal is one named message
(`self_referential_binding`) shaped like the tree's own: the rule, then the way out.

> ``rc`` names itself in its own value: a block-wide binding's value may not reference the binding
> it binds, because the name **is** the value's own node in the IR graph, so the value would be its
> own input. Write the rebinding with `let`, which compiles the value before the name enters scope,
> so `let rc = …` reads the **outer** `rc` and binds a new one — recursion is spelled the same way,
> with the reference under a `=>` parameter (`f = n => … f (n - 1)`), which is a call rather than a
> self-reference.

**Measured, `cpu` backend, one case per process. The "was" column is this note's own §1–§3.**

| kernel body | was | now |
|---|---|---|
| `rc = rc * 2.0` | **`0xC00000FD`** | **refused by name**, exit `0` |
| `rc = qb * rc` | **`0xC00000FD`** | refused, exit `0` |
| `rc = 1.0/qb; rc = rc * (2.0 - qb*rc)` (the reported two lines) | **`0xC00000FD`** | refused, exit `0` |
| `rc = rc` (bare) | refused by the kernel walk | refused by the resolver, at the same span |
| `rc = id (rc * 2.0)` — **through a call** | **`0xC00000FD`** | refused, exit `0` |
| `rc = [rc * 2.0][0]` — **through an array read** | **`0xC00000FD`** | refused, exit `0` |
| `rc = (n => rc * n) (qb * 2.0)` — **self-reference under a lambda** | **`0xC00000FD`** | refused, exit `0` |
| `rc = (n => rc * n) ((n => rc * n) (qb * 2.0))` | **`0xC00000FD`** | refused, exit `0` |
| `rc = 1.0/qb; let rc = rc * 2.0` | runs | **runs**, `[0.6666667, 0.6666667, 0.6666667, 0.6666667]` |
| `qb = 3.0; rc = qb * 2.0` (no self-reference) | runs | **runs**, `[6.0, 6.0, 6.0, 6.0]` |
| the two-name quotient `rc0 = 1.0/qb; rc = rc0 * (2.0 - qb*rc0)` | runs | **runs**, `[0.33333334, …]` |
| the forward reference `rc = qb * 2.0; qb = 3.0` | runs | **runs**, `[6.0, …]` |
| `f = n => … f (n - 1); rc = f 3` (the recursive call) | runs | **runs**, `[4.0, …]` |
| `even`/`odd`, mutual recursion | runs | **runs**, `[0.0, …]` |
| `f = n => … f (n - 1); rc = f 400` (trip 400) | runs | **runs**, `[400.0, …]` |

**The last four rows are the rule's edge, and they are why the refusal is not "under a lambda".**
A self-reference is legal exactly when it is **the head of an apply inside a `=>` body** — the
recursive call, which is a call edge and not a value read. Every spelling in the tree is that
shape (`fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]`,
`@loop dot = s => … dot (s(0) - 1, …)`, `sum_to = s => … sum_to (…, …)`), so all of them keep
running. **`rc = (n => rc * n) (qb * 2.0)` is the same self-reference wearing recursion's clothes**
— it crashed before this slice and is refused now, and a rule that stopped at "under a lambda"
would have let it through.

**Two things the refusal needed beyond the diagnostic**, both measured rather than assumed:

- **The use is left unresolved**, so it lowers to the inert `ErrorBlock` an unresolved name
  lowers to. **A resolve diagnostic alone is not enough**: `compile_with_imports_at` reports
  diagnostics *and* lowers the program in the same call, and the cycle that lowering walks is what
  overflowed. With the diagnostic alone, `rc = rc * 2.0` in a kernel body **still died with
  `0xC00000FD`** — measured, on this worktree, before the inert lowering was added.
- **The refused use is reported once per binding**, at the first reference, and every further
  self-reference in the same value is still lowered inert (so no cycle survives anywhere in it).

**What a refused kernel program prints**: the resolve refusal first, then the kernel walk's own
*"a kernel body reached a node with neither a value nor an operation"* for the inert node that
replaced the self-reference. **The head is the right one** — it names the rule and the fix at the
offending use — and the second is the same cascade an unresolved name produces today, which the
spec's §Diagnostics describes as the frontend's recovery. Outside a kernel body there is
no second diagnostic.

**It is a check-time refusal, not a run-time one**: `compile("rc = rc * 2.0\nrc")` alone — parse,
resolve, lower, check, **no evaluation** — reports `checked = false` with exactly one diagnostic.

### 11.2 Is `let`-rebinding fully supported? — **yes, and the user's "may not yet" is false**

**Measured, plain front end, no `compute` import, one program per process.** The user wrote that
the front end *"may not yet support a self-referential rebinding"*; it does, and it answers
correctly.

| program | answer |
|---|---|
| `qb = 3.0; rc = 1.0/qb; let rc = rc * 2.0; rc` | **`0.6666667: Float`** — reads the outer `rc`, binds a new one |
| `let a = 1.0; let a = a + 1.0; let a = a + 1.0; a` | **`3.0: Float`** — a `let` chain |
| `a = 1.0; let a = a + 1.0; a` | **`2.0: Float`** — a `let` over a block-wide binding |
| `{ a = 1.0; let a = a + 1.0; a }` | **`2.0: Float`** — shadowing inside a block |
| `let a = a + 1.0; a` | refused as **`unresolved name 'a'`** — with no outer `a`, `let` has nothing to read, and that is the documented behaviour, unchanged |
| `f = n => { t = if n == 0 then 0.0 else f (n - 1); t + 1.0 }; f 4` | **`5.0: Float`** — the recursive call inside a block-bodied function |
| `f = n => if n <= 0 then 0.0 else 1.0 + f (n - 1); f 4` | **`4.0: Float`** |
| `even`/`odd` mutual recursion | **`1.0: Float`** |
| `a = b * 2.0; b = 3.0; a` (forward) | **`6.0: Float`** |
| `a = b; a = 1.0; b = a; (a, b)` (mutual, later wins) | **`(1.0, 1.0): tuple{Float, Float}`** |

**The refuted half of the user's sentence, stated plainly**: nothing about `let` needed building
for this. The restrictive form is the spec's §Bindings *"compiles the value before the name enters
scope"*, and it answers a self-referential rebinding correctly today.

### 11.3 The second finding — **a legal program still overflows**

**`a = b` and `b = qb * a` in a `compute.parallel` body is legal, and it still dies with
`0xC00000FD`.** This is the finding that matters more than the one the ruling closed, because the
ruling does not touch it: a **mutual** value cycle is a *different* binding naming the other, the
spec still promises it (`"any other binding of the block"`, and
[`compile_scope_statements`](../../crates/lichen-language/src/compile.rs)'s own `# Invariant`:
*"a value may forward- or mutually reference the block's bindings"*), and the checker totalizes it
outside a kernel body — measured, it answers.

**Measured, `cpu` backend, the same driver, on a thread of a chosen stack:**

| stack | `a = b; b = qb * a; rc = a` in a kernel body |
|---|---|
| **1 MiB** — the default main-thread stack on Windows | **`0xC00000FD`** |
| 2 MiB | the repository's own **512-level refusal** |
| 4, 8, 64 MiB | the same 512-level refusal, unchanged |

**So the refusal closed the crash for the shape the ruling names, and the walk's exposure is
unchanged for the shape the ruling does not name.** The walk reaches the same budget it always
reached; §3's finding stands — the budget is a depth the default stack cannot reach — and §6's two
false comments still describe it in the present tense.

**The neighbouring shape, measured, because it decides how the rule is drawn**: `a = b; b = c;
c = qb * a` does **not** crash — the aliasing in `compile.rs` collapses the three-name chain and
the kernel walk refuses it by name (*"a kernel body reached a node with neither a value nor an
operation"*), exit `0`. So the exposure is not "any cycle": it is a cycle whose edges all survive
aliasing. **A one-line change to the walk would change which programs land in which column**, and
that is a decision, not a measurement.

**Why this is the user's call, and not this slice's.** Three ways out, and they disagree about what
the language means:

1. **Restore `#[stacksafe]` on the kernel-body walk** — §7's first bullet, a plain defect fix, and
   it makes the 512 budget *reachable*, which is what §3 says the refusal is for. It changes no
   semantics and no number a program is written against. **It is the smallest thing that closes
   this crash**, and it is a one-line change to a file whose comments are already wrong (§6).
2. **An in-progress marker in the walk**, so a cycle is cut rather than budgeted — a contract
   change: the walk would have to decide what a kernel body *writes* for a value that is its own
   input.
3. **Lower `512` to fit a 1 MiB stack** — §7's second bullet, and the constant's own `# Invariant`
   says *"a constant, never derived from the thread's stack"*. **That sentence is one of the two
   this note did not touch**, and lowering the number would trade a crash for a refusal at a trip
   count a program could once be written against.

**This slice did none of the three.** It refused the shape the user ruled illegal, and it says here
that the other shape is still open.

### 11.4 The two false `# Invariant` statements — quoted, unchanged, with a recommendation

§6 quotes both. **Neither was edited, and this slice could not edit them without permission**: a
`# Invariant` sentence may not be added or changed by an agent. Here they are again, verbatim, so
the recommendation sits beside them.

`crates/lichen-compute/src/compute.rs:2920-2927` — **before, unchanged**:

```rust
/// How many levels deep [`emit_node`]'s walk may go before it refuses.
///
/// # Invariant
/// The budget is on the walk, and the walk's depth is the trip count: an unmarked
/// recursion is expanded into a nested chain, so depth is linear in the trips.
/// `emit_node` is `#[stacksafe]` and this limit is the policy — a constant, never
/// derived from the thread's stack, and unreachable except by expansion.
const MAX_KERNEL_BODY_DEPTH: usize = 512;
```

`crates/lichen-compute/Cargo.toml:7-10` — **before, unchanged**:

```toml
# `emit_node` is the one recursive walk on the kernel lowering path that nothing
# else grows the stack for, so it is `#[stacksafe]` and depth-budgeted
# (`MAX_KERNEL_BODY_DEPTH` in `compute.rs`).
stacksafe.workspace = true
```

**Both are still false, and the refusal does not make them true.** What did change is their
*reachability for this program*: `rc = rc * 2.0` no longer reaches that walk at all, so the
sentence *"unreachable except by expansion"* is now true of **the self-reference** — while §11.3
measures a legal program that reaches the budget anyway, which is the sentence being false about
the code rather than about one program. `emit_node` still does not exist, and nothing in
`lichen-compute` is still `#[stacksafe]`.

**Recommendation, and why it is not this slice's to make.** Restore the annotation **on
`Lower::value`** in `crates/lichen-compute/src/compute/body.rs`, which is the walk that carries the
budget (§6 measured that `2101384e` removed it), and rewrite both sentences to name *that*
function. It restores a stated invariant rather than changing one; `stacksafe` is already a
dependency; and it is the half §11.3's first option needs to close the crash that remains. **Two
reasons it did not happen here**: it is a **production change to another crate's hot walk** on the
strength of a §11.3 measurement taken on this machine only, and **editing an `# Invariant` needs
the user's permission** — which this slice does not have. **If the user wants option 1, it is one
annotation and two sentences, and this note names all three places.**

### 11.5 What this slice changed, what it deleted, and what the suites say

**Changed**: `crates/lichen-language/src/resolve.rs` (the refusal, the `Apply` head-position
tracking, the lambda depth, and the message); `docs/language-spec.md` (§Bindings, the IR table's
cycle sentence, §Name resolution, and the frontend-recovery sentence in the spec's §Diagnostics);
five tests in
`crates/lichen-language/tests/language/pipeline.rs`; this note; and the `docs/README.md` index row.

**Deleted, and named, before the commit**: `crates/lichen-language/examples/selfref_probe.rs` (the
no-`compute` readings in §11.1 and §11.2, one program per process) and
`crates/lichen-compute/examples/kernel_selfref_probe.rs` (the kernel-body rows of §11.1, the
stack-size argument of §3 and of §11.3, and the check/evaluate split of §4). **Both are scratch
drivers; neither is referenced by anything in the tree.**

**The suites, each measured here and attributed**:

| suite | result | movement |
|---|---|---|
| examples survey (`zz_examples_panic_probe`) | **`clean=43 panicked=[] drifted=[]`** | **none** — the required figures, unmoved |
| `lichen-compute` | **201 passed / 17 failed / 2 ignored** | **none** — the *same seventeen* names with the change reverted, measured in this worktree. **The brief's "204 / 14 / 2" does not reproduce at `dev@72b765aa` on this machine**; the seventeen are `automatic_tiling` (3), `combine_fold`, `consumer_guard`, `fold_datum` (2), `graph_jit` (2), `graph_structure`, `identity_from_body` (3), `pack_placement` (4), and they fail identically before and after |
| `lichen-compute-gpu` | **88 passed / 0 failed** | **none** — the RTX 3060 figure, so the default device here is the 3060; the Iris Xe figure (87 / 1, the `ln`) is the other device's |
| `lichen-language` | **260 passed / 11 failed** | **none** — the same eleven at `dev`. Before this slice's test update it was 257 / 14: the three new ones were the tests that pinned the overturned reading, and they are updated here |

**No `# Invariant` or `# Safety` sentence was added or changed anywhere.** The spec correction is
not one of those sections, and §11.4 names the two that are still wrong rather than editing them.
`crates/lichen-lowlevel/src/evaluation.rs` and `gc.rs` were not touched.

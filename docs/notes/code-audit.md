# Code audit and remediation queue

> Status: current — this is the audit's live open queue: the items below are the
> ones still open, and each item's status field is its live state. The audit
> inventory that produced them was taken at `dev@e4c0bae`; items found later say
> which `dev` they were seen at in their own evidence. A resolved item is deleted
> once its fix landed, so the queue lists only work still owed; the ruling that
> landed with it lives where the code does — in the note that owns the behaviour,
> or in the code's own `# Invariant`.
> Points at: the code it names (every claim carries a `file:line`).

This is the one place the audit's open findings live. It exists so the fixes can
be done one at a time, reviewed, and committed without re-deriving the analysis.

## How to read this

**Evidence legend** — every finding is marked:

- `verified` — re-read first-hand at the cited lines while writing this note.
- `reported` — found by a scoped read-only sweep and *not* independently
  re-verified. Treat the line reference as a lead, confirm before fixing.

**Status legend** — `todo` / `doing` / `blocked:<decision-id>` / `wontfix:<why>` /
`partly fixed — <what is left>`. A `wontfix:` row is a contract: its decision
below is the semantics, not a suggestion.

**Decisions** — items that cannot be fixed without a design call are marked
`blocked:Dn` and listed in [Decisions](#decisions). Do not silently pick an
answer for one.

**Tests** — this project forbids an agent writing tests without permission.
Items whose only sound proof is a regression test are marked `needs-test`; they
are blocked until that permission is given (see [Decisions](#decisions), `D3`).

## Queue

| id | severity | area | item | status |
|---|---|---|---|---|
| P1-17 | high | language-server | Every request runs the whole frontend | blocked:D6 |
| P1-23 | high | language-parser | The parser's 16 MiB worker overflows at 175 nesting levels | wontfix:D9 |
| P1-24 | high | language-parser | The AST's own recursive `Drop` overflows on a deep tree | wontfix:D9 |
| P1-39 | medium | compute | A `compute.call` inside a **parallel** kernel body is refused with a `NodeId`, so the device has no working call route | partly fixed — the message names the shared fact and lists the three shapes; the missing apply (a kernel compiled against an applied body) is parked under `compute-jit-low-types.md` |
| P1-42 | medium | compute | A `compute.write` the codomain does not reach is silently not written, with no diagnostic | wontfix:D17 |
| P1-50 | high | highlevel, lowlevel, language | An un-annotated recursive fold's struct-state read answers `none` | wontfix:D18 — the recorded `raw none: ?a` of `S = struct{.y Int}` ; `@loop f = (s => if s(0) == 0 then s(1) else f (s(0) - 1, S(.y (s(1).y + 1))))` ; `(f (0, S(.y 3))).y` is the un-annotated shape: the arity cannot be inferred, and `(s : tuple{_, _})` makes it `3: Int`. Mechanism and engine experiments in [recursive-fold-typing](recursive-fold-typing.md) |
| P4-10 | medium | render | The type printer recurses per type depth and overflows at ~200 | wontfix:D9 |

## P1 — correctness

### P1-17 — Every LSP request runs the whole frontend `verified`

**Status: `blocked:D6`** — the first half of `D6` (cache the extracted indexes and
reuse one `PackageStore`/registry handle across requests; cancellation plus a
debounce) has landed, and so have **client-version tracking** and **file
watching**; the remaining sub-item is (b) `BufferSession`, blocked on T3.

- **(b) `BufferSession` — untouched, blocked on T3.** It avoids the lex and parse
  on the keystroke path, and T3 — the memoized check — does not exist, so skipping
  the check without it is not a win; it is worth doing for the keystroke path and
  is not a substitute for what has already landed.
- **File watching landed.** The server now declares save support (`did_save`
  re-analyzes the saved document at once) and handles `did_change_watched_files`:
  a changed file drops every cached index whose dependency list names it and the
  affected open documents are re-analyzed. The dependency half of the cache key
  stays load-bearing — it is what tells the server *which* indexes an import
  change invalidates.
- **Document-version tracking landed.** `did_open`/`did_change` record
  `text_document.version` with the text, and the document's own diagnostic set now
  publishes with it (`Some(version)`), so a client can reject a set a newer edit
  superseded. An imported file's set stays `version: None` — it is not the
  document. The text hash remains the *cache* key; the version is what the client
  compares.

### P1-23 — The parser's 16 MiB worker overflows at 175 nesting levels `verified`

Split out of `P1-22` when its attribution was corrected: the parser is the
**first** layer to overflow, not the guarded one. `(e)`, `[e]` and their
neighbours are transparent in the grammar, so the recursion depth is the
*syntactic* nesting depth of the source, one frame per level.

```
$ n=174  → compiles            (349 bytes)
$ n=175  → thread '<unknown>' has overflowed its stack     (351 bytes)
```

Measured on this revision with `(` … `)`; `[` … `]` is identical because they
share the path. That is roughly 16 MiB / 175 ≈ **94 KiB of stack per nesting
level**, which is a lot per frame and suggests the recursion runs through
`chumsky`'s combinatorial machinery rather than one thin function.

**Why it cannot be fixed the way `P1-22` was.** `#[stacksafe]` works by wrapping
*a function of ours*, and the recursion here lives **inside the parser library**.
Growth would have to be arranged around the whole parse — for example running it
on a thread whose stack is grown on demand rather than a fixed 16 MiB — which is
a real design change, not an annotation. It is therefore part of the open
nesting-depth question rather than an independent fix.

**Why it matters.** A 351-byte file aborts the process, and the language server
parses on every request, so this is the crash a user reaches first. `P4-3` names
the walk duplication and the parser's cost; this is the crash.

**Closed `wontfix:D9`** — accepted risk, not repaired. See `D9` for why a limit
safe to enforce today would be dictated by this thread size, and for what
revisiting it would take.

### P1-24 — The AST's own recursive `Drop` overflows on a deep tree `verified`

Found while measuring `P1-22`. `#[stacksafe]` guards the walks, but the tree is
still freed by the compiler-generated recursive `Drop` for a boxed recursive
enum, and **no annotation of ours can reach it**:

```
lex + parse + mem::forget   survives 8000 nested terms on a 1 MiB stack
lex + parse + drop          does not
```

So a program deep enough to survive parsing and checking still aborts on the way
out — after all the work succeeded, which makes it the most confusing of the
three to debug. The only fixes are an iterative `Drop` impl or a
`ManuallyDrop`-based teardown for the expression type, which is a change to the
AST's shape and not a local guard; it is therefore part of the same nesting-depth
question as `P1-23`.

**Closed `wontfix:D9`** — accepted risk, not repaired. The cost of leaving it is
stated where it bites: the abort lands *after* a successful parse and check, so
the only symptom is a process that dies having printed nothing wrong.

### P1-39 — A `compute.call` in a parallel kernel body is refused with a `NodeId` `verified`

`verified` at `dev@1fe580c`, first-hand at the cited lines. Found by
`crates/lichen-compute/examples/recursion.rs`.

```text
k0 = compute.jit (v : Int => v + 1)
p = compute.parallel (cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write ((compute.Write _)(.to n, .at i, .value compute.call k0 i))
}) "gpu"
-- compute.parallel: kernel body hits a node with neither value nor operation
   (node=NodeId(394v1))
```

Three facts make it worse than a missing feature, and they compound.

**It is the only refusal in the compute surface that named nothing.** Every other
one names its own cause — `CONDITIONAL_WRITE`, `UNDECIDED_DOMAIN`, `CALLEE_ARGUMENT`,
the `SpirvRefusal` variants. This one reported a `NodeId`, a compiler-internal
number, which tells a reader nothing they can act on.

**The same call works in a scalar body.** `k1 = compute.jit (v : Int =>
compute.call k0 v + 1)` compiles and runs, answering `5 : Int`. So "one kernel
body calls another" is solved in one of the two body shapes and not the other,
and the difference was not named either.

**It means the device had no working call at all.** Only `parallel` names a
backend, so a cross-kernel call is reachable on a device *only* from inside a
parallel body — and that path failed in the compiler, before any backend saw it.
So `SpirvRefusal::CrossKernelCall`, which
[`lichen-compute-gpu.md`](lichen-compute-gpu.md) documented as the reason
cross-kernel calls were out of scope there, **was a refusal no lichen program
could provoke.** Both halves are now closed: the call lowers in a parallel body,
and both backends resolve it from a caller-supplied launch set
([lichen-compute-gpu § Several functions in one module](lichen-compute-gpu.md#several-functions-in-one-module)).

**Partly fixed, and the remainder is two cases of one fact.** The message is
now the fact all three share (*a kernel is compiled from a template before any
apply, so a binding the body would fill in at run time is still empty*) and lists
the three shapes, **deliberately without claiming which one it is** — a refusal
that names the wrong cause sends the reader to the wrong place, and the first
version of this message did exactly that by asserting a body-local binding when
the case that exposed it was a `compute.call`. The three cases, all confirmed on
both backends:

| program | result |
|---|---|
| a `compute.call` inside a **parallel** body (the identical call in a *scalar* body runs) | refused |
| a module-level helper called with a **body-local alias** fed by a read | refused |
| a helper **defined in the body** and called there | refused |

All three are the same **missing apply**, not three bugs: the enabling change is
compiling a kernel against an *applied* body, which
[`compute-jit-low-types.md`](compute-jit-low-types.md) calls "a fact about the
language, not a mechanism gap" and parks as per-call-site specialisation. That is
also the prerequisite for a self-referential call in a conditional's branch — the
shape that needs the function's type written out — so the two are worth looking
at together. Recorded in
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#8-the-defects-and-which-are-fixed)
§4.1 and §8.

### P1-42 — A `compute.write` the codomain does not reach is silently not written `verified`

`verified` at `dev@a63086f`, measured by a spike into
[shared-memory-and-barriers](shared-memory-and-barriers.md#why-the-witness-was-the-wrong-answer-and-what-replaces-it)
§3. Found while asking what a buffer *version* would order.

```text
w1 = compute.write ((compute.Write _)(.to b, .at i, .value i + 7))
compute.write ((compute.Write _)(.to b, .at i, .value i + 100))   -- the codomain
-- answered raw[raw 100, raw 101, raw 102, raw 103]
-- no diagnostic, no refusal: the first write never happened
```

**The lowering is demand-driven from the codomain leaves.** `lower_index_function`
(`crates/lichen-compute/src/compute/body.rs:230`) walks the codomain and nothing
else, so a `write` bound to a statement the return value never reads is never
emitted. The gate that used to sit beside this counted **emissions** against declared
outputs, and so saw one write against one output; it is retired, and what replaced it
checks that each codomain position holds a write naming *that* position — which this
program satisfies, because the second write does name it. The silence follows either
way.

**One narrowing since.** A write reached through another write's `.to` *is* emitted:
the ordinal a write fills is resolved along that chain, so `write (.to w1, …)` over
`w1 = write (.to b, …)` writes `b` twice and both links are in the body. What stays
dead is a write reached **neither** by the codomain nor by a `.to` chain — the shape
measured above, whose second write names `b` directly rather than `w1`.

The same mechanism was measured independently on a `@loop`: `seed = fill (…)` as a
statement beside a top-level write compiled, ran, and answered with the top write's
value — the nest was never in the IR at all.

**Why it is a diagnostic gap rather than obviously a semantic bug.** The language's
model is that a parallel index function's codomain *is* its output positions, so a
write outside it is not an output and arguably should not happen. But `compute.write`
reads as an effect at the call site, and nothing says otherwise — so the author's
two-output program compiles, runs, both backends agree, and one of the two writes is
missing. That is the failure class
[gpu-algorithms-ladder §5](gpu-algorithms-ladder.md#5-a-scatter-accumulate-is-silently-wrong)
made this project careful about.

**The effect token that landed at `4a41a71` does not catch it, and could not.** Both
writes take their state operand from the same `Const(State, 0)` root (measured: both
were the same `ValueId`), so the first write's result is a value nothing consumes and
the chain says nothing about reachability. Catching this needs a check that a
`ComputeOperator::Write` node exists in the body **and** is reached from the
codomain, which is a fact about the lichen graph rather than about the lowered body —
so it belongs in `compile_parallel_fragment`, beside the existing per-output
`write_value_node` gate, and not in `KernelBody::validate`.

Two shapes a fix could have taken were written down here — refuse the unreachable
write by name, or lower it anyway so the existing tally gate fires on it. **Both are
rejected; closed `wontfix:D17`.** The short form of `D17` is that this is not a
diagnostic gap at all: an index function's type is `A T.I -> T.O`, so it takes the
inputs *and the outputs* and returns the outputs, and walking the returned value
necessarily covers every declared output — the per-output `write_value_node` gate and
the check that each position's write **names** that position together already say so.
A `write` the
author did not connect to the return is a dead expression, and dropping a dead
expression is what a pure-function semantics does. What made it look like a bug is
that `compute.write` *reads* as an effect at the call site; under the buffer map's
model it is not one, it is the value of an output position. That is also why the
`State` token of `4a41a71` was a category error: you do not thread a state through
values. See [shared-memory-and-barriers](shared-memory-and-barriers.md#why-the-witness-was-the-wrong-answer-and-what-replaces-it)
§3, which reaches the same conclusion from the other direction.

The item stays in the file rather than being deleted because the *measurement* is
worth keeping: two writes to one output with only the second reachable do answer
with the second's value and no complaint, and anyone who later makes outputs
readable inside a body — which
[§9 step 4](shared-memory-and-barriers.md#9-the-staging) does, for a `block[1]` slice — has to
decide again whether an unreturned write is dead, because at that point it has a
reader. `D17` answers it for global buffers only.

## P4 — optimization

### P4-10 — The type printer recurses per type depth and overflows at ~200 `verified`

Found while measuring `P4-8`, and recorded rather than left as a footnote: the
guard's scan count collapsed by 171× but **wall time did not move at all** (2000
prints at depth 170: 4716–4968 ms with the `Vec`, 4973–5151 ms with the set),
because the binding constraint is the printer's **own recursion**, not the guard.
A hand-built right-nested array type of depth 200 **stack-overflows the process**,
before and after that change.

That makes a **fourth** stack-exhaustion path, and unlike the other three it is in
the renderer, which the language server reaches on hover — so it is reachable from
a deeply nested *type* in a file the user merely opened.

**Closed `wontfix:D9`, with `D9`'s reasoning transferred:** a file that aborts a
tool the user ran on their own machine crosses no privilege boundary, and the
editor exposure has the same shape as `P1-23`'s. It is recorded separately rather
than folded into `P1-23` because the *site* is different — that one is the parser's
worker, this is a printer — and because anyone measuring render cost needs to know
the depth is capped by recursion rather than by the guard. If that reasoning is
wrong for the renderer specifically, this is the item to reopen.

### P1-50 — An un-annotated recursive fold's struct-state read answers `none` `verified`

Re-measured at `dev@064436d6` while auditing the spec's recursion-typing paragraphs;
the whole record — mechanism, the author's `if`-desugar ruling, the four measured
negative routes, the handover and the workaround — is in
[recursive-fold-typing](recursive-fold-typing.md). The row exists because that defect
was the only measured type-system defect not in this queue, and the queue is the
thing a slice reads first.

**Decided `wontfix:D18`** (see the Decisions block): the parameter's **shape** is the
annotation the language requires — the arity cannot be inferred from uses, while the
field types infer per call, so `(s : tuple{_, _})` makes the recorded program answer
`3: Int`. The section stays as the verification record of the un-annotated form and of
the engine route that was rejected.

What the spec says, and what holds: an annotated recursive function's result is
decided (`gcd = (…) : tuple{Int, Int} -> Int` answers `6: Int`), without the
annotation the function's cells stay open (`raw 6: ?a`), the fold form annotates the
same way (`… : tuple{Int, S} -> S` answers `3: Int`), a struct-literal base keeps
working (`7: Int`), `Int : Type` / `string : Type` / `Type : Type` hold, and
`array{Int, 3} : Type` is refused `expected Type, found TypeArray` — every claim
measured here holds; the gap is exactly the un-annotated struct-state fold of this
row. The open fix is an engine change (the owner's call), and the note's test surface
is the whole regression: the S-state fold must answer `3` once fixed, the
tuple-of-Ints control must keep answering `7`, and the struct-literal base must keep
working.

## Decisions

These block the items marked `blocked:Dn`. Do not pick an answer silently.

- **D1 — Is `~/.lichen/artifacts/` untrusted input? — DECIDED: untrusted.**
  What that commits to: the reader must be **memory-safe and total against
  arbitrary bytes** (`P0-1` and `P0-5` are done; `P0-2` remains), and the
  container gets a **body digest** so corruption is detected before any of the
  body is interpreted (`P0-7`). One honesty note recorded with the decision: a
  digest detects *corruption*, not a deliberate writer — whoever can write the
  file can recompute the digest. Authenticity is not what a digest buys, and
  the docs must not claim it does; memory safety and total validation are what
  actually bound an attacker, and the digest is what turns bit rot into a clean
  recompile.
  *Rejected — trusted:* `checked_*` arithmetic only, signatures unchanged. It
  would have left the `P0-2` UB-from-safe-code path open.
- **D2 — Which git URL schemes are legitimate? — DECIDED: the minimal fix.**
  `git clone -- <url> <dir>` and reject any `url`/`rev`/`branch`/`tag` whose
  first byte is `-`. That closes argument injection (`--upload-pack`, and a
  `rev = "-f"` pin downgrade) without rejecting any legitimate URL form.
  *Rejected for now — a scheme allowlist:* stricter, but `ext::` is already
  disabled by default in modern git and an allowlist risks rejecting a form
  someone relies on (a local path dependency, say). If a scheme policy is ever
  wanted, it is a new item, not a reopening of `P0-3`.
- **D3 — Tests. — DECIDED: allowed, minimal, per item.** Each fix may add the
  smallest test that falsifies the defect it fixes, and only that. This
  unblocks `P1-5`, `P1-6` and `P1-13`, whose only sound proof is a test, and it
  means `P1-6` can now be pinned before it is touched. Tests stay in separate
  files from sources per `AGENTS.md`, and a fix must still not run the
  full-scale suite.
- **D4 — Toolchain integrity. — DECIDED: accept GitHub-over-TLS, and fix the
  docs instead.** The trust root is HTTPS to GitHub plus the release the
  maintainer published; no checksum or signature is added, and the download is
  not narrowed to github.com. What this commits to, and what the change must do:
  **delete the provenance claim the docs currently make.** `toolchain.rs`'s
  module doc reads as a guarantee (*"the toolchain and the package manager are
  always the same revision"*) while the commit is pinned and the bytes are not —
  the pin says *which* revision was asked for, never *what* arrived. State the
  actual trust model in its place, and state it once. The mechanical hardening
  the decision does not cover is split out as `P1-20` (a predictable shared temp
  name and no `fsync` before the rename), and the undocumented `$PATH` fallback
  is named there too.
  *Rejected — `SHA256SUMS`:* it would have kept the claim honest, at the cost of
  a release step and a verification path. *Rejected — signing:* strongest, and
  the most work; revisit if the toolchain ever ships to third parties.
- **D5 — Table key hashing. — DECIDED: canonical content unfolding.**
  A hashed key must be a function of the key's *content*, so a table or function
  value used as a table key hashes the same after a freeze and a reload as it did
  before. Chosen over a freeze-assigned structural identity (which would make the
  hash depend on the layout pass rather than on the value) and over declaring
  such keys unsupported (which would turn a silent miss into a refusal, narrowing
  the language rather than fixing the contract).
  What this commits to: `P1-3` and `P1-4` are one change, because they are the
  same key-comparison contract — a self-referential key needs a *canonical*
  unfolding so that two coinductively equal keys unfold identically, and the
  cycle token `hash_inner` uses must then agree with `key_eq`'s coinduction
  rather than counting depth. Expect the expr to be the hardest item in the
  queue: the unfolding has to be well-defined for a cyclic graph.
  **Re-verified at `8b94e13c`, and the equality side measured**: `key_eq`'s own
  contract is "a table keys by identity, a function by its id" (`table.rs:395-397`)
  and the measurements agree — content-equal tuples (`(1, 2)` bound or literal) and
  arrays (`[1, 2]`) hit, the same table value as key and lookup hits, a
  content-equal but **distinct** table misses, and two distinct lambdas with one body
  miss. The spec had said every key read is "deep-content-equal", which was wrong for
  tables and functions; it now states the equality the code implements, with this
  record as the measurement.
- **D6 — How far to wire incrementality. — DECIDED: (a) and (c) first, then (b).**
  Order: cache the extracted indexes per `(uri, version)` and reuse one
  `PackageStore` across requests, and add cancellation plus a debounce; wire
  `BufferSession` afterwards, once the first two have landed. `P1-5` is done, so
  the key is now genuinely injective and (b) is no longer blocked by it. Note
  what (b) does and does not buy while T3 (the memoized check) does not exist: it
  avoids the lex and parse, not the check, so the follow-up is worth doing for
  the keystroke path and is not a substitute for (a).
  `P1-18` — compute's registry eviction, per-launch wasm rebuild and unbounded
  `plrun` — is **not** part of this: it shares nothing with the editor path and
  proceeds on its own.
- **D7 — How to close `P0-2`. — DECIDED: make the obligation explicit (`B'`).**
  The accessors stay unbounded but become `unsafe` with one written contract, and
  the raw-pointer fields are privatised behind checked constructors.

  *Rejected — full lifetime binding (`A'`):* tying the returned slice to `&self`
  does not compile at the VM's own hot spots, because they read an arena slice
  **while mutating the module**. Verified at `gc.rs:57-68` (iterates
  `array.items()` across `self.garbage_collect_node`, then passes
  `array.items()` to `self.alloc_array`), `evaluation.rs:559-570` and `:578-582`
  (the deep pass), and `equality.rs:391-424` (holds `pa.items()`/`pb.items()`
  across `self.unify_inner`/`add_equality`/`record_error`); the same shape is at
  `function.rs:252`, `:555-580`, `table.rs:222`, `:264`, `apply.rs:143-144`.
  Every one of those would have to copy its node ids into a `Vec` before
  descending — an extra allocation per array/table visit in the deep pass,
  unification and GC. That is a hot-path regression, and it works directly
  against `P4-6`, which exists to remove exactly that kind of per-visit clone.
  Type safety bought with a permanent per-visit allocation in the VM's three
  hottest loops is not the trade this project should make.

  *Rejected — the audience split:* a lifetime-bound accessor for outside crates
  plus a crate-private raw one for the VM internals avoids the allocation
  entirely, but it still forces the callers that mutate inside the loop to be
  restructured (the checker's table walks, among others), and it adds a second
  accessor to a type that is already hard to read. Not worth the extra surface
  when `B'` reaches the same bound on the external hole.

  *What `B'` leaves open, stated so it is not mistaken for done:* inside the
  crate the one-clause invariant is enforced by discipline and review rather than
  by the type system. The mitigation is that the obligation now lives in exactly
  one written contract, every slicing site is `unsafe` and greppable, and no
  out-of-crate caller can obtain the slice from safe code at all.

- **D8 — Is a budget refusal sticky or scoped? (open; found by `P1-2`.)**
  The two guards implement opposite policies and nothing states which is
  intended.

  *Scoped* is what `deep_depth` does: incremented at entry, restored on every
  exit, so a refusal applies only to the subtree past the limit and shallow
  siblings still walk and are decided. `P1-2` documented the counter that way,
  on the argument that an inflated nesting counter is never restored except by
  `reset_apply_budget`, so every later `evaluate_node_deep` in the process would
  start past the limit.

  *Sticky* is what `apply_depth` does: its refusal returns before the decrement,
  and the comment there says so deliberately — *"so a caller still inside a
  refused apply cannot re-enter the walk"*. The `BudgetExhausted` doc points the
  same way: the verdict is latched because *"a second walk must know it ran
  against an already abandoned graph rather than discovering the exhaustion
  again"*. Read plainly, that says once abandoned, stay abandoned.

  So the asymmetry is the symptom, and one of the two is the anomaly. It is not
  a live bug: in production `evaluate_depth_limit` is 300 000 and the limits are
  only lowered by tests, so the difference is observable only when a limit is
  actually tripped. Resolve it as one policy stated on both counters —
  `P1-2`'s panic fix is independent of the answer.
- **D9 — Does the frontend need a nesting-depth limit? — DECIDED: no; accepted
  risk.** The three surviving stack-exhaustion paths (`P1-22` is fixed; `P1-23`
  the parser's worker and `P1-24` the AST's recursive `Drop` are not) are left as
  they are, and both items are closed `wontfix:D9`.

  The reasoning, recorded so it is not re-opened by accident: a source file that
  aborts the command-line compiler is a denial of service against the user's own
  machine, with no privilege boundary crossed — the compiler is not a service and
  the file being compiled is the user's own input. On the editor side the same
  input takes down the language server, which *is* user-visible, but the exposure
  is the same shape: a file the user chose to open (§ "What this does not
  cover", below).

  *Rejected — a depth limit with a diagnostic:* it would have to be a documented
  **language** limit, and the rejection is not about the work. `P1-23`'s number is
  imposed by the parser's fixed 16 MiB worker (175 nested levels before it dies),
  so any limit safe to enforce today is below that — i.e. the limit's value would
  be dictated by an implementation constant that is itself arbitrary, and the
  language would carry a restriction whose only justification is one crate's
  thread size. Fixing *that* first (growing the parser's stack on demand) is the
  other rejected option.
  *Rejected — unbounded stack growth:* it trades a crash for input-proportional
  memory, which is a worse failure for the language server, and it needs the same
  invasive iterative `Drop` for `P1-24`.

  **What this does not cover, so it is not mistaken for gone:** `P1-23` and
  `P1-24` remain real. A 351-byte file still aborts the parser, and a tree deep
  enough to be accepted still aborts while being freed. Revisit if the compiler
  ever becomes a service, if the language server starts accepting files it was
  not handed by the user, or if a legitimate program is found in the wild that
  nests past 175 levels.

- **D10 — Where the command-line surface lives. — DECIDED: its own crate.**
  `lichen-language` is the compiler **library**, and `clap` is a hard dependency
  whose entire use is `cli.rs`, so every embedder — including
  `lichen-language-server` — links a command-line parser it never calls. The CLI
  moves out of the library into a crate of its own (`P2-12`).

  *Rejected — a feature of `lichen-language`:* it is additive, so the surface
  stays in the library and every consumer still compiles the module; the generated
  plugin manifest would have to emit the feature, `[[bin]]` would need
  `required-features`, and the release workflows would have to pass it — the same
  three things that have to move for a separate crate, but with the CLI still
  inside the library afterwards.

  **What the move must carry, because each of these is a caller and not a detail:**
  `crates/lichen-package/src/plugin.rs` generates a plugin compiler's `main.rs`
  that calls `lichen_compiler::cli::main_with_native_packages::<crate::LangProgram>`,
  so the generated manifest's dependency line changes with the move; the compiler
  binary's target moves with it; and the release workflows that build and ship it
  follow. Keep the flag surface and the behaviour identical — this is a move, not
  a redesign, and `AGENTS.md`'s rule about not considering forward compatibility
  unless asked applies to any temptation to tidy the flags while in there.

  **Not part of this decision:** the README generator and `sync-readme`
  (`P2-6`'s remaining half) are repo tooling rather than a user-facing surface,
  and they leave the library on their own terms.
- **D11 — How far to encapsulate node state. — DECIDED: all six fields.**
  `P2-4`'s premise was partly refuted before this decision: the documented
  write choke-point (`Module::write_node_value`) **is** implemented, and
  `Node::value`/`low_shape` are private with a single writer each — so no public
  field bypasses the *value* write path. The real defect is the other six fields
  (`operation`, `function`, `block`, `visiting`, `evaluated_deep`, `equality`),
  which are `pub` with invariants of their own and no choke-point covering them.

  **The `low_shape` half of that premise was false until recently.**
  `Module::node_shape`/`set_node_shape` read and wrote `Node::low_shape` **per
  node**, while `class_low_type`/`refine_class_low_type` route it through the class
  representative — so a low type had a second writer and a second reader that
  disagreed, and both had zero callers across source, tests and examples. Removing
  them leaves `refine_class_low_type` as the single write side, which is what this
  decision assumed all along. Any reintroduced per-node accessor re-opens it.

  *Chosen — privatise all six and add the accessors.* The measured cost is **119**
  `cargo check` errors (116 in `lichen-lowlevel`'s own integration tree, 3 in
  `lichen-highlevel`, plus roughly a dozen more in `lichen-compute` that the
  compiler never reached). The acknowledged tension, recorded so it is not
  discovered mid-refactor: Rust cannot make a field write-private and read-public,
  so a write path must be **invented** for `operation` and `equality.parent` (they
  have none today), and read accessors must be added for `evaluated_deep`,
  `visiting` and `disjoint::Meta` — the read surface therefore **widens** while the
  write surface narrows. That is accepted: a documented reader is a contract,
  whereas a public field is an invitation.

  *Rejected — correcting the docs and leaving the fields:* it would close the item
  with the encapsulation the crate's own prose claims still absent, and the
  fields' invariants are load-bearing for *answers* rather than for memory safety,
  which is exactly the kind of breakage no test catches.
  *Rejected — privatising only the load-bearing few:* it leaves the boundary
  arbitrary, and the reader cannot tell which fields are contract and which are
  convenience.

  **Scope note:** `Module::nodes` is itself `pub`, so the refactor must also
  decide whether node state is read through the module or through the slotmap.
  Prefer the module (it is where a checked accessor can live); do not leave both.
- **D12 — What the CI gate enforces. — DECIDED: clear the backlog, then `-D
  warnings`.** `P3-3`: the workspace has 66 clippy warnings and CI enforces
  nothing — no `fmt --check`, no `cargo test`, no clippy.

  *Chosen — pay the backlog down first, then gate hard:* clear the 66 (the
  dominant clusters are 26 `collapsible_if`, 8 `type_complexity`, 8
  `needless_borrow`, 3 `arc_with_non_send_sync`), then add a job running
  `cargo fmt --all -- --check`, `cargo test` and
  `cargo clippy --workspace --all-targets -- -D warnings`. One gate, one meaning,
  and no baseline file to keep in sync.

  *Rejected — gating on "no new warnings" from a baseline:* it gets regression
  protection sooner but adds a file whose drift is itself a maintenance hazard,
  and this branch has already shown how quickly warning locations move.
  *Rejected — `fmt` and `test` only:* it would leave `arc_with_non_send_sync` —
  which is a real finding, not a style preference — unenforced.

  **Order matters and is part of the decision:** the backlog clearing and the gate
  are one item's work, and the gate must not land before the backlog is gone, or
  CI is red from its first run.
- **D13 — The parser's per-parse worker. — DECIDED: (a), a process-lived worker
  fed a per-parse copy of the tokens. Landed.** Measured: a
  fresh thread is **47%** of a 551-byte parse (289 µs of 608 µs, release) and
  **1.2%** of an 85 KiB one, and the cost is the **spawn**, not the 16 MiB stack
  (1/16/64 MiB spawns measure the same). The other half of `P4-3`'s finding is
  closed as refuted: the combinator graph **cannot** be hoisted, because 13 of its
  closures capture the token slice, and storing it behind a `'static` bound fails
  to compile (`E0597`, "`tokens` does not live long enough").

  *Chosen — (a), the copy.*  The three ways to feed a process-lived worker were
  all unpalatable on paper; the tie-break is that they differ by **three orders
  of magnitude on the input the item exists for**. The token copy is 1.6 µs on
  the editor's 551-byte file, where the spawn it removes is 289 µs; on the 85 KiB
  one it is 592 µs against a 1.4 ms spawn share, so it eats less than half the
  win there and leaves that input ahead. That makes (a) the *safe* choice and
  the *fast* one at once, where (b) is `unsafe` for no measured gain and (c)
  crosses three crates' APIs for none. `ParseWorker` is therefore one
  `OnceLock`'d thread with the 16 MiB stack, each parse a job that owns its
  tokens and answers on a channel.

  *Rejected — the serialization objection, on the facts.*  One worker does
  serialise concurrent parses where per-parse threads did not, which is a real
  change in shape.  It is accepted because nothing that parses concurrently is
  on a hot path: the language server serialises requests anyway
  (`concurrency_level(1)`) and the CLI parses one file.  A pool would need reply
  routing for no measured win, so the shape is recorded rather than abstracted
  away.

  *Rejected — leaving it to `P1-17`'s cache.*  The cache removes a repeat request
  for one text and does nothing when the text changes, which is every keystroke.
  That is exactly the input (a) is measured on.

  **Re-measured on landing, same session, alternating rounds, release** (input A
  = `examples/struct_generic.lichen`, 539 bytes / 38 tokens; input B = the
  generated 81 560-byte / 24 001-token file, as `P4-3` describes):

  | | before | after |
  |---|---|---|
  | A, min | 259.6–296.1 µs | **164.3–203 µs** |
  | A, median | 354.7–422.1 µs | **177.6–271.9 µs** |
  | B, min | 102.9–105.5 ms | 101.1–103.8 ms |

  Best-against-best on the minimum — the statistic that survived the noise — A
  goes **259.6 → 164.3 µs, −37%**, and B is inside 2%, which is what the
  decomposition predicted (a 1.2% spawn share against a 0.6% copy).  Absolute
  figures are *not* comparable to the ones `P4-3` recorded: that session's
  baseline was ~608 µs for the same input where this one measures ~270 µs, which
  is why the comparison above was re-derived by building both sides in one
  sitting rather than against the note.

  Pinned by `parses_share_one_worker_thread` (both parses on one thread, and not
  the caller's) and `a_panicking_parse_leaves_the_worker_alive`.  The second
  guards the hazard the reuse *introduces*: a per-parse thread contained a panic
  to its own parse for free, and a shared one does not unless the panic is caught
  on the worker and resumed on the caller.  Watched to go red — with the catch
  removed, the first parse's panic kills the worker and the **next** parse fails
  with "the parse worker is gone", which is what a long-lived host would see and
  a single-shot test never would.
- **D14 — Where the IR's strings live. — DECIDED: (c), keep the leak and
  document the rate; re-measure before revisiting.** `P4-6`'s intern
  leak is real and measured: every compile permanently leaks every string
  literal (`Expr::Str` has no dedup at all) and every distinct interned name, at
  1–11 bytes per literal per compile and 31 bytes/compile on an editor-like
  stream of changing sources. Nothing reclaims any of it.

  *Chosen — neither fix, because the note never priced the leak in absolute
  terms and the absolute terms are small.*  31 bytes per compile is **about 3 MB
  per 100 000 keystrokes**; a heavy editing day is 10 000–50 000 edits, so under
  1.6 MB.  That is an unbounded growth, and unbounded is what the item is about —
  but it is unbounded at a rate that does not justify either candidate:
  - **own the strings in the IR** — a lifetime parameter on the literal and on
    `ExprKind` (or an owning arena the IR borrows from), rippling through `IR`,
    the checker, and `persist`'s codec.  This is the only *real* fix (it
    reclaims), and it is the largest change in the ledger, to be paid against
    bytes per keystroke.
  - **a process-global intern table** — dedups identical strings across
    compiles, but still never reclaims anything and does nothing for the editor,
    where each keystroke's literal is a new distinct string.  It bounds the
    growth rate, not the growth.

  The decision is therefore recorded **here, and the leak sites point at it**
  (`compile.rs`'s `intern_op` / `intern_str` and the `Expr::Str` arm;
  `lowlevel/codec.rs`'s deserializer leak is the same shape): each site carries a
  one-line note naming `D14`, because the failure mode this guards against is a
  reader concluding the leak was overlooked.  The measured rate, and why the
  `&'static str` is load-bearing, live in this entry — `D14` is the decision to
  revisit rather than the comment to delete.

  *What would revisit it:* the rate changing by orders of magnitude (a host that
  compiles far more often than a keystroke stream), or `ExprKind`'s `Copy`
  ceasing to be a requirement for another reason — at which point (a) costs only
  the ripple and buys the whole leak back.
- **D15 — Who owns a compiled kernel or buffer? — DECIDED and landed: the
  arena owns buffers, the process owns kernels (content-addressed).** The
  original finding stands: `KERNELS`/`BUFFERS` grew without
  bound — one fragment per `$jit`/`$parallel` evaluation and one `count`-element
  vector per `plrun`, nothing ever removed; measured, one distinct program
  evaluation adds exactly one of each (60 evaluations took the registries from
  179 to 239 kernels and 2 to 62 buffers).  (`P1-18`'s other two claims are fixed
  — the derived-module cache and `plrun`'s element bound.)

  The reason it was a decision: the registry cannot tell when an entry is
  unreachable. `KernelId`/`BufferId` were `pub type … = usize`, so the id is
  `Copy` and is copied into node value caches, equality classes, apply clones
  and static modules; nothing observes the last copy dying, and the arena has
  no per-value `Drop` to hang a release on. Eviction would have to guess, and
  the first eviction of a live id turns a later `launch`/`read` into the lazy
  marker — a silent wrong answer, worse than the leak. A bound that *refuses*
  new entries instead never drops a live one, but it permanently bricks a
  long-lived host at N programs and changes a working program's answer, which
  is a functional regression rather than a memory bound.

  **The `Arc`-in-the-value shape is priced, and it is the whole lowlevel.**  The
  `Copy` requirement is not per-variant: `P::Value`'s contract is
  `ValueExt: Debug + Copy + PartialEq` (`lowlevel/lib.rs:416`), so one non-`Copy`
  variant forces the bound off the *entire* vocabulary.  Dropping `Copy` from
  `ComputeValue`, `LangValue` and that bound turns the workspace into **70 errors
  across 13 files in `lichen-lowlevel`** — every site that copies a node value,
  in a VM whose value read and write paths are hot.  That kills the option, and
  with it the whole "owner in the value" family.

  **Buffers: the block arena, as `AnyHandle<[i64]>`.**  The mechanism the note
  never considered is the one the codebase already uses for compound data: a
  `LowValue::Array` is a `Copy` handle into a block's bump arena, and
  `AnyHandle<T>` is `Copy` for any `T` (`lowlevel/lib.rs:601`).  `ValueExt`
  defines the **ext-handle payload** contract for a vocabulary's own payload
  (`is_handle`/`handle`/`set_handle`/`alignment`), and the lowlevel's copy path
  routes a *program-specific* value to `copy_ext`, which consults it — so a
  buffer payload is relocated like any other and **dies with its block**.  No
  registry, no id, no eviction, no aliasing, and `Copy` is untouched: the value
  stays a handle.

  What was actually missing was smaller than "unexercised": the composed
  `LangValue` hard-coded `is_handle() -> false` ("the composed values are
  structurally inert"), so **no plugin leaf could own a payload at all**.  The
  composition now dispatches `is_handle`/`handle`/`set_handle`/`alignment` to
  its leaves, which is what makes the `None => copy_ext` arm in the lowlevel's
  copy paths reach a plugin rather than silently skipping it.  Measured caveat,
  recorded rather than hidden: a `Bump` never reclaims per-object, so repeated
  `plrun`s inside one *live* module accumulate in that block until GC drops it —
  the editor's case is exact (P1-17 drops the `Build` per analysis, so buffer
  memory is bounded by open documents), and a long-lived single module leans on
  `garbage_collect` — which is what the language's `@gc` marker is for
  ([gc-block](gc-block.md)).

  **Kernels: the process, content-addressed.**  A fragment is not a leaf payload:
  `KernelInstr::CallKernel(KernelId)` makes it reference *other* fragments, and
  assembly resolves those through the registry, so "immutable artifacts shared
  across modules" is load-bearing for the call graph and not merely a cache.
  They stay process-global, but ids become a **content digest** with an intern
  index and a unique-id fallback on a genuine collision (so an id can never alias
  a different fragment — the failure mode is a recompile, never a wrong kernel).

  Content addressing is not a tidiness change; it is what makes the derived
  module cache work at all.  That cache is keyed on the kernel id, so
  a fresh id per compile meant it **could never hit**, and every keystroke
  re-assembled and re-ran `wasmi::Module::new`.  Measured on the same
  `jit`+`launch` program compiled three times in one process, counting module
  cache misses:

  | | round 0 | round 1 | round 2 |
  |---|---|---|---|
  | a fresh id per compile | +1 | +1 | +1 |
  | content-addressed | +1 | **+0** | **+0** |

  The counter is `compute::module_cache_misses`, visible for tests and
  measurement for the same reason the package store's `compiled`/
  `loaded_from_cache` are.

  Pinned by `lichen-compute/tests/lichen_compute/buffer_payload.rs` (a pointer comparison, not
  a content read — a dangling bump payload frequently still *reads* correctly,
  so a content-only test would pass on a broken relocation) and by the two
  `kernel_intern_tests` in `compute.rs`.  All three were watched to go red: with
  the leaf answering `is_handle = false` the relocation test fails at the
  dispatch, with `set_handle` a no-op it fails at the address, and the intern
  tests fail if the id is per-compile again.
- **D16 — Is `e[i]` an array read, or a positional read of any container? —
  DECIDED: arrays only; the spec is corrected.** The spec and the checker state
  two different languages.

  *Arrays only* is what `check_index` implements and documents
  (`checker/indexing.rs:19-28`, `:39-52`): the container's type is pinned to a
  fresh array type, so a tuple or a struct instance is refused with
  `DiagKind::Guard`, and the positional read of a **tuple** is the dedicated
  `a(k)` (a struct instance reads by name, `s.x` — the tuple-only narrowing is
  the addendum to `P1-34`'s outcome) — the operator is chosen by syntax, never by
  a runtime kind dispatch. Every example agrees (`examples/index.lichen` reads
  `b(0)`).

  *Any container* is what `language-spec.md` §3 stated, twice and explicitly:
  `e[i]` reads the `i`-th element of "an array, tuple, or struct instance", and
  `s(1, 2)[0]` "is the first field".

  The two are not reconcilable, and they differ on what a written program means,
  not on how it is spelled: `(1, 2)[0]` is `expected TypeArray, found TypeTuple`
  today (the kind-based wording of
  [eval-before-unify](eval-before-unify.md) §2.2; when this was written it spelled
  the patterns) and would be `1` under the spec. **Chosen — the doc fix:**
  the code's intent is explicit, its doc argues *why* (syntax picks the
  operator, never a runtime kind dispatch — the same rule that keeps `e[i]`,
  `a(k)` and `t <== k` three different things), every example agrees, and a
  language that *should* index tuples is a feature rather than a
  reconciliation. `P1-34` corrected §3's two sentences; nothing in the checker,
  the parser or the examples moved, so both reproductions still answer with the
  guard.
  **Re-verified at `8b94e13c`**: `(1, 2)[0]` and `struct{.x Int}[0]` are refused
  (`expected TypeArray, found TypeTuple` / `found Type`), `l(1)` over an array is
  refused (`expected TypeTuple, found TypeArray`), and `l[1]` over an array is
  `20: Int` — the four reads answer exactly as this decision says.

  *Rejected — teaching `check_index` the tuple and struct kinds:* it is the
  larger of the two and it would have to answer a question the deleted sentence
  answered badly — what a *struct's* positional index means, given that the
  spec's answer was "its wrapped tuple's elements" while the struct read has its
  own resolution through the struct's name table (`s.x`). A user who wants
  `(1, 2)[0]` to be `1` files that as a new item, and it starts by answering the
  struct question.

  One adjacent fact belongs with the decision rather than the item: the type
  access `X{n}` (since removed — [type-access](type-access.md)) *looked* like it
  could have spelled the spec's meaning without
  touching `check_index`, and it could not — over a runtime container it reads
  components of
  something that is not a tuple type value, which is refused at check time (the
  container's type must be the tuple kind; `P1-35`), and on the tree this was
  written on was a **reported** runtime error where it used to print `none`
  silently (`P1-35`). So the choice really was between the
  two sides above; there was no third spelling then, and the form is gone now.

- **D17 — Is a `compute.write` the codomain does not reach a silently dropped
  effect, or a dead expression? — DECIDED: a dead expression; `P1-42` is
  `wontfix`.** The index function's type settles it. `S = T: KT _ => A T.I -> T.O`
  and `P = T: KT _ => struct{.n Int, .in T.I, .out T.O}`, so an index function takes
  the extent, the inputs **and the outputs**, and returns the outputs. Walking the
  returned value therefore necessarily covers every declared output, and two gates
  already enforce that they are the same set: each codomain position must reduce to a
  `compute.write` (`write_value_node`), and that write must **name** that position —
  its `.to` resolves to the ordinal the position holds. A `write` outside that walk is
  not an unwritten output — the
  output *is* written, by the write that is in the codomain — it is an expression
  whose value nobody reads, and dropping it is what a pure-function semantics does.
  What that commits to: **a `compute.write` is not an effect, it is the value of an
  output position.** The lowering now reads that position **off the value**: a write's
  ordinal is resolved from its `.to` operand through any chain of writes, and never
  from the order the writes were emitted in — before that, the ordinal was an emission
  counter and the `.to` the author wrote was never read at all, so a body naming its
  targets in another order than the codomain lists them silently exchanged two outputs.
  That is the buffer map's model stated as a rule, and it is why
  the `State` token of `4a41a71` was a category error rather than merely an expensive
  choice — you do not thread a state through values, and a token that nothing reads
  cannot order anything. The replacement in
  [shared-memory-and-barriers](shared-memory-and-barriers.md#why-the-witness-was-the-wrong-answer-and-what-replaces-it)
  §3 follows from the same sentence: `write` returns a new **version of the buffer**,
  which is a value, so an unreturned one is dead by the same rule and a returned one
  is ordinary data flow.

  *Rejected — refuse the unreachable write by name:* it would be a new diagnostic for
  a program that is already wrong by its own type, and it would have to decide what
  "unreachable from the codomain" means for a write inside a `@loop` nest, which is a
  harder question than the one being answered.

  *Rejected — lower it anyway so the tally gate fires:* cheaper, and it reuses a
  diagnostic that already names both numbers, but it makes the emitter's work depend
  on nodes the type says are not outputs, and it would turn a dead expression into a
  refusal for a program whose declared outputs are all correctly written.

  **The scope of this decision is global buffers.** [§9 step
  4](shared-memory-and-barriers.md#9-the-staging) makes a `block[1]` slice readable
  inside the body that writes it, and at that point an unreturned `SliceWriteCall` *does*
  have a reader — another lane's `SliceReadCall` — so "dead" stops being the right answer
  for it. Whoever lands step 4 has to decide it again for a `block[1]` scope, and the
  honest default there is the opposite one: a staged write is reachable from any lane of
  the workgroup, so reachability from the codomain is not the test.
- **D18 — Can an un-annotated recursive function's result type be decided? — DECIDED:
  no; the parameter's **shape** is the annotation the language requires, and `P1-50` is
  `wontfix`.** Measured at `dev@575ad0be`: without any annotation `g = s => s(1)` prints
  `?a -> ?b` — the **arity** of a tuple a recursive function destructures cannot be
  inferred from its uses — while the **field types** can: annotating the shape alone,
  `@loop f = ((s : tuple{_, _}) => …)`, makes the application decide them
  (`(f (0, S(.y 3))).y` answers `3: Int`, `(s : tuple{_, _}) => s(1)` applied to `(0, 7)`
  answers `7: Int`, and `tuple{Int, _}` / `tuple{Int, S}` behave the same). So the
  contract is: **a recursive function whose result type must be known states its
  parameter's shape**, and every field type may stay `_`. The inference itself is not the
  defect — it works as soon as the arity is stated. The engine route (the clone walk
  carrying class members, then evaluating the codomain's deciders) was measured: it fixed
  the repro and improved the un-annotated `gcd` to `6: Int`, but it hung
  `recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget`, and gating it
  to shallow passes regressed the repro — both recorded in
  [recursive-fold-typing](recursive-fold-typing.md), not carried forward.

- **D19 — Does a released scope keep its pending asserts? — DECIDED: yes; the condition is a
  collect root.** `check_asserts` drains after the pass that collects, so the old lowlevel
  contract "an assert's condition dies with its block" (`gc_prunes_asserts_of_dropped_blocks`)
  let `@gc` change a program's verdict: measured, `x = @gc { @assert (1 > 2); 3 }; x` built
  `ok=true` under the retired deep trigger, while the same program with the collection
  disabled reported `assertion failed`. The condition survives the release now, and the test
  states the new contract (`gc_moves_a_pending_assert_condition_out_of_a_dropped_block`).
  *Rejected — asserts die with their scope:* that is the one semantic change `@gc` was
  documented not to have ("it computes nothing").
- **D20 — May a template's own scope be collected at compile time? — DECIDED: yes, and the
  per-call scope is then not promised.** A body whose `@gc` value does not depend on its
  parameters decides during the definition pass, so a trigger on the read releases the
  template scope there: measured, `f = n => @gc { a = 1; a }` leaves 7 blocks / 5 declared
  scopes after compile and 2 / 0 after the run. *Rejected — skip a block whose parent is a
  function's template body:* it keeps a whole scope alive for the module's lifetime to serve
  calls that may never come.

- **D21 — How does a collect keep a node only a parent-block payload names? — DECIDED: the
  language layer guarantees the boundary; the engine does not validate.** A referrer may name
  its own block, an ancestor (the ancestor outlives it), a parentless block (never released),
  or a child scope's declared root. The language front end holds it **by construction**:
  `check_gc` gives the parent `Index(expr, 0)` and `Index(expr, 1)` reads of the scope's expr,
  so the declared root is the only child node a parent names, and reading that handle is what
  collects the scope. Measured: the `block_boundary` pin scans every payload and operand of a
  language corpus and finds 0 violations (it failed on `x = [@gc 1, 2]` before the change).
  *Rejected — a parent cell aliased to the inner's:* `@gc f 41` answered `none`, because the
  engine does not redirect a class member's read through its representative; *rejected — the
  declared root as the value handle:* a block scope's value is its tail, not its root. The
  engine stays unvalidated by decision, so a hand-built module may still break the boundary —
  the ignored repro in `basic::gc_safety` records that contract.

# Code audit and remediation queue

> Status: current — this is the audit's live open queue: the items below are the
> ones still open, and each item's status field is its live state. The audit
> inventory that produced them was taken at `dev@e4c0bae`; items found later say
> which `dev` they were seen at in their own evidence. The resolved items were
> deleted once their fix landed, so this note lists only work still owed.
> Points at: the code it names (every claim carries a `file:line`).

This is the one place the audit's open findings live. It exists so the fixes can
be done one at a time, reviewed, and committed without re-deriving the analysis.

## How to read this

**Evidence legend** — every finding is marked:

- `verified` — re-read first-hand at the cited lines while writing this note.
- `reported` — found by a scoped read-only sweep and *not* independently
  re-verified. Treat the line reference as a lead, confirm before fixing.

**Status legend** — `todo` / `doing` / `blocked:<decision-id>` / `wontfix:<why>`.

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
| P1-33 | medium | highlevel, language | A self-recursive call in a conditional's branch is refused as "expected Int, found Int" | todo |
| P1-36 | medium | highlevel | A duplicate kind-marker tag shadows a codec arm and warns instead of failing | todo |
| P1-39 | medium | compute | A `compute.call` inside a **parallel** kernel body is refused with a `NodeId`, so the device has no working call route | todo |
| P1-40 | medium | lowlevel | The apply budget refuses a long **terminating** loop as non-terminating | todo |
| P4-10 | medium | render | The type printer recurses per type depth and overflows at ~200 | wontfix:D9 |

## P1 — correctness

### P1-17 — Every LSP request runs the whole frontend `verified`

**Status: `blocked:D6`** — the first half of `D6` (cache the extracted indexes and
reuse one `PackageStore`/registry handle across requests; cancellation plus a
debounce) has landed; the remainder below has not.

- **(b) `BufferSession` — untouched, blocked on T3.** It avoids the lex and parse
  on the keystroke path, and T3 — the memoized check — does not exist, so skipping
  the check without it is not a win; it is worth doing for the keystroke path and
  is not a substitute for what has already landed.
- **No file watching.** `did_save` and `did_change_watched_files` are still
  absent, so editing an imported `math.lichen` never refreshes the importer; that
  is why the dependency half of the cache key is load-bearing rather than a
  belt-and-braces extra, and it leaves the server to notice a change only by
  hashing, on the next request.
- **No document-version tracking.** `server.rs:161-168` still discards
  `text_document.version` and `publish_diagnostics` always passes `version: None`,
  so a client cannot reject a stale diagnostic set. The text hash is what makes
  the *cache* sound, and it is not the same thing as a version the client can
  compare against.

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

### P1-33 — A self-recursive call in a conditional's branch is refused `reported`

`reported` for the *site*, not for the symptom: the refusal reproduces first-hand,
the cause is **not located**.

```text
f = a => b => if b == 0 then a else f b (a - b)
f 48 18
```

`dev@a972a79` answers `error: expected Int, found Int` — no span line, both sides rendered
as the same type name.

**The escape hatch is to annotate the *function*, and that is what makes this an
inference defect rather than a missing feature.** The same body prefixed with
`: Int -> Int -> Int` checks and runs (it then exhausts the apply budget, as
`-` on a wrapping `Int` must); annotating the *parameters* instead
(`f = a => b => { a : Int; b : Int; … }`) still fails with the same message. So the
program is well-typed, the checker cannot get there on its own, and what it
reports is a type conflict between two nodes that both mean `Int` — a message
that names the wrong thing. (`examples/gcd.lichen` writes the function's type out
for the same reason, in the tuple-domain spelling.)

Narrowing, each run first-hand against the same binary (`dev@a972a79`):

| shape | result |
|---|---|
| `g = a => b => a - b; f = a => b => if b == 0 then a else g b (a - b)` | runs (`Int`) — a non-recursive two-argument callee in the branch is fine |
| `f = a => b => [f b (a - b), a][b == 0]` | refused identically — so it is not the `if` desugaring, which is exactly this array index |
| `f = a => b => f b (a - b)` | runs (and exhausts the apply budget, as it must) — a self-reference alone is fine |
| `f = x => if x == 0 then x else f (x - 1)` | runs — single-argument self-recursion in a branch is fine |
| `f = (a => b => if b == 0 then a else f b (a - b)) : Int -> Int -> Int` | **checks and runs** — the function's type written out is the whole difference |
| `f = a => b => { a : Int; b : Int; if b == 0 then a else f b (a - b) }` | still refused — pinning the parameters is not enough |

So the trigger is a **self-referential call as an element of the conditional's
array**, with a curried two-argument function on both sides of it — and the
checker gets there only while the function's own type is *unwritten*.

**The lead, and it is only a lead:** both sides print `Int`, and `Int` does not
fail to unify with `Int`, so the two sides are two *different* nodes that each mean
`Int`. The annotation is the interesting half: writing the function's type out
decides which nodes the recursive reference's domain and codomain cells *are*, and
the failure disappears — so what is wrong is which cell the recursive call's
argument is compared against, not the types involved. The place to look is the
interaction between a block-wide (self-referential) binding's pre-registered
skeleton and the per-apply parameter clones (`lowlevel::function`, and
`checker/operators.rs`'s pin of an `Int` operand to `self.int_type`). Nothing here
has read that path; this is a starting point, not a diagnosis.

**Located, not fixed — re-measured at `dev@cbf4fcc`, and the lead above was
wrong about which side is at fault.** It is not the checker's cells at all: the
diagnostic is `DiagKind::Runtime` with `loc: None` — an **apply-time parameter
check in the VM**, not a checker's pin. The two sides it compares evaluate to
`USize(48)` and `USize(18)`, i.e. the *values* the two arguments were given at
the call site, and they print as `Int` only because a `Type`-typed value is
rendered by its type.

The trigger, narrowed first-hand (the earlier table above all holds; `if c then
t else e` lowers to `[e, t][c]` — the branches are swapped, `compile.rs:754` —
so "the array index" and "the `if`" are one form):

| shape | result |
|---|---|
| `f = a => b => [f b a, 0][b == 0]` | **refused** — the minimal form |
| `f = a => b => [0, f b a][b == 0]` | runs — same call, other element, so the *branch* is never taken |
| `f = a => b => [f b a, 0][0]` | recurses (budget) — a literal index does not trigger it |
| `f = a => b => [f b a, 0][c]`, `c = 0` | recurses — nor does a constant index |
| `f = a => b => [f b a, 0][b - b]` | **refused** — any computation over the parameter does |
| `f = a => b => [f b a, 0][b == b]` | runs — that one evaluates to 1, so the other branch is taken |
| `f = a => b => [f a b, 0][b == 0]` | recurses — passing the parameter to the *second* argument is fine |
| `f = a => b => [f b, 0][b == 0]` | **refused** — one curried apply is enough |
| `f = a => b => [a, 0][b == 0]` | runs — no recursive call |

So: the index expression must **read a parameter**, the selected element must be
a **self-recursive apply passing that same parameter to the function's first
parameter**, and the function's type must be unwritten. Everything else in the
report above is downstream of that.

**Where it goes wrong, traced.** Instrumenting `apply_parameter_check`
(temporary, reverted) on `f = a => b => [f b a, 0][b == 0]; f 48 18` shows
three applies, and the third is the wrong one:

```
APPLY fn=1v1 … template_param=19v1 leaf=Parameterized   argument leaf=USize(48)
APPLY fn=3v1 … template_param=96v1 leaf=Parameterized   argument leaf=USize(18)
APPLY fn=5v1 … template_param=162v1 leaf=USize(48)      argument leaf=USize(18)   <- refused
```

`1v1` is `f` and `3v1` the inner `b => …`; **`5v1` is a fresh closure clone**,
and *its declared parameter already holds 48* — the first argument of the
enclosing call, not the 18 this apply is checking. The unify is
`unify(cloned_param, argument)` at the pair's leaf: 48 against 18. So the
statement to take forward is: **when an apply instantiates the curried closure
that `f`'s body will apply again, the clone's parameter is seeded with the
enclosing apply's argument**, which is exactly the class of defect the note's
lead guessed at, one level down from where it guessed. The candidate sites are
`function.rs`'s closure branch in `value_apply` (`:464-562`: the fresh id is
registered with the *source* parameter at `:485-492` and re-pointed at the
remapped clone at `:517`/`:558`) and `regroup_clones` re-establishing the
template's class topology among the clones (`function.rs:179-187`) — both of
which can seed a fresh parameter from a source that is already bound.

**Why this is recorded and not fixed.** The two candidate sites are the VM's
closure-instantiation and class-regrouping paths, where a wrong answer is a
silently wrong *value* rather than a crash; the audit has no `lowlevel` test
that pins a curried closure's per-call parameter (`tests/basic/evaluation.rs`
covers recursion, not this), and this item's own scope is the queue's. A fix
here is its own item with its own regression test, and the diagnosis above is
what that item needs to start from. The lead paragraph's guess — the checker's
skeleton and `check_binop`'s pin — is refuted: nothing the checker does is
involved, and the parameter annotation's only effect is which argument the
definition pass happens to be holding.

**What wants this fixed, and what it is not.** A `loop` operator for kernel
bodies — `loop f n`, applying `f` `n` times — is the shape that wants this, and
it is *the* thing standing between the language and an unrolled loop in a kernel.
Its own status, and the two ceilings a static expansion runs into instead, are in
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach)
§4.1; `P1-39` is the other half, because in a kernel the operator also needs the
body applied before it is lowered, and `P1-40` is what a *dynamic* loop removes by
construction. **[Loop conversion](loop-conversion.md) is the design that reaches
the kernel case, and it supersedes the `loop f n` surface this entry names** — a
`loop` is a one-node cycle in its general form, and the natural formulations this
item would have to annotate (Euclid below, `mutual_recursion.lichen`) are not of
the form `T -> T` repeated `n` times. It does not fix this item: the marker
supplies no type, so the annotation is still the escape for a two-argument
self-reference on the **host**. Today the only working form is the annotation this
entry records as the escape:

```text
loop = (f => n => x => if n == 0 then x else loop f (n - 1) (f x))
     : (Int -> Int) -> Int -> Int -> Int
inc = x => x + 1
(loop inc 3 0, loop inc 10 5, loop inc 0 7)   -- (3, 15, 7): <Int, Int, Int>
```

**The blast radius is wider than loops, and the repository's own example is
written around this bug without saying so.** The trigger is not "recursion" and
not "loops" — it is a **self-referential two-argument call inside a conditional's
branch**, and the most natural way to write a two-argument recursive algorithm in
a curried language is exactly that. Euclid, unannotated:

```text
gcd = a => b => if b == 0 then a else gcd b (a % b)
gcd 48 18
-- expected Int, found Int
```

The same body annotated checks and runs (`6 : Int`) — so the escape applies, and
Ackermann, a two-parameter fold and a two-parameter tree walk are all in the same
place. **`examples/gcd.lichen` dodges this twice over**: it takes a **tuple**
parameter rather than two curried ones *and* writes the type out —

```text
gcd = (p => if p(1) == 0 then p(0) else gcd (p(1), p(0) % p(1))) : <Int, Int> -> Int
```

— and its `doc` attributes the annotation to a *different* reason ("under
self-recursion a call's result is its own type cell"). So the annotation is
documented as an inference nicety while it is also what keeps the example from
being refused, and a reader following the natural curried form gets a diagnostic
that names two identical types. Worth recording because **the shape of the
example is carrying a constraint the note does not state**, and the cost is paid
by every program written the obvious way rather than by the one that is shown.

### P1-36 — A duplicate kind-marker tag shadows a codec arm and warns instead of failing `verified`

`verified` at `dev@04b5aef`, first-hand at the cited lines; found while adding a
ninth kind marker, which is recorded in
[`floating-point.md`](floating-point.md).

The kind-marker registry (`crates/lichen-highlevel/src/shape.rs:44`) states its
own compatibility contract: *"an existing entry's tag must NEVER change, and a new
marker takes the next unused tag"*. The `TypeValue` codec is generated from that
registry, and `crates/lichen-highlevel/src/program.rs:541-545` additionally
claimed that a registry entry colliding with a hand-spelled arm **"fail[s] to
compile"**.

Neither half of that is true, and the difference is the whole item.

**The tag space is not the registry's.** `define_type_value_codec` expands the
registry's arms and *then* appends `TypeValue::TypeId(n)` — a nominal struct
identity, not a kind marker — which holds tag `8` (`:562-568` write, `:581` read).
So the space is the `TypeValue` codec's, shared with an arm the registry does not
own, and the ninth marker takes `9` rather than `8`.

**A collision warns rather than fails.** The registry arms come first on both
sides, so a marker claiming `8` makes `8 => TypeValue::TypeId` unreachable on
read. The compiler emits `unreachable_patterns` — a **warning**. `cargo check`
passes. Every persisted `TypeId` then decodes as that marker, with the `u64` that
followed it left unread in the stream, and the failure surfaces as a wrong type
somewhere later rather than as a build error.

Measured while implementing: `cargo check -p lichen-highlevel` succeeded with the
warning present, and the collision is caught by an existing round-trip check
(`crates/lichen-language/src/persist/codec.rs:174-193`, which writes a
`TypeId(7)` and then iterates `KIND_MARKERS`) — which is a test this crate does
not run.

**Latent, not live.** No marker claims a duplicate tag today, so nothing is
broken. It is a trap for the next person who adds one, which is the same class as
`P1-26` and `P1-27`: the format's guarantee lives in a comment rather than in a
check.

Note the asymmetry, which is correct and worth preserving: a *gap* in the tag
space falls through to `tag => return Err(format!("unknown type-value tag {tag}"))`
(`:582`) and is a clean error, while a *collision* shadows an arm and corrupts
silently. Any guard added here must tighten the second without softening the
first.

Three ways to close it, none free: generate a compile-time duplicate check
alongside the codec (a `const _: () = assert!(…)`, which is what the project
already uses for the attribute order), add a `build.rs`/macro-time deduplication
that refuses at expansion, or pin the registry's tags to a checked constant.
`floating-point.md` §3.2 records the trap for whoever allocates the next tag; the
guard itself is this item.

### P1-39 — A `compute.call` in a parallel kernel body is refused with a `NodeId` `verified`

`verified` at `dev@1fe580c`, first-hand at the cited lines. Found by
`crates/lichen-language/examples/recursion.rs`.

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
also the prerequisite for `P1-33`'s intended use (below), so the two are worth
looking at together. Recorded in
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#8-the-defects-and-which-are-fixed)
§4.1 and §8.

### P1-40 — The apply budget refuses a long *terminating* loop as non-terminating `verified`

`verified` at `dev@cd1ddeb`, first-hand, via `crates/lichen-language/examples/recursion.rs`.

The VM's budget is a real and good thing — it is how a runaway recursion is caught
rather than hung. But it reports one verdict for two different situations, and the
message names the wrong one:

```text
sum_to = s => if s(0) == 0 then s(1) else sum_to (s(0) - 1, s(1) + 1)
sum_to (1000, 0)     -- 1000: ?a          25.4 ms
sum_to (4000, 0)     -- this binding never terminates — it applied a function
                      -- more than 2000 times (non-terminating recursion)
```

The second program **terminates**, in 62 ms. The budget exists to say "this never
terminates"; here it says that about a loop that finishes, and the reader is sent
to look for non-termination that is not there. This is a conflated verdict, and
the fix shape follows from that: **the verdict needs to distinguish
"stopped because it exceeded a budget" from "stopped because it cannot
terminate."** A budget that refuses must say which.

**Related and separate — and this half is now fixed.** The second ceiling an
unrolled loop runs into is the emitter's own recursion, which **overflowed the
stack between 400 and 1000 iterations with no diagnostic at all** — a crash
rather than a refusal. Measured: 100 iterations 4.4 ms, 400 iterations 11.6 ms
for a four-element kernel (about 29 µs of compile time per iteration), and a hard
overflow at 1000. **Re-measured first-hand on this machine at 100**, on the main
thread of a debug build, via `crates/lichen-language/examples/recursion.rs` — the
probe completes trips 1 and 10 and overflows on 100. So the figure above is the
low end on a thread with a larger stack, and the low end is a property of the
thread as much as of the walk.

**The overflow was `emit_node`, and it is now a named refusal.** Instrumented
first-hand, the walk reaches level ~175 of ~1 MiB of main-thread stack and dies
— and the `Flow` walk behind it never runs at all (a conditional lowers to a
`select`, so `lower_instrs` iterates a flat list). The walk's depth is the trip
count itself: an unmarked recursion is expanded, and every expanded copy nests
inside the previous one's else arm, so the graph is a chain. Measured on the
probe, `depth ≈ 18 + 3.1 × trip` — trip 1 reaches 21, trip 10 reaches 49.

**Why `#[stacksafe]` had not already covered it:** `lichen-compute` did not
depend on the crate at all, and `#[stacksafe]` only tests for room **at an
annotated frame** — so although `compile` is annotated and grows a segment, the
whole subtree below it shares that one segment and nothing inside ever asks for
another. Both halves were needed and neither was enough:

- `#[stacksafe]` on `emit_node` is what makes a deep body *survivable* at all:
  with the budget but no annotation, trip 100 still crashes below the limit.
- the budget is what makes it *bounded and named*: with the annotation but no
  budget, trip 400 compiles fine (answers 403 in 66 ms on `cpu`, 220 ms on `gpu`)
  at ~1260 levels and ~7 MiB of stack, and a trip of 4000 would do the same.

**The limit is 512 levels of the walk** (`MAX_KERNEL_BODY_DEPTH`,
`crates/lichen-compute/src/compute.rs`), which is about **160 expanded copies**
of a step this size. A constant, deliberately: a threshold derived from the
thread's stack would differ between a debug and a release build of the same
program, and a number that changes with the build profile is not one a program
can be written against — which is the bar this item sets. The refusal names the
limit, the cause (the expansion, not the program), and the fix: mark the
recursion `@loop`
([loop-conversion](loop-conversion.md) §1.1), or write a small trip count out by
hand. Before/after on the probe:

```text
                                   before                    after
  trip 100     thread 'main' has overflowed its stack    24.8 ms  103: ?a      (cpu)
                                                             55.7 ms  103: ?a      (gpu)
  trip 400     (never reached)                           refused: a kernel body's expression nests
                                                        more than 512 levels, so lowering it would
                                                        recurse deeper than a compile should spend on
                                                        its stack … Mark the recursion `@loop` so it
                                                        may become a dynamic loop instead of an
                                                        expansion (`docs/notes/loop-conversion.md` §1.1)
```

Note that a *dynamic* loop — one the JIT emits into the backend IR rather than
expanding — removes **both** of this item's ceilings by construction, which is
why
[`gpu-algorithm-roadmap.md`](gpu-algorithm-roadmap.md#41-axis-b-already-in-the-language-and-what-it-does-not-reach)
§4.1 measures an unrolled loop before recommending it, and is the argument for
`P1-33`'s operator being a builtin rather than a library function. **[Loop
conversion](loop-conversion.md)
keeps that argument and changes the operator**, and its Stage 1 is also the fix
shape for the second ceiling: the structured body is what turns the emitter's
400-to-1000 **crash** into a named refusal, independently of whether any loop is
ever written.

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
  rather than counting depth. Expect the pair to be the hardest item in the
  queue: the unfolding has to be well-defined for a cyclic graph.
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
  `garbage_collect`.

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

  Pinned by `lichen-language/tests/buffer_payload.rs` (a pointer comparison, not
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
  not on how it is spelled: `(1, 2)[0]` is `expected array<Int, Int>, found
  <Int, Int>` today and would be `1` under the spec. **Chosen — the doc fix:**
  the code's intent is explicit, its doc argues *why* (syntax picks the
  operator, never a runtime kind dispatch — the same rule that keeps `e[i]`,
  `a(k)` and `t{k}` three different things), every example agrees, and a
  language that *should* index tuples is a feature rather than a
  reconciliation. `P1-34` corrected §3's two sentences; nothing in the checker,
  the parser or the examples moved, so both reproductions still answer with the
  guard.

  *Rejected — teaching `check_index` the tuple and struct kinds:* it is the
  larger of the two and it would have to answer a question the deleted sentence
  answered badly — what a *struct's* positional index means, given that the
  spec's answer was "its wrapped tuple's elements" while the struct read has its
  own resolution through the struct's name table (`s.x`). A user who wants
  `(1, 2)[0]` to be `1` files that as a new item, and it starts by answering the
  struct question.

  One adjacent fact belongs with the decision rather than the item: the raw form
  `X<e>` *looks* like it could have spelled the spec's meaning without touching
  `check_index`, and it cannot — over a runtime container it reads components of
  something that is not a tuple type value, which is refused at check time (the
  container's type must be the tuple kind; `P1-35`), and on the tree this was
  written on was a **reported** runtime error where it used to print `none`
  silently (`P1-35`). So the choice really was between the
  two sides above; there is no third spelling available today.


# Incremental parsing / compilation for a typing editor

> Status: partially implemented — the T1 heart plus the O(edit) lex and
> statement-region re-parse primitives are landed; the session splice and
> T3/T4 remain.  Nothing here reflects work that was not done; the implemented
> stages are marked below.
>
> **Implemented (the "T1" heart):**
> - *Step 1* — the `Placeholder` / `ErrorBlock` split: `Expr::Err` now carries a
>   byte `range` (and `start`); the parser surfaces the recovered error regions
>   on `Program::error_blocks`; a recovered error lowers to a distinct
>   [`ExprKind::ErrorBlock`], never a `Placeholder`, and the checker **skips**
>   it (no cells, no unification, no cascade — it cannot emit a type-level
>   "expected X, found Y" from inside a region the user is still typing).
> * **One inert marker, errors absorbed at their layer (the P1 completion).**
>   The resolve stage no longer *stops* the pipeline: an *unresolved name*
>   lowers to the **same** [`ExprKind::ErrorBlock`] the parse layer uses (never
>   a new `Unresolved` variant), plus a `Resolve` diagnostic.  So lex, parse,
>   and resolve all absorb their errors as an opaque region + a diagnostic, and
>   the lower layers (compile → check) keep seeing the same effective,
>   name-free content no matter which error happened above — the checker needs
>   exactly one skip path, not a growing zoo of error nodes.
> * *T1* — a `BufferSession` (`crates/lichen-language/src/session.rs`) tracks a
>   **name-resolved content signature**: a hash over the tree *shape* with every
>   name replaced by the binding it resolves to (a stable slot, not its
>   spelling) or a single **unresolved** sentinel.  Extending an unresolved name
>   (the long-identifier typing case), rewriting an error block, or consistently
>   renaming a binding leaves the signature unchanged → the established
>   `IR`+`Build` is reused and only the fresh frontend/resolve diagnostics are
>   re-derived.  An edit that changes the resolved structure re-lowers and
>   re-checks (the debounced-rebuild fallback).
>
> **Implemented (the O(edit) lex / parse primitives — T2's heart):**
> - `lex::lex_resume` — re-lex only the region an edit touched, reusing the old
>   prefix and re-synchronizing with the old stream once a token is (kind, byte
>   range) identical at the shifted position.  `O(edit)` in regex work; the
>   lexer is stateless except `Glue` (immediately-preceded-by-no-trivia), so a
>   merge (`a b` → `ab`) or split (`ab` → `a b`) is handled.  Tokens keep
>   owning **byte ranges**.
> - `parse::parse_statement_region` — re-parse a contiguous *statement window*
>   into its statements, with the whole statement-list recovery, for incremental
>   splicing.
> - `Program::stmt_ranges` — the AST records each statement's **token-index
>   range** (`Vec<(usize, usize)>`, one per statement + one for the final expr);
>   tokens own byte ranges, so the session maps a changed byte region → token
>   indices → statements for re-parsing.  No byte-range duplication.
> - `Sig` (session signature) is now **per-statement**: `stmt_hashes: Vec<u64>`
>   plus an order-sensitive `combined()`, so the high (AST) layer can re-derive
>   only the statements an edit touched instead of re-hashing the whole tree.
>
> **Implemented (the `BufferSession::compile` splice — T2's wiring):**
> - `BufferSession` keeps a `LastState` snapshot (source, tokens, program) the
>   previous compile ran under.  On an edit, `compile` computes the minimal
>   byte span that differs (common-prefix/suffix), **re-lexes it with
>   `lex_resume`** and **re-parses only the statement window** the edit touches
>   (`parse::parse_statement_region_traced`, which also yields each statement's
>   absolute token range), then **splices** the fresh statements into the
>   snapshot's program — prefix kept, window replaced, suffix's token ranges
>   shifted by the token-count delta.  The result is identical to a whole-buffer
>   re-lex + re-parse; both the regex work and the parse window are `O(edit)`.
> - The splice is **conservative**: it falls back to a whole-buffer parse when an
>   invariant cannot be confirmed — a degenerate program, a recovered error block
>   lying outside the window (whose diagnostic must not be dropped), or a
>   trailing binding (the whole-program parser owns that "must end with an
>   expression" error).
> - The **signature is now incremental too**: `Sig` signs one hash per *top-level*
>   logical statement (each covering its own subtree), aligned with
>   `Program::stmt_ranges`.  `compile` re-signs **only** the spl
>   window plus a binding-shifted tail, and reuses the untouched statements'
>   hashes — so a keystroke re-hashes `O(edit)`, not the whole AST.  It
>   re-signs everything when the window's binding names change (a slot shift) or
>   the statement count changed, which keeps it sound.
>
> **Not yet wired / not implemented:**
> - T3 memoized check (`done`/`check_into` resume) and T4 true
>   unification-state checkpoint/rollback.
>
> Points at: `crates/lichen-language/src/{lex,parse,compile,lib,session}.rs`,
> `crates/lichen-highlevel/src/{ir,checker}.rs`.

## 1. The two principles

The design rests on two rules the user set out:

**P1 — clean layer boundaries.** Each layer (`lex → parse → compile(AST→IR) →
check`) contains **only its own errors**. An error recovered at layer `N` is
masked at `N` as an opaque *error block* (a byte-range region + a diagnostic); it
never crosses into layer `N+1` as a real construct. So a layer never has to
"understand" an error a higher layer already handled.

**P2 — diff-gated lowering.** Every lowering is memoized on the *beyond-error*
content of its input. On an edit, do a **quick diff** that asks "did the
well-formed content change?" — if only an error block changed, the previous
output is reused and no re-lowering happens.

Together they give exactly the requested behavior: a user typing a new
unfinished piece only ever mutates an error block, so nothing below it is
re-derived.

## 2. The current leak (why this matters)

The codebase already obeys P1 at the **lex** boundary: `lex_with` emits a
diagnostic and *skips* the offending character, so the parser never sees a lex
error. That is the model to generalize.

It **violates** P1 at the **parse → compile** boundary, and this is precisely
the conflation:

```rust
// compile.rs
Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span), // a real `_`
Expr::Err(span) => self.alloc(ExprKind::Placeholder, span),         // a recovered error
```

A recovered parse error becomes the **same** highlevel construct as an
intentional `_`. The checker then treats it as a real inference hole
(`checker.rs` `ExprKind::Placeholder => { fresh_cell(); fresh_cell(); … }`). So
the highlevel cannot tell "I typed `_` on purpose" from "the parser recovered a
broken region here". Consequences:

- The highlevel *does* carry the parser's error, just disguised.
- The error block participates in unification as if it were real code, so the
  **lowlevel/check unify** path can emit a *type* "expected X, found Y" from a
  region the user is still typing. Note this is a lowlevel/type message — **not**
  the parser's own diagnostic; the parser also emits its *syntactic*
  "expected X, found Y" (chumsky `RichReason::ExpectedFound`,
  in `parse::diag_from`), which is a distinct message and is unaffected by the
  masking decision below.
- There is no way to mask the error region for a diff, because it is not
  distinguished from real code.

To fix it, split the two meanings: a real `_` stays `ExprKind::Placeholder`;
a recovered-error region becomes an explicit error block that stops at the
frontend.

## 3. Why reuse is structurally possible here

Three properties the code already has:

1. **Token byte ranges are stable and absolute.** `lex::Token` carries both
   `span: (line, col)` and `range: (u32, u32)` byte offsets. Error regions can
   be described as byte ranges, and a diff over the remaining content can be
   computed cheaply.
2. **The IR is an append-stable dense arena.** `IR.expr: Vec<Expr>` indexed by
   `ExprId`; `alloc` pushes. Appending new `ExprId`s does **not** renumber
   existing ones, so a lowering that grew one statement at a time keeps its
   prefix ids stable.
3. **The checker memoizes per IR expression.** `Checker.term/val/ty/attr` are
   `Vec<Option<NodeId>>` indexed by `ExprId`. If the IR prefix keeps its ids,
   those results are reusable *if* the checker can skip already-built
   expressions.

## 4. The mechanism, layer by layer

The core idea: **an error block is a mask, and a content signature is the diff.**

At each boundary we keep, alongside the layer's real output, a list of masked
regions `(byte_start, byte_end)` and the set of diagnostics for them. The
*lowering signature* for a layer is a hash of only the **unmasked** content
(tokens outside error regions, AST subtrees outside error blocks). The layer
caches `signature → output`. On an edit:

1. Recompute the signature (cheap — hash the clean segments).
2. If it is unchanged, **reuse** the cached output; the only change is an error
   block, which is handled at this layer (re-emit its diagnostic, resize the
   mask).
3. If it changed, re-lower the affected clean segment, append to the cached
   output, and update the signature.

This is *content-addressed* reuse rather than position-addressed, so it works for
an arbitrary edit — not just an append — as long as the edit does not alter any
unmasked segment's signature.

### Lex → parse
`lex_resume(code, line_starts, base, byte_offset, prev_end)` re-lexes only from
the edit point. A lex error is a masked region (skip the char, record the
diagnostic), matching the current lexer. Tokens outside masked regions get
signed.

### Parse → AST
Keep the existing recovery, but represent a recovered error as an **opaque error
block**, not a code node that later layers consume as code:

```rust
// ast.rs: `Expr::Err(span)` today
enum Expr {
    …
    /// A recovered-error region: opaque, carried by the frontend only.
    /// `range` is the byte span its fallback covers; `start` the token where
    /// the broken construct began.  Holds no grammar — the lower layers must
    /// not treat it as an expression.
    Err { range: (u32, u32), start: Span },
}
```

`Program` also carries the list of `(Expr::Err mask, Diag)` blocks. The AST's
lowering signature hashes the statement `Expr` subtrees **excluding** these
blocks. Re-parsing is needed only when a signature changes: an edit that only
stretches the trailing error block changes no statement signature.

### AST → IR (highlevel)
The fix that satisfies P1: **do not lower an error block into the highlevel as a
`Placeholder`.** Two options, both cleaner than today:

- **(a) Mask it out.** Lower the well-formed statements; an error block is *cut
  out* (or replaced with a distinct `ExprKind::ErrorBlock` leaf the checker
  *skips* — no cells, no unification, no cascade). Choose this for a 1:1 lowlevel
  representation.
- **(b) Stop the pipeline.** If the required root is itself an error block,
  don't run the highlevel check at all; report only the frontend diagnostics.

Recommend **(a)**: it keeps the pipeline total (there is always a root to check,
the statements still get checked) while making the error region an explicitly
non-code object. The essential point is that the highlevel no longer has to
interpret a recovered error as a real expression — and **it never sees one**.

Concretely, `Checker` gets a skip path for `ExprKind::ErrorBlock` (like its
`Static` is handled): record the region as masked, emit nothing, mark `done`.
The IR lowering signature covers the *clean* statements only.

### IR → check
With P1 in place, the check layer sees only well-formed code plus masked regions.
Because a masked region is not checked, it cannot introduce spurious
"expected…found…" errors, and it cannot re-unify the established program. The
checker memoizes `term/val/ty/attr` per `ExprId` and skips any `ExprId` already
built (`done`), so re-checking after an error-only edit does nothing.

## 5. Design tiers (each independently useful)

- **T1 — error masks + per-layer lowering signature.** Add byte-range error
  blocks at parse, split `ExprKind::ErrorBlock` from `ExprKind::Placeholder` at
  the IR, and give each lowering a cached `signature → output`. This alone
  delivers the user-visible case: appending an unfinished piece reuses the
  established AST/IR/check and only re-processes the error block. **This is the
  cheap, high-confidence win** and the heart of the request.
- **T2 — suffix lex + statement-region parse** (`BufferSession`, `lex_resume`).
  Optimizes the *recompute* cost on large files (no 16 MB-stack re-thread, no
  whole-file re-parse). Optional on top of T1.
- **T3 — memoized check** (`done`/`term` skip + `check_into` resume). The full
  "do not re-check the established program" effect.
- **T4 — the honest boundary** — true incremental checking with unification-state
  checkpoint/rollback. Large change to `Checker`/`Module`; not warranted for an
  editor. The error-only case (the common one) is fully covered by T1+T3; a
  general edit falls back to a debounced re-check.

## 6. Example usage

```rust
let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");

// User types `g = x =>` (unfinished tail) → a masked error block, nothing else.
sess.insert(22, "g = x =>");
let r1 = sess.compile();     // signature (clean statements) unchanged → reuse IR/check

// User types ` x +` → still an error block, still clean statements.
sess.insert(28, " x +");
let r2 = sess.compile();     // same clean signature → reused; only the mask grows
assert_eq!(r1.signature, r2.signature);
```

Because the mask is excluded from the signature, `r2` needs no re-lowering of
`a`,`f`,`f 2`. The frontend re-emits only the error block's diagnostic.

## 7. Where the changes land

| Stage | File / function | Change |
|---|---|---|
| ast | `src/ast.rs` `Expr::Err`, `Program` | carry a byte `range`; surface the error-block list on `Program` |
| parse | `src/parse.rs` `parse_inner`, `diag_from` | record `(range, diag)` per recovered region; keep the AST mask, don't rely on `Placeholder` |
| compile | `src/compile.rs` `compile_expr` `Expr::Err` arm | lower to a distinct `ExprKind::ErrorBlock` / cut the region out; never a `Placeholder` |
| checker | `src/highlevel/checker.rs` `check_expr` | a skip path for `ExprKind::ErrorBlock` (emit nothing, mark `done`) + a `done` memo / `check_into` resume |
| IR | `src/highlevel/ir.rs` `ExprKind` | add `ErrorBlock` (or define the mask at the frontend and keep lowlevel clean) |
| API | `src/lib.rs` | `BufferSession`, per-layer signature, `compile`/`check_into` accessors |

## 8. Feasibility, risks, the genuinely hard parts

- **The `Placeholder` overload must be split.** Today a recovered error and an
  intentional `_` are the same IR node. This is the root cause of the leak; fix
  it before any incremental work, since the signature/diff hinges on being able
  to *identify* an error region distinctly.
- **Error regions need a byte `range`.** `Expr::Err` and the parse diagnostics
  carry only `(line,col)`. The parser computes positions from token indices, so
  it can derive byte ranges from `Token::range`; this should be recorded on the
  mask.
- **Name resolution is whole-program.** A block-wide binding pre-enters one
  scope frame before any value compiles. Reusing a compiler across edits is safe
  *if* it appends `ExprId`s without renumbering; a shadowing tail binding must
  invalidate the affected name (a T3 region-invalidation case, not the default).
- **Masking changes diagnostics (careful — two kinds of "expected…found…").**
  Cutting an error block out means the **lowlevel/check unify** path no longer
  processes the region, so it can't emit a *type* "expected X, found Y" from
  *inside* it. The **parser's own syntactic** "expected X, found Y" (chumsky
  `ExpectedFound`) for that region still fires at the parse layer — masking does
  **not** suppress syntax feedback. So the win is a reduction in *type-level*
  noise while the user is typing, with syntax diagnostics preserved; that is a
  deliberate behavior change worth noting.
- **Content vs position addressing.** The signature is content-addressed, so it
  survives arbitrary edits; only a change that alters an unmasked segment's
  signature forces re-lowering. This is strictly more general than an
  append-only approach, at the cost of hashing the clean segments per edit
  (cheap).
- **Thread/stack.** `parse::parse` spawns a 16 MB worker thread every call. T2
  should thread the parser's construction once (or per masked region) rather
  than per whole file.

## 9. Roadmap

1. **Split `Placeholder` vs `ErrorBlock`** and add byte-range error masks at
   parse. ✅ (the prerequisite for every diff/reuse decision).
2. **T1**: per-layer lowering signature + cached `signature → output`; error-only
   edits reuse everything. ✅ (name-resolved signature; the user-visible behavior).
3. **T2**: suffix lex + statement-region parse. ✅ primitives landed
   (`lex::lex_resume`, `parse::parse_statement_region`, `Program::stmt_ranges`),
   plus the per-statement signature. ✅ *wiring*: the `BufferSession::compile`
   splice re-parses only the touched statement window (falling back to a full
   parse on the borderline cases) and re-signs the name-resolved signature
   incrementally — reusing the untouched statements' hashes and re-hashing only
   the window + a binding-shifted tail, so the whole-AST hash is gone.  A
   binding-name change or a statement-count change re-signs the tail (sound).
4. **T3**: memoized check (`done`/`term` skip, `check_into` resume).
5. **Debounced re-check** as the fallback for edits that genuinely change code.

Steps 1–4 together implement the rule: when the user writes new unfinished code,
the error block is contained at the parser layer and the established AST/IR/check
is reused because a quick beyond-error diff says nothing else changed.

# Incremental parsing / compilation for a typing editor

> Status: current — the T1 heart, the O(edit) lex and statement-region
> re-parse primitives, and the `BufferSession::compile` splice are shipped and
> described as shipped below; T3 (memoized check) and T4 (unification-state
> checkpoint/rollback) remain proposed.  The implemented stages are marked below.
>
> **The keystroke path is still open** (`P1-17` (b)): the language server runs a
> `BufferSession` per open document on its compile worker and keeps its own
> per-text index (`P1-17`), falling back to a one-shot analysis for a compile the
> session cannot do — so the splice below runs on the server, but the
> finer, keystroke-granularity edit path that would avoid the lex and parse when
> the text *changes* is not wired.
>
> The session also carries the **cross-build** half: it holds a cell store and
> lowers through it, so a rebuild reuses every `cache`d binding the edit did not
> reach.  That is [incremental-update](incremental-update.md) — §3 for the mark
> and §4 for dirty propagation — and nothing on *this* note's stages
> (lex → parse → resolve → lower → check) changed for it.
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
> * *T1* — a `BufferSession` (`crates/lichen-language/src/session.rs`) runs the
>   `resolve` stage (a single `BinderId` resolver,
>   `crates/lichen-language/src/resolve.rs`) on every compile and tracks the
>   **resolved content key**: a name-free, digest-free serialization over the
>   resolver's `BinderId`s (each name replaced by the binding it resolves to,
>   error blocks opaque, spans dropped).  Extending an unresolved name (the
>   long-identifier typing case), rewriting an error block, or consistently
>   renaming a binding leaves the key unchanged → the established `IR`+`Build` is
>   reused and only the fresh frontend/resolve diagnostics are re-derived.  An
>   edit that changes the resolved structure re-lowers and re-checks (the
>   debounced-rebuild fallback).
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
> - **Name resolution is a single stage** (`resolve.rs`): the resolver assigns
>   each binder a dense `BinderId` and writes it into the AST's resolve fields
>   (`Expr::Name` use, `Binding`, lambda parameter, record field), and is the
>   only authority for the scope rules.  The compiler reads those fields through
>   a `BinderId → ExprId` map (one IR node per binder) instead of re-resolving —
>   the "a use is the binder's own id" graph-sharing invariant holds by
>   construction.  The session's reuse compares the resolver's `BinderId`
>   annotations (via `content_key`) directly rather than hashing resolution.
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
> - **The reuse gate is digest-free**: the session re-resolves the spliced
>   program and compares its `content_key` (a name-free serialization over the
>   resolver's `BinderId`s, error blocks opaque).  Equal key ⇒ the resolved
>   structure is unchanged ⇒ the established build is reused; no `sha2` digest
>   and no collision ambiguity.  The splice's window still bounds the re-parse,
>   but the reuse decision re-walks the (small, already re-parsed) program
>   rather than diffing per-statement hashes.
>
> **Not yet wired / not implemented:**
> - T3 memoized check (`done`/`check_into` resume) and T4 true
>   unification-state checkpoint/rollback.
>
> T3 and T4 are not only a checking concern: the checker *is* the evaluator
> (its definition pass runs the program), so "resume the check" and "reuse the
> evaluation" are one problem with a representation choice attached.  That
> choice, and the deep pass's own lack of memoization, are worked out in
> [incremental-evaluation](incremental-evaluation.md) — which is where T3's
> remaining scope is stated; this note stays the account of `lex → parse →
> resolve → lower → check`.
>
> Points at: `crates/lichen-language-lex` (`lex_resume`),
> `crates/lichen-language-parser` (`parse_statement_region*`, `Program::stmt_ranges`),
> `crates/lichen-language/src/{resolve,compile,lib,session}.rs`,
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

## 2. Why the split is load-bearing

The codebase already obeys P1 at the **lex** boundary: `lex_with` emits a
diagnostic and *skips* the offending character, so the parser never sees a lex
error. That is the model the parse boundary follows.

It is the **parse → compile** boundary where conflating the two would break P1.
The conflation to avoid is exactly this lowering:

```rust
// compile.rs — WRONG: both arms are the same construct
Expr::Placeholder(span) => self.alloc(ExprKind::Placeholder, span), // a real `_`
Expr::Err(span) => self.alloc(ExprKind::Placeholder, span),         // a recovered error
```

If a recovered parse error became the **same** highlevel construct as an
intentional `_`, the checker would treat it as a real inference hole
(`checker.rs` `ExprKind::Placeholder => { fresh_cell(); fresh_cell(); … }`), and
the highlevel could not tell "I typed `_` on purpose" from "the parser recovered a
broken region here". The consequences are why the two are separate today:

- the highlevel would carry the parser's error, just disguised;
- the error block would participate in unification as if it were real code, so the
  **lowlevel/check unify** path could emit a *type* "expected X, found Y" from a
  region the user is still typing. That is a lowlevel/type message — **not** the
  parser's own diagnostic; the parser also emits its *syntactic* "expected X,
  found Y" (chumsky `RichReason::ExpectedFound`, in `parse::diag_from`), which is
  a distinct message and is unaffected by the masking;
- there would be no way to mask the error region for a diff, because it would not
  be distinguished from real code.

So the two meanings are split: a real `_` is `ExprKind::Placeholder`; a
recovered-error region is an explicit error block that stops at the frontend.

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
The rule that satisfies P1: **an error block is never lowered into the highlevel
as a `Placeholder`.** Two options:

- **(a) Mask it out.** Lower the well-formed statements; an error block is *cut
  out* (or replaced with a distinct `ExprKind::ErrorBlock` leaf the checker
  *skips* — no cells, no unification, no cascade). This is the 1:1 lowlevel
  representation.
- **(b) Stop the pipeline.** If the required root is itself an error block,
  don't run the highlevel check at all; report only the frontend diagnostics.

The implementation takes **(a)**: it keeps the pipeline total (there is always a
root to check, the statements still get checked) while making the error region an
explicitly non-code object. The essential point is that the highlevel never has to
interpret a recovered error as a real expression — and **it never sees one**.

Concretely, `Checker` has a skip path for `ExprKind::ErrorBlock` (like its
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
  "do not re-check the established program" effect.  Its evaluation half — what
  a reused answer is allowed to be, and the deep pass's own missing memo — is
  [incremental-evaluation](incremental-evaluation.md).
- **T4 — the honest boundary** — true incremental checking with unification-state
  checkpoint/rollback. Large change to `Checker`/`Module`; not warranted for an
  editor. The error-only case (the common one) is fully covered by T1+T3; a
  general edit falls back to a debounced re-check.

## 6. Example usage

```rust
let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");

// User appends `g = x =>` (an unfinished tail) → a masked error block, nothing else.
sess.set_source("a = 1\nf = x => a + x\nf 2\ng = x =>");
let r1 = sess.compile();     // clean signature unchanged → reuse IR/check

// User completes it to `g = x => x +` → still an error block, still clean statements.
sess.set_source("a = 1\nf = x => a + x\nf 2\ng = x => x +");
let r2 = sess.compile();     // same clean signature → reused; only the mask grows
assert!(r1.reused && r2.reused);
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

- **The `Placeholder` overload must stay split.** A recovered error and an
  intentional `_` must not be the same IR node: the signature/diff hinges on being
  able to *identify* an error region distinctly, and the checker's skip path is
  what keeps the two apart.
- **Error regions need a byte `range`.** A parse diagnostic carries only
  `(line,col)`. The parser computes positions from token indices, so it derives
  byte ranges from `Token::range`; the mask records them.
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
- **Thread/stack.** The grammar recurses deeply enough to exceed a normal stack,
  so `parse::parse` runs on a **process-lived `ParseWorker`** with a 16 MB stack
  rather than the caller's thread; per-parse thread creation was itself a
  measurable cost, so the worker is reused and parses are serialised through it.
- **The window's byte projection is the splice's sharp edge.** The splice
  re-parses the token range
  `[ns, ne)` and splices it between the untouched prefix and suffix, so it needs
  the window's *new* byte range and the suffix's token shift. Two rules are
  load-bearing, and both fail on an edit that **deletes whole statements**,
  because such an edit also deletes the separator between the window and the
  suffix:
  - the window's new **end** is projected from the **suffix's** first byte (at or
    after the edit end by construction), *not* from the window's own last token,
    which ends inside the replaced region;
  - the suffix's shift is measured from the suffix's **own** first token in each
    stream — the same token on both sides — *not* from the window's end token,
    which the edit may have deleted.
  Project either and the window cuts through the statement that follows: a
  truncated binding is re-parsed, the suffix is spliced after it, statements are
  duplicated, and a `stmt_ranges` entry can end one past the token stream — the
  next splice then panics on `old_tokens[old_th - 1]`. The corrupted program
  still *evaluated* correctly, so the tell is a spurious parse diagnostic and
  the later panic, not the value. An **empty** window (`ns == ne`, prefix meets
  suffix) is legal and must not be handed to the region parser, which requires at
  least one statement.
  The oracle for it is [incremental-update](incremental-update.md) §9's
  differential comparison (value *and* diagnostics against a fresh compile, over
  every prefix of an edit sequence); a count-only reading misses it.
- **A cloned statement's spans must be shifted, and a boundary edit must not widen
  the window.**  Two rules, and they interact:
  - a **clone's spans are stale**.  The splice clones the untouched statements
    around the window, and a clone's bytes are unchanged but its *position* is
    not — a `Span` is a `(line, col)` pair, so an edit that adds or removes a
    **line** moves every statement after it.  A diagnostic in a cloned statement
    then renders on the wrong line.  `lichen-language`'s `spans` module walks
    the clone and rewrites every span — and a recovered error's byte range —
    through `offset_of_span`/`line_col`, which is exact.  A *wider* window hides
    this by re-parsing; it does not fix it, and the overlap path is exposed either
    way.
  - the **boundary fallback** (`no statement body overlaps` — a separator edit, or
    an append at a statement's end) must window `[prev, prev + 2)`, not
    `[prev, old_n)`.  Widening to the end of the buffer re-parses and re-dirties
    the whole tail: it costs most of a rebuild and drops most retained cells, for
    the edit shape an agent produces most often (appending to a line).  Two
    statements rather than one, because the insertion is inside the
    byte range spanning them and the region parse is byte-bounded — several
    inserted statements are re-parsed too.

## 9. Roadmap

The named tiers and their state; T3 and T4 are **proposed**, not built.

1. **Split `Placeholder` vs `ErrorBlock`** and add byte-range error masks at
   parse — landed (the prerequisite for every diff/reuse decision).
2. **T1**: per-layer lowering signature + cached `signature → output`; error-only
   edits reuse everything — landed with the name-resolved signature (the
   user-visible behavior).
3. **T2**: suffix lex + statement-region parse — the primitives landed
   (`lex::lex_resume`, `parse::parse_statement_region`, `Program::stmt_ranges`),
   plus the per-statement signature, and the `BufferSession::compile` splice
   re-parses only the touched statement window (falling back to a full parse on
   the borderline cases) and re-signs the name-resolved signature incrementally —
   reusing the untouched statements' hashes and re-hashing only the window + a
   binding-shifted tail, so the whole-AST hash is gone. A binding-name change or
   a statement-count change re-signs the tail (sound).
4. **T3 — proposed**: memoized check (`done`/`term` skip, `check_into` resume).
5. **Debounced re-check** as the fallback for edits that genuinely change code.

Steps 1–4 together implement the rule: when the user writes new unfinished code,
the error block is contained at the parser layer and the established AST/IR/check
is reused because a quick beyond-error diff says nothing else changed.

## Recovered measurements

- **`lex_resume` is `O(edit)` in the regex work only.** It re-lexes the changed
  region and re-synchronizes against the old stream, but the value it returns is
  a fresh `Vec`, so materializing the result copies the reused prefix and suffix
  — linear in the file's **token count**, not in the edit. A caller that must be
  `O(edit)` end to end has to splice the returned token slice into its own
  structure rather than treat the returned `Vec` as free.

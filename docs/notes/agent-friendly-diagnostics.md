# A diagnostic an agent can act on: a property, and where this codebase meets it

> **Status: the property is stated and one case is fixed.** The unclosed-delimiter case is fixed
> (`4cb42d3f`); the offender list below is **parser-heavy and under-covers the checker and the module
> layers**, which is the first thing to close before another diagnostics slice.
> Points at: `crates/lichen-language-parser/src/parse/diagnostics.rs`,
> `crates/lichen-language-parser/src/parse.rs`, `crates/lichen-highlevel/src/diagnostic.rs`,
> `crates/lichen-highlevel/src/checker/`.

## 1. The property

The user's ruling, in their words: **"报错必须追求对 agent 友好"** — error reporting must aim to be
agent-friendly. Stated so that a message can be judged rather than admired:

> **A diagnostic is agent-friendly when a reader who cannot ask a follow-up question can act on it.**

Four checkable parts, each with the way it fails:

| | Property | Fails when |
|---|---|---|
| **P1** | names the **fix** | the message says what was wrong but not what to write |
| **P2** | the span is the **earliest actionable** place | the span is a position the reader can do nothing at |
| **P3** | no **misleading head** | the first of several diagnostics is about a later mistake |
| **P4** | **one diagnostic per mistake** | one mistake yields several, or one mistake yields none |

## 2. Where this codebase stands, measured

* **P1 — the checker satisfies it structurally.** `crates/lichen-highlevel/src/diagnostic.rs` carries a
  `DiagKind` taxonomy and `UnifyEntry { a, b, loc, kind }` (`:196-213`), so a unification failure renders an
  expected/found pair rather than free prose, and `AssertSpelling` exists to say more than the channel's
  generic *"expected 1, found …"* (`:215-223`). The **parser** satisfies P1 only while the expected-label
  union is short, and fails it when the union is long.
* **P2 — the checker satisfies it**: failure sites pin the mismatch to a specific spine slot
  (`checker/operators.rs:36`, `checker/indexing.rs:49`). **The parser cannot** — see §5.
* **P3 — neither satisfies it.** The cascade is emitted in **chumsky's order**
  (`parse.rs:164`, loop at `:184-193`). The fix below improves it; it does not solve it.
* **P4 — neither satisfies it.** The dedup at `parse.rs:184` keys on `(span, message)`, so one message at
  two spans survives twice (§4, offender 2).

## 3. The case that raised it, fixed

`examples/decode_loop.lichen:174` with **one `)` removed** from `.at (s(0) - 1)`.

**Before** — and the head did not contain a `')'` label **at all**, so no reading of the first line could
have found the mistake:

```
expected a separator, 'return', 'pub', '@loop', … or '~', found '-'
```

**After**:

```
unclosed '(' opened on line 174: expected a separator, 'return', 'pub', '@loop', … or '~', found '-'
```

One command:

```
cargo test -p lichen-language-parser --lib unclosed_delimiter_probe -- --nocapture
```

**The mechanism, and why it took two halves.** This is an error **identity** problem, not an error
**position** problem: the reported position really is the furthest failure a PEG can give, and **sorting
the cascade would move the head three lines further from the edit**. So the fix adds a per-diagnostic
clause (`diagnostics.rs:52`) *and* a head rule (`parse.rs:151`) that carries the earliest unclosed-delimiter
fact onto the head when a **later** diagnostic is the one that found it. The **span is untouched**
(`e.span().start` as before; `an_unclosed_delimiter_is_reported_where_the_parse_stops` still passes).

**The reserved surface was not touched, and here is the proof.** `diagnostics.rs:32` **prepends**; it never
substitutes. The expected-label list in the before/after above is **character-for-character identical**,
with the clause in front of it. No grammar rule, `choice` or label set changed —
`parse-clean-probe.md:33-38` reserves that surface, and it stands.

## 4. The offenders met in practice, and what was left

| # | Site | Reproduction | Verdict | Taken? |
|---|---|---|---|---|
| 1 | `diagnostics.rs:8` (was) | `decode_loop.lichen:174` − one `)` | P1, P3 fail | **fixed** (`4cb42d3f`) |
| 2 | `parse.rs:184`, dedup key `(span, message)` | the same probe: `[0]` at `(1,1)` and `[1]` at `(151,1)` are **byte-identical** 25-label messages | P4 fails | listed |
| 3 | `parse.rs:203` `"the program could not be parsed"`, span `(1,1)` | the same probe: `[4]` | P1 **and** P2 fail — no information, and the span is the whole-file fallback (`parse.rs:183`) | listed |
| 4 | cascade composition | the same probe: `[3]` at `(172,27)` is about a different region than `[0]` | P3, P4 | listed |

**Why one fix and not four.** Offender 2 looks like a one-line change and is not: deduping by message alone
would hide a genuine second mistake that happens to share a wording — the honest fix is for the cascade to
carry the position **in the message**, which is a grammar-surface change. Offender 3 sits on the
`output == None` path, and a better span needs to know where the parse first went wrong, which is §5.

## 5. What a PEG cannot do about it, and what meeting it would cost

**P2 cannot be met for a PEG.** Where parsing stops *is* the furthest failure, and that is genuinely the
furthest failure. Meeting P2 needs either a different algorithm — error recovery over every prefix, or a
GLR that records the earliest failing token — or a **separate delimiter-balance pass that runs before the
PEG**, which is the cheap half of what this fix did and could be extended into a real standalone
diagnostic. **The first option costs exactly the surface `parse-clean-probe.md:33-38` reserves**: the
expected-label union *is* the record of the backtracking, so an algorithm that reports the earliest failure
instead of the furthest no longer produces that union.

## 6. What is not covered yet

The offender list is parser-heavy, and the checker and module layers were first claimed from a grep of
their sources. Two surveys have since gone over them, and **the slice that wrote this note verified their
load-bearing claims itself** — but *"neither survey ran anything: every claim is read-verified, not
reproduction-verified"*. So this section records **reads with quoted strings**, not measurements, and the
gap is **narrowed, not closed**.

**Two corrections to what the surveys first said, both worth more than the claims they replaced.**

* **`TableMiss` is a P1-only failure, and its fix is one field away.** The survey said the key is never
  printed; it is — as the **span**. `crates/lichen-highlevel/src/diagnostic.rs:462-464` builds it as
  `Diag::factual(DiagKind::TableMiss, self.node_loc(*key))`, so the caret lands on the key and **P2
  holds**; only P1 fails, because `render.rs:261` is a literal — *"table lookup missed — no entry for this
  key"*. The key is bound on that line and consumed only as a location, so printing it makes the message
  answerable — the reader can see **which** key to add.
* **`DiagKind::IndexTarget` is dead code, and its message was already written.** It is defined
  (`diagnostic.rs:33`), rendered with the right words (`render.rs:173`: *"expected a tuple, array, or
  struct type, found {}"*), and **produced nowhere** — the other `IndexTarget` hits are
  `RuntimeIndexTarget` (a distinct runtime variant) and `EvalError::IndexTarget` (a different type in
  another crate). So the correct message for `expected TypeArray, found Type` exists and was **never
  wired**. Wiring it is a **semantic** change to which diagnostic exists, on a path whose test
  (`crates/lichen-highlevel/tests/checker.rs:738`, named
  `indexing_a_function_reports_expected_tuple_or_array`) was not read.

**And one claim confirmed verbatim**: `crates/lichen-preprocess/src/lib.rs:429-432` builds
`PreprocessDiag::unattributed`, so a preprocessor diagnostic carries **no span** even though the
directive's offset is known to the block parser — a P2 failure on a surface no parser rule touches.

**Still unmeasured**: whether the `?a -> ?b` expected side renders as claimed (`tests/checker.rs:713-736`
supports the value structure but the rendered string was not captured); whether `checker.rs:738` already
pins the `IndexTarget` wording; the positional-struct `raw[Int, Int]` offender, whose fix belongs to
`crates/lichen-render/src/render/type_printer.rs` under `docs/notes/type-rendering-inconsistent.md`; and the
two facts unchanged from the main report — the 37 examples do not run on `dev` (`invalid SlotMap key used`,
`crates/lichen-lowlevel/src/evaluation.rs:517`, pre-existing) and the eight named compute binaries were
never run.

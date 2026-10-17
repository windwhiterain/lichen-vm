# A diagnostic an agent can act on: a property, and where this codebase meets it

> **Status: the property is stated and seven cases are fixed.** The unclosed-delimiter case is fixed
> (`4cb42d3f`), the preprocessor's missing span is fixed (§7), the `NamedField` miss now blames the
> read (P2), the report's check-level assert now heads its runtime consequences (P3), an unknown
> field no longer also reports the gap it explains (P4), and a type access's miss now names its
> struct type (P1) — all in the
> corrections below; the offender list below is
> **parser-heavy and under-covers the checker and the module
> layers**, which is the first thing to close before another diagnostics slice — the two cases above are
> the checker layer's first reproduction-verified entries.
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
reproduction-verified"*. So this section records **reads with quoted strings**, and the one item that was
reproduction-verified since is `IndexTarget` below; the gap is **narrowed, not closed**.

**Eight corrections to what the surveys first said, each worth more than the claims they replaced.**

* **An unknown field name also reported the gap it explains, and now it does not.** `S = struct{.a Int}`
  ; `S(.b 1)` reported two diagnostics — `no field named b in the struct type struct{.a Int}` and
  `missing field a in struct instantiation` — for one typo. The unknown name explains the gap, so the
  missing-field pass now skips an instantiation that has an unknown name: one typo is one diagnostic
  (P4), and a genuine omission (`S(.a 1)` on a two-field struct) still reports the missing field.
  The unknown-field message now also appends the struct's did-you-mean clause —
  `S(.sbu 1)` is `no field named sbu in the struct type struct{.a Int, .sub Int}#11, did you mean
  'sub'?` — the clause the field read already carried, so an instantiation typo names its fix too.
  The three other instantiation mismatches now name the struct as well:
  `missing field y in the struct type struct{.a Int, .y Type}#11`,
  `duplicate field a in the struct type …`, and
  `too many fields for the struct type struct{.a Int}#11`; before, a reader had to
  find the instantiation to learn which struct was meant.

* **The check-level assert was printed after the runtime consequences, and the assert
  channel is now gone from the named read.** `f = x => x.a` ; `f [10, 20]` reported five
  diagnostics: four runtime read failures (`this value is not a container`, `table lookup
  missed`, twice each) and the actionable `expected a struct type, found array{Int, 2}`.
  `Build::diagnostics` pushed `assert_errors` after `eval_errors`, so the head of the
  report was about consequences and the cause came last — the P3 failure, verbatim, and
  the asserts block then preceded the runtime block. The author's unify-first ruling
  removed the assert tier for the named read (see [eval-before-unify](eval-before-unify.md)),
  so the program now reports **one** diagnostic, at the read's own site — `this value is
  not a container — it has no element to read` at `1:10` — and the ordering rule holds
  wherever asserts remain.

* **An apply of a deferred non-function was reported twice, and it now reports once.**
  `f = g => g 1` ; `f 5` printed `this value is not a function — it cannot be applied`
  at `1:5` **twice**: `check_app` builds a value apply and a type apply at one site, and
  the runtime-failure dedup keyed by the blamed **node** kept both. That kind is now
  keyed by the error's resolved **location**, so one mistake is one diagnostic — the
  two `an_apply_of_a_deferred_*` tests pass, and the language suite went 7 red to 5.

* **A type access's miss named no struct, and it is fixed with the renderer fallback it needed.**
  `S = struct{.a Int, .sub Int}` ; `S::sux` reported `table lookup missed — no entry for key "sux"`
  at `1:1` — naming neither the struct type nor a close field — while the instance form `s.sux`
  reports `no field with this name in the struct type struct{…}, did you mean 'sub'?`. That is P1.
  The guard does not reuse `check_named_field`'s shape helpers — the struct's name table lies on the
  **kind** path the runtime reads (`lazy_index_path(container_ty, STRUCT_KIND_NAMES_PATH)`, and the
  container's value answers `is_struct = false`, `names = false`). It evaluates that path at check
  time, decodes its `LowValue::Table`, and refuses a missing name as `NamedField` at the read. The
  message then needed the renderer fallback: a struct type **value** has no `[shape, kind]` term for
  the type printer, so the arm prints the container through the value printer with the recorded
  container type (`Diag::b`), yielding `no field with this name in the struct type
  struct{.a Int, .sub Int}` at `2:1`. The did-you-mean clause does not need the type term after
  all: the arm reads the runtime's own name table off the kind path (`kind[0][0][1]`) and suggests
  from its entries — `S::sux` is `…, did you mean 'sub'?`, while a name far from every field
  (`S::aaaa`) gets no clause. The suites hold at their baselines (highlevel 140/0, language 265/11,
  compute 221/2/2, language-server 21/1).

* **A `NamedField` miss blamed the container's definition, not the read, and it is fixed.** A program
  with `a = A(1,)` then `a.z` (a struct without `.z`) rendered `--> 2:5` — the `A(1,)` instantiation —
  because `check_named_field` recorded the guard at `self.loc(container, 1)` and the container
  expression is the *substituted* instantiation, not the use. Blaming the read expression instead
  (`self.loc(e, 0)`) puts the caret on `a.z` (`3:1`; `3:5` inside `q = a.z`). This is a **P2** failure
  worth its own entry: the old position mentions neither `.z` nor the struct's fields, so a reader who
  cannot ask again was sent to the wrong line. The suites hold at their baselines.

* **`TableMiss` was a P1-only failure, and it is fixed.** `91d236fd` ("a missed table read names the key it
  looked up") gave `Diag` a `table_key` slot, filled it from `self.module.node_value(*key)` at
  `diagnostic.rs:469-472`, and made `render.rs:261-264` answer *"table lookup missed — no entry for key
  {key}"*, with the old literal kept as the fall-back for an unreadable key. P2 already held — the caret
  lands on the key — so the message now names the key as well, which is the whole of P1.
* **`DiagKind::IndexTarget` was dead code, its message *obsolete* rather than merely unwired, and both are now
  deleted** — the kind, its doc comment, and the `render.rs` arm that spelled it. It was
  defined (`diagnostic.rs:33`), rendered with words that no longer held (`render.rs:173`: *"expected a
  tuple, array, or struct type, found {}"*), and **produced nowhere** — the other `IndexTarget` hits
  are `RuntimeIndexTarget` (a distinct runtime variant) and `EvalError::IndexTarget` (a different type
  in another crate). **Measured, not read**: the wiring was applied, run through the language front end
  and reverted. `check_index` (`checker/indexing.rs:49`) refuses a concrete non-array target with
  `DiagKind::Guard`, so `render.rs`'s `Guard` arm prints the **kind it requires**; flipping that one
  call site to `IndexTarget` swaps the kind and nothing else, and the spans are unchanged. **After the
  deletion the suites are unchanged**: `lichen-highlevel` 140 passed / 0 failed, and `lichen-language` 260
  passed / 11 failed, the same eleven names as before — the kind produced nothing, so removing it can move
  nothing.

  | program | `Guard` (today) | `IndexTarget` (wired, reverted) |
  |---|---|---|
  | `a = x => (1, Int)(x); a[0]` | `expected TypeArray, found ?a -> ?b` | `expected a tuple, array, or struct type, found ?a -> ?b` |
  | `f = (x => x); f[0]` | `expected TypeArray, found ?a -> ?a` | `expected a tuple, array, or struct type, found ?a -> ?a` |
  | `5[0]` | `expected TypeArray, found Type` | `expected a tuple, array, or struct type, found Type` |
  | `t = (1, 2); t[0]` | `expected TypeArray, found TypeTuple` | **`expected a tuple, array, or struct type, found TypeTuple`** |
  | `s = [1, 2]; s[0]` | *(checks)* | *(checks)* |
  | `r = "ab"; r[0]` | `expected TypeArray, found Type` | `expected a tuple, array, or struct type, found Type` |

  **The bold row is why the wording is obsolete rather than ready.** `t[0]` reads an index of a
  **tuple**, and this path refuses it — so the wired sentence names a tuple as an acceptable target
  while a tuple is exactly what was found. That is P1 failing with a *false* fix, which is worse than
  the sentence it replaces, and it is the language's stated rule rather than an accident of this
  check: `docs/language-spec.md:665-666` calls a **concretely** non-indexable type — *"a tuple, a
  struct, a function, a table or an atomic type"* — a refusal, and `:649-654` says each read form
  *"states the container kind it accepts"*, so `e[i]` accepts an **array** only (`a(k)` is the tuple
  read, `.name` the struct read). `render.rs:173` is a fossil of the design before those reads split:
  one operator that dispatched on the container's kind, and a union of shapes was then the honest
  sentence. **Two of its three named shapes are refused by this path today.**

  **And the pin is three tests, not one.** The wording was worth checking against, because wiring it
  changes the kind at three sites: `crates/lichen-highlevel/tests/highlevel/checker.rs:738`
  (`indexing_a_function_reports_expected_tuple_or_array`, asserting `DiagKind::Guard`),
  `crates/lichen-highlevel/tests/highlevel/checker.rs:766`
  (`indexing_an_int_reports_expected_tuple_or_array`, the same assertion for `5[0]`), and
  `crates/lichen-language/tests/language/pipeline.rs:2180`
  (`indexing_a_function_is_an_index_target_error`). Measured with the wiring applied:
  `lichen-highlevel` goes 0 → 2 failures and `lichen-language` 11 → 12, each new failure one of those
  three and nothing else.

  **The baseline those counts are against, measured `2026-10-17` on `dev` (`0dbcbcdc`)** — both
  crates' suites were run before any edit, so a reader can tell a regression from a pre-existing red.
  `cargo test -p lichen-highlevel -j 1`: **140 passed, 0 failed**. `cargo test -p lichen-language -j 1
  --no-fail-fast`: **261 passed, 11 failed**, and none of the eleven is an index-target read
  (`defer_pending` ×4, `gc_block::a_gc_block_releases_its_scope`, `pipeline` ×4, `registry` ×2 — all
  *"the program's root value is undecided"* or a deferred-import/apply fact). `indexing_a_function_is_an_
  index_target_error` **passes** on that baseline, so it is a clean signal for the wiring.

  **The current baseline, re-measured at `9fad30c7`** (the dynamic apply's result-cell bind; see
  [compute-kernel-struct](compute-kernel-struct.md)): lowlevel **146/13**, highlevel **140/0**,
  language **267/5** (the known five: `gc_block`, `pipeline` ×2 — the float and zero-divisor
  recordings — and `registry` ×2; the four `defer_pending` tests were deleted at the author's
  direction), compute **230/2/2**
  (`graph_jit` and
  `graph_structure`; the latter is the one-read refactor's witness, per
  [compile-order-class](compile-order-class.md) §4.7), language-server **22/0** — its known red
  (`compute_kernel_bindings_render_by_name_not_raw_layout`) is **fixed**. A future red is a
  regression against *this* line.

  **The measurement method, so it is cheap to redo**: the language front end, one
  `lichen_language::compile(&source)` call per program, reading `report.diagnostics` — the `message`
  is `render::checker_message`'s own output and `check.kind` the highlevel's kind, so the probe needs
  no harness of its own. `diags("…")` in `tests/language/pipeline.rs` is that call in test form; the
  `.lichen` CLI renders the same sentence through the same function.

**And one claim confirmed verbatim, now fixed**: `crates/lichen-preprocess/src/lib.rs:429-432` used to
build `PreprocessDiag::unattributed`, so a preprocessor diagnostic carried **no span** even though the
directive's offset is known to the block parser — a P2 failure on a surface no parser rule touches.
Taken; see §7 for the measurement and the `Invariant` it had to edit.

**Measured since**: the `?a -> ?b` claim is confirmed verbatim. The minimal program `5 6` renders

```text
error: expected ?a -> ?b, found Int
  --> 1:1
```

so the guard's expected side is the function the guard built, and it prints as an arrow with two undecided
slots — the test at `tests/highlevel/checker.rs:713-736` pins the value structure and this is the string it
produces. And `checker.rs:738` does **not** pin the `IndexTarget` wording: that test asserts the diagnostic's
kind, location and the requirement's `TypeArray` kind, never a rendered sentence — and neither does any other
test, so nothing in the tree would catch the obsolete wording changing.

**Closed as unreachable from source**: the positional-struct `raw[Int, Int]` residual. This section pointed at
[type-rendering-inconsistent](type-rendering-inconsistent.md), which has since **closed as unreachable**, so
the residual was measured instead of inherited. **The language refuses the spelling**: `S = struct{Int, Int}`
answers `error: a struct field must be named — write `struct{.name T}`` at the field (`1:12`), for the type
expression, for a naked use and for an instance alike — the same three programs, the same message. And the
printer's positional handling is **intentional, not a fallback**: `struct_field_names` returns `None` per
unnamed field (`render.rs:368-370`) and the struct arm renders `None` entries positionally and `Some` entries
`.name Ty` (`:443`). So the offending observation has neither a source spelling nor a printer bug behind it,
and what this section records is the measurement that closed it. A hand-built IR can still build an unnamed
field, so this is "unreachable from source", which is the stronger claim a front end can settle.

**Superseded here**: the two facts the main report carried are no longer the state of `dev`. The examples no
longer panic with `invalid SlotMap key used` — the read path's collection is disabled while its trigger is
repaired ([`gc-trigger-disabled.md`](gc-trigger-disabled.md)), and `examples::every_example_runs_and_prints_what_it_declares`
fails on its own assertion instead. The "eight named compute binaries" no longer exist: every crate's
integration suite is one binary now, and the compute one runs 220 tests.

## 7. What became of the survey's items

**`TableMiss` now names the key** (`91d236fd`). `Diag` gained `table_key: Option<P::Value>`, **mirroring
`IndexOutOfBounds`'s `index`/`length`** — so the key stays a **value**, not a rendered sentence, and the
source-blindness invariant at the head of `diagnostic.rs` is untouched. Measured on the language front end:

```
t = table(1 ==> 10, 2 ==> 20)   t <== 7
before: table lookup missed — no entry for this key
after:  table lookup missed — no entry for key 7
```

The span is unchanged at `(2,7)`, so **P2 still holds** and only the message gained the key. **Only scalar
keys** are rendered: a non-scalar key needs the key's *type* node, and value and type are separate
`ExprState` nodes rather than offsets, so there is no O(1) derivation for it — the spelling is missing
there, not the reachability.

**`DiagKind::IndexTarget` is settled by the pin, and stays unwired — confirmed by measurement, and for a
stronger reason than the pin.** `tests/highlevel/checker.rs:738` is *named* for `IndexTarget` but pins a
different kind — `assert_eq!(diags[0].kind, DiagKind::Guard)` — with the expected side pinned as the **array
kind**. So the contract is *"a statically non-indexable read is a `Guard` whose expected side is the array
kind"*. Wiring would both rewrite the pin **and** state a **weaker** requirement (a union of three shapes
where the guard names one), changing every statically non-indexable read (`5[0]`, `(\x. x)[0]`, `"s"[0]`)
with no bug reported. Not taken; pin and wiring both left alone.

**What the measurement adds** (§6's table): the weaker requirement is not merely weaker, it is **false on
this path**. `(\x. x)[0]` gets a longer sentence that still names the fix; `t[0]` over a **tuple** gets
*"expected a tuple, array, or struct type, found TypeTuple"* — a sentence naming a tuple as acceptable while
a tuple is what this path refuses, which `docs/language-spec.md:665-666` settles the other way. So the pin
rests on more than three assertions, and the pin is **three** tests rather than the one `:738` names:
`checker.rs:738`, `checker.rs:766` and `crates/lichen-language/tests/language/pipeline.rs:2180`.

**The preprocessor's missing span is fixed, by the user's decision rather than by a reinterpretation.**
The offset was available and discarded one layer up: `parse::parse` returns `LocatedDirective`, and
`block_directives` dropped it with `.map(|(d, _)| d)`. The behaviour was documented as intended
(`lib.rs:26-27`, `:42-43`, and the `Invariant` at `:403-407`), so taking it meant **editing that
Invariant** — which the user authorised explicitly, rather than this slice reading the old text a
different way. **P2 now holds there.** `LocatedDirective` became a struct
(`crates/lichen-preprocess/src/parse.rs:51`) and `block_directives` returns it (`lib.rs:323`), so the
offset is in the type: there is no function left that hands out an unlocated directive, and a caller
that does not want the offset says `.directive` where it drops it. `stage_depends` rebases the
interior offset onto the file (`lib.rs:419`) and both its diagnostics now carry the span.

Measured on the smallest `@depend` source, before and after:

```
t =  # a project header
     ---
     probe_dep_span = depend "https://example.invalid/probe.git"
     ---
     42
before: dependency 'probe_dep_span' is not fetched (expected <cache>/probe_dep_span) — run `lichen fetch` first
        span:    None
after:  dependency 'probe_dep_span' is not fetched (expected <cache>/probe_dep_span) — run `lichen fetch` first
        span:    Some((3, 1))
```

One command, both readings:

```
cargo test -p lichen-preprocess --test directive_span -- --nocapture
```

**The message is byte-for-byte unchanged**; only the span moved. Rendered at the front end it now
carries the caret, on the directive rather than nowhere:

```
error: dependency 'probe_dep_span' is not fetched (…) — run `lichen fetch` first
  --> 1:4
 1 | ---probe_dep_span = depend "https://example.invalid/probe.git"---
   |    ^
```

**What did *not* change, and why it is not a leftover.** `PreprocessDiag.span` stays an
`Option<Span>`: `ImportResolver::resolve_import` is implemented by the caller and returns a
`PreprocessDiag` it built itself (`crates/lichen-language/src/package.rs:1005`), whose span may
genuinely be absent, and `preprocess` overwrites it with the import's own position
(`lib.rs:271`). That is a real boundary where a span is not yet known, not a forgotten one. Two
tests in `crates/lichen-language/tests/language/preprocess.rs` pinned the old contract
(`span.is_none()` and "renders without a line-1 caret"); the user's decision invalidates them, so
they now pin the directive's line instead.

**And one path correction:** there is no `crates/lichen-highlevel/src/render.rs`. The renderer is
`crates/lichen-language/src/render.rs`, and `CheckerDiag` is an alias for
`lichen_highlevel::diagnostic::Diag<P>` (`render.rs:3`) — which is why the `TableMiss` fix spans two crates.

## 8. A new offender, measured on the operator ladder (`23aa0f26`)

Writing greedy sampling exposed two refusals that fail **P1** — neither names the fix — and one of them names a
**cause it is not**, which is worse than silence:

* **A comparison cannot select inside a fold's step.** `if (read > s(1)) then (s(0) - 1) else s(2)` is refused
  with *"a kernel body reached a node with neither a value nor an operation, so there is nothing to emit for it
  (node=NodeId(1707v1))"* — a sentence about a `let` alias filled at run time, for a mistake that is about a
  `Select` inside a reduction step. It is **not** the `~` mark: the same body with the mark moved onto `max` and a
  pass-through slot compiles and runs. So a reader is told the wrong cause, and a node id is not something they can
  act on. The working spelling is arithmetic on the comparison's `0`/`1`, with a load-bearing `1 *` anchor — without
  it the same body is refused as *"instruction 15 mixed an integer and a float in one operation"*, which at least
  names a class.
* **A fold's base arm may return exactly one state slot, and the refusal is terse.** A whole-state or two-slot-tuple
  base arm is refused with *"…the domain is NodeId(135v1) … with leaves [NodeId(135v1)]"*. It does point at the
  domain — the domain is built from the base arm's value (`crates/lichen-lowlevel/src/loop_conversion.rs:131`) and
  exactly that one value is emitted (`crates/lichen-compute/src/compute/body.rs:1195`) — but it does not say the rule
  a reader has to satisfy, so the consequence (a carried maximum is unobservable afterwards) has to be inferred.

Both are recorded here rather than fixed: the first needs a diagnostic that distinguishes a step whose value has no
`Select` from one whose alias is filled later, and the second needs a sentence naming the one-slot rule. Neither is
a message change, so neither belongs to the slice that fixed the unclosed delimiter.

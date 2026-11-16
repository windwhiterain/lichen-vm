# Polymorphic-flag migration: recon measurements, census and site map

> Status: **recon complete for slice 0; implementation not started.** This note records what
> was *measured* on checkout `da8d77b6` (branch `feature/polymorphic-recon`, worktree
> `.worktrees/polymorphic-recon`), so the migration has baselines it did not have to derive
> and a site map it does not have to re-find. The rule, the rulings and the hazards are
> [the contract note](polymorphic-flag.md)'s; nothing here restates them.
>
> **One input is missing.** The scratch corpus that
> [nested-apply-none](nested-apply-none.md) §28 depends on
> (`C:\resource\lichen-vm\.scratch-core\`) **is gone**: `find /c/resource -maxdepth 5` for
> `m4.lichen`, `predicate-scan*`, `fingerprint-scan*` and `*condition-scan*` returns nothing,
> and `.scratch-core` exists in no worktree. Its manifest hash in that note's §28 is now
> a record of a loss, exactly as it warned. The nested-apply baseline below is therefore a
> **reconstruction** from the note's own program texts, identified as such per row.

## 0. How the measurements were taken

- **The front end is the runner** (AGENTS.md, TestKit): every answer below is
  `target/debug/lichen-compiler.exe <file>`, built from this worktree's sources with
  `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, `-j 4`.
- **The build needs the MSVC environment on this machine.** From Git Bash, `cargo build`
  fails twice before it gets anywhere: the shell's `/usr/bin/link` shadows MSVC's `link.exe`
  (`link: extra operand …`), and `LIB`/`INCLUDE` are unset, so the first C build script
  (`stacker`) fails on `kernel32.lib` and then on `cl.exe`. The working environment is
  MSVC `14.44.35207` plus Windows Kits `10.0.26100.0`, with `bin/Hostx64/x64` first on
  `PATH` and `LIB`/`INCLUDE` spelled out; it is kept in `.scratch-core/env.sh` (scratch).
  The build then reports `Finished dev profile … in 52.26s`, and every `Compiling`/path in
  it names `.worktrees\polymorphic-recon`, not the parent checkout.
- No code was changed. Scratch programs live under `.scratch-core/` in this worktree only.

## 1. Baseline answers

### 1.1 The five Ideal API forms (SPEC §Ideal API), verbatim

Each form was written exactly as the spec prints it, one file each, and run.

| form | program | answer |
| --- | --- | --- |
| 1 uniform signature | `sum3 := [_ -> (_: Int), 3]['a, 'b, 'c] -> 'a + 'b + 'c`<br>`sum3 (_[1, 2, 3])` | `6: Int` |
| 2 named signature | `div := {.'dividend: Int, .'dividor: Int} -> dividend / dividor`<br>`div _{.dividend 2, .dividor 1}` | `2: Int` |
| 3 struct signature | `Div := _ -> {.dividend _: Int, .dividor _: Int}`<br>`div := 'x = Div _ -> x.dividend / x.dividor`<br>`div _{.dividend 2, .dividor 1}` | `2: Int` |
| 4 function generic | `div := 'T -> {.'dividend: T, .'dividor: T} -> dividend / dividor`<br>`div _ _{.dividen 1, .dividor 2}, div _ _{.dividen 1.0,.dividor 2.0}` | **two errors**: `no field named dividen in the struct type Struct, did you mean one of 'dividend', 'dividor'?` at `2:18` and `2:51` |
| 5 nested | `A := _ -> {.'x: Int, .'y: Float}`<br>`B := _ -> {.a A _, .'z: String}`<br>`f := {.a A _, .b B _, .'w: Type} -> ...` | **parse error** at `3:37`: `expected '@assert', …, ':', '#', '!', '?', or '~', found '.'` — `...` is spec prose, not lichen |

Two spelling corrections were then measured, so the *forms* have baselines that run:

| corrected form | program | answer |
| --- | --- | --- |
| 4 | `div _ _{.dividend 1, .dividor 2}, div _ _{.dividend 1.0, .dividor 2.0}` | `0.5: Float` (the block's value is its last expression, so the `Int` call's answer is not printed) |
| 5 | `B := _ -> {.a A _, .'z: string}` and body `w` (`f _{.a A _, .b B _, .w Int}`) | `Int: Type` |

`String` is spelled **`string`** in this surface: the lexer maps `string` to `TokenKind::KwString`
(`crates/lichen-language-lex/src/lib.rs:352-353`, `:829`), and `String` is an ordinary
unresolved name. That is a spec-text defect, not a language one.

### 1.2 The nested-apply set, reconstructed

The corpus is gone (§0), so each program was rebuilt from the text
[nested-apply-none](nested-apply-none.md) quotes, **spelled with primed binders**: the
note writes `mk = x -> y -> x`, and that spelling no longer resolves (see §2 — an unprimed
name is a reference, so it answers `unresolved name 'x'` for every name). Where the note
quotes a body in a shorthand this front end refuses (`add n a` for a pair, `[e]` without a
length), the program below is the closest runnable spelling and the row says so.

| program | reconstructed text | answer here | that note's answer |
| --- | --- | --- | --- |
| `m4` (A) | `mk := 'x -> 'y -> x` ; `f := 'n -> (mk n) 1` ; `f 3` | `3: Int` | `none` |
| `v4` (A) | `mk := 'x -> 'y -> x` ; `f := 'n -> (mk n) n` ; `f 3` | `3: Int` | `none` |
| `v3` (A) | `mk := 'x -> 'y -> x + y` ; `f := 'n -> (mk n) 1` ; `f 3` | `4: Int` | `4` |
| `h1` (A) | `mk := 'x -> 'y -> x + y` ; `f := 'n -> (mk n) n` ; `f 3` | `6: Int` | `6` |
| `b6` (B) | `f := 'n -> { a := 1; add(n, a) }` ; `f 3` | `4: Int` | `none` |
| `b7` (B) | `f := 'n -> { a := n + 1; add(a, a) }` ; `f 3` | `8: Int` | `none` |
| `g3` (B) | `f := 'n -> { a := n + 1; b := add(a, a); b }` ; `f 3` | `8: Int` | `none` |
| `h2` (B) | `f := 'n -> { b := add(n, n); b }` ; `f 3` | `6: Int` | `6` |
| `i7` (C) | `f := 'n -> 'y -> add(n, y)` ; `g := f 3` ; `g 4` | `7: Int` | `none` (its quoted text, `f = n -> add n`, applies a pair-taking callee to one argument and is refused here) |
| `t1` | `f := 'n -> (mk 3) 1` ; `f 0` | `3: Int` | `3` |
| `z1` | `f := 'n -> (('x -> 'y -> x) n) 1` ; `f 3` | `3: Int` | `3` |
| `s1` | `f := 'n -> { c := mk n; c 1 }` ; `f 3` | `3: Int` | `3` |
| `c1` | `f := 'n -> { g := mk; (g n) 1 }` ; `f 3` | `3: Int` | `3` (local binding, §5 of that note) |
| `n1e` | `mk := 'x -> 'y -> x` ; `mk 3 1` | `3: Int` | `3` |
| `r4` | `f := 'n -> [ (mk n) 1 , 1 ]` ; `f 3` | `[?a]: Array` | `[3]` |
| `w2` | `f := 'n -> (mk n) 1` ; `7` | `7: Int` | `7` |
| `w4` | the same body in `f` and in `g := 'n -> (mk n) 2` ; `f 3` | `3: Int` | `none` |
| `w6` | `h := 't -> { f := 'n -> (mk n) 1; f 3 }` ; `h 0` | `3: Int` | `3` |
| `w7` | `f 3` then `f 4` | `4: Int` (the block's last expression) | `(none, none)` for their tuple print |
| `w8` | `h := 'k -> k 3` ; `h f` | `3: Int` | `none` |

**What this baseline says.** Every program the note records as answering `none` answers its
correct value here, which matches that note's own status (`fixed`, §27's repair landed): no
twenty-two-program set remains to protect as *failing*, and the migration's acceptance is
that these answers **do not move**. `r4` is not a counterexample: `[?a]` is how this front
end prints any array (`f := 'n -> [3, 1]; f 0` also answers `[?a]: Array`, and `(f 0)[0]`
answers `?a: ?a`), so the row is an instrument difference, not an answer difference.

### 1.3 The budget test and the three affected binaries

| command | the tool's own line |
| --- | --- |
| `cargo test -p lichen-lowlevel --test basic` (includes `recursion::deep_evaluating_an_infinite_stream_records_the_deep_budget ... ok`) | `test result: ok. 167 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.22s` |
| `cargo test -p lichen-highlevel --test highlevel` | `test result: ok. 136 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.12s` |
| `cargo test -p lichen-language --test language` | `test result: FAILED. 277 passed; 12 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.34s` |

**The language binary is red on this checkout before any edit of mine** (the worktree is a
clean checkout of `da8d77b6`; `git status` showed only the untracked `.scratch-core/`). The
twelve: `callee_inference::a_named_type_function_parameter_and_its_field_reads`,
`callee_inference::an_instantiation_whose_callee_is_inferred`,
`field_read_kinds::a_formal_field_read_is_checked_against_the_argument_at_the_call`,
`field_read_kinds::a_named_read_of_a_deferred_array_is_refused_at_the_read`,
`field_read_kinds::a_named_read_of_a_deferred_non_struct_is_refused`,
`field_read_kinds::a_named_read_of_a_deferred_struct_is_accepted`,
`frozen_function_type::a_function_template_in_a_frozen_module_is_a_function`,
`merge_carry::a_merge_carries_the_decided_type_to_a_member_added_after_the_commit`,
`pipeline::a_field_reads_class_is_decided_where_the_container_type_is`,
`pipeline::a_struct_instance_reads_its_fields_by_name`,
`template_instances::a_struct_types_wildcard_fields_stay_open_across_instances`,
`template_instances::an_inline_literal_domain_is_fresh_per_application`. Several carry a
formal-reference diagnostic in their captured output (`'n` / `?a` readings), which is the
same territory as §2 — whoever starts the migration should establish whether these twelve are
one cause before treating this binary as a gate.

## 2. The unprimed-lambda census (ruling 3)

### 2.1 The prime *is* the binder, in this surface

Ruling 3 says an unprimed lambda is monomorphic. **That cannot be observed today, because an
unprimed name is not a binding at all.** The parser makes `'x` an `Expr::Formal` and a bare
`x` an `Expr::Name` (`crates/lichen-language-parser/src/parse.rs:1109-1120`), and the lexer
admits `'` only adjacent to the name it precedes
(`crates/lichen-language-lex/src/lib.rs:453`, enforced at `:526` and `:654` by
`prime_precedes_name`). A `Formal` resolves to the parameter slot the lambda declared
(`crates/lichen-highlevel/src/checker.rs:1775-1785`); a `Name` is a reference, so with no
binding in scope it fails to resolve.

Measured, both spellings of the same lambda:

| program | answer |
| --- | --- |
| `Fill := {.n @any : Int}` ; `fill := (c = Fill) -> c.n` ; `fill ((Fill){.n 1})` | `error: unresolved name 'c'` at `2:10`, exit 1 |
| the same with `('c = Fill)` | `none: Int`, exit 0 |

So the census question "which binders lack the prime" has the answer **none can**: a
lambda parameter is either a `Formal` (primed), a placeholder `_` (never a binding — the
parser says so at `parse.rs:1103-1104`), or an expression that is not a binder at all.

### 2.2 Counts per tree

`.lichen` sources only; `arrows` is every `->` (this surface has no function-type arrow, so
each is a lambda arrow); `prime marks` is every `'`.

| tree | files | arrows | prime marks | `'name ->` | `'name = … ->` | `'name : Ann ->` | unprimed named binder |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `examples/` | 49 | 178 | 616 | 57 | 110 | 3 | **0** |
| `lichen-std/` | 1 | 10 | 10 | 10 | 0 | 0 | **0** |
| `lichen-std-native/` | 1 | 1 | 1 | 0 | 0 | 1 | **0** |
| `crates/lichen-compute/src/` | 2 | 46 | 46 | 16 | 27 | 3 | **0** |
| `crates/lichen-language/src/` (`core.lichen`) | 1 | 9 | 9 | 9 | 0 | 0 | **0** |
| `crates/*/tests/**` | see §2.3 | — | — | — | — | — | **1** |

The prelude is verified all-primed as the contract note states: `core.lichen`'s nine arrows
are `'t -> …` and eight `'operands -> …` (`crates/lichen-language/src/core.lichen:2-10`), no
`_` parameter, no unprimed name.

The category counts are how many binders are *spelled* each way, not a partition of the arrow
total — a `-o` count double-counts a line that matches two patterns. The claim that matters is
the exhaustive one, and it was measured directly: across `examples/`, `lichen-std/`,
`lichen-std-native/` and `crates/`, **seven lines carry a `->` with no prime on the line at
all** — five are prose inside a program's `output = "…"` doc string or a `doc`-style comment
(`examples/import/geometry.lichen:4`, `examples/import/math.lichen:3`,
`examples/let_polymorphism.lichen:3`, `examples/placeholder.lichen:3`,
`examples/matmul.lichen:18`), and two are §2.3's stale fixture. Every other lambda in the tree
writes its binder with a prime.

### 2.3 The one unprimed binder in the tree, and it is dead

`crates/lichen-compute/tests/lichen_compute/inplace_write_refusal.lichen:9` and `:15` write
`fill := (c = Fill) -> {` and `raw := (lane = Par) -> {`. Under §2.1 that is `unresolved name
'c'` — measured on the same shape. The file is **not run by any test**: the only reference is
a doc comment in `crates/lichen-compute/tests/lichen_compute/refusal_over_an_answer.rs:13`,
and that test carries its own inline copy of the same program spelled `('c = Fill)`
(`:19`, `:28`). So the file is a stale duplicate of a live fixture: it is the census's only
unprimed binder, and it is broken by the current surface rather than by anything the
migration would do. (Recorded, not fixed: this work is read-only.)

### 2.4 What ruling 3 means for the migration

The ruling's *semantics* — an unprimed lambda creates no `@polymorphic` cell and its body is
referenced in place — has **no syntax that can reach it** in this checkout: every lambda the
language can express is primed, so every lambda is polymorphic, and "monomorphic lambda" is a
surface the migration would have to add (a binder marker other than `'`) rather than a
behaviour to preserve. Two consequences to settle with the maintainer before slice 1 wires
the flag:

1. **The census the contract note asks for is empty**, so the migration's "unprimed lambdas
   elsewhere change semantics" cost is zero today. Nothing in `examples/`, `lichen-std/`,
   `lichen-std-native/` or `core.lichen` depends on the monomorphic reading.
2. **The sugar row is unexercised**: SPEC §Syntax says `...'a... -> ...'b...` desugars to
   `Function{@polymorphic a := _; …}`. Since the surface admits nothing else, the flag's
   `Some`-ness at creation is currently "always", and a migration that computes it from a
   marker would compute the constant `true` until the marker exists. Whether that is the
   intended landing shape is a decision, not a measurement.

## 3. Site map (file:line, as this checkout stands)

### 3.1 Node creation — where a primed binder's cells are made, and where the flag must be emitted

`crates/lichen-highlevel/src/checker/lambda.rs`, `check_lam` (entry `:21-28`):

- `:55` `let formals = self.formal_leaves(parameter);` — the binder leaves of the parameter
  expression, found by the walk at `:206-232` (a nested `Function` is another lambda's own;
  an already-declared `Formal` is not this lambda's leaf).
- `:64-82` per formal, the two cells the flag must mark: `fresh_cell()` twice (`:66-67`), the
  `[value, type]` slot expr (`:70`), the scope binding (`:71-77`), and
  `parameter_values.insert(function, value_cell)` (`:81`).
- `:142` the shell of the function is closed (`current_parameters.pop()`, `scopes.pop()`);
  `:147-148` the function's `r#return` and `parameter` are recorded; `:151` `return_type`.

This block is the only place a lambda's parameter cells come into being, so it is the site a
`@polymorphic` flag on "the function's parameter/body placeholder cells" (contract note §2,
last bullet) has to reach. `ExprKind::Formal` is resolved to its slot in
`crates/lichen-highlevel/src/checker.rs:1775-1790`; a `Formal` that is not yet bound is what
`formal_leaves` collects (`:218-224`).

### 3.2 The deep pass — what it walks today

`crates/lichen-lowlevel/src/evaluation.rs`, `evaluate_node_deep` (`:752`) →
`evaluate_node_deep_inner` (`:876-985`). Its descents:

| edge | line | note |
| --- | --- | --- |
| the node itself, run | `:931` `let value = self.evaluate_node(Dyn(node), current);` | runs an operation; does **not** deep-walk the operand |
| array items | `:933-945` | skips `item.shallow` (`:941`) — a lazy region by design |
| table keys and values | `:947-958` | both items, with the table's block as `current` |
| `External` operator's operand | `:960-976` | one level, only for `LowOperator::External` |
| wildcard template | `:926-930` | decided content, returns `None` without descending |

It writes `evaluated_deep` at `:979-981` and clears `assumed_concrete` at `:982`. The depth
budget is `:912-923` (`evaluate_depth_limit`, `BudgetExhausted::EvaluateDepth`) — the guard
the contract note's D18 hazard requires the flag's propagation to keep.

### 3.3 Every reader of the class-scope verdict

| site | what it reads |
| --- | --- |
| `crates/lichen-lowlevel/src/function.rs:83-90` | `ApplyCtx::scope_root` — the `# Invariant` superseded by the flag ("a class is cloned only when **every** member lies inside that scope") |
| `function.rs:92-95` | `ApplyCtx::scope_verdicts` — one entry per class judged, so a clone never splits a class |
| `function.rs:509-510`, `:1145-1146` | the two `ApplyCtx` constructions (`scope_root: self.function_scope(function)`, fresh `scope_verdicts`) |
| `function.rs:1239-1254` | `node_class_inside_scope` — the verdict, cached per class representative, with the `closure_scope` second chance |
| `function.rs:904` | the clone walk asks it once per node (`let class_inside = self.node_class_inside_scope(node, ctx);`) |
| `function.rs:634`, `:645` | the other two callers, on `return_value` and `template` |
| `crates/lichen-lowlevel/src/scope.rs:194-197` | `class_scope_summary` — the class's summary, held by the representative |
| `scope.rs:200-232` | `class_inside_scope` — the outermost-member test the superseded rule rests on |
| `scope.rs:233-261` | `refresh_class_summary` — the one writer, called from `set_node_owner` (`:263-272`) and `merge_class_summaries` (`:274-284`), i.e. **maintained at every write and every union** |
| `crates/lichen-highlevel/src/checker/lambda.rs:129-132` | `class_inside_scope` on the domain cell, for the wildcard-fill mark |
| `crates/lichen-lowlevel/src/template.rs:439` (`template_class_is_cloned`), `:450` (`template_class_verdict`), scope axis at `:455-462` | `class_inside_scope(representative, &scope)` against `TemplateCtx::scope` — a cell is fresh only when **every** member of its class lies inside the template's own scope |
| `template.rs:97-103`, `:157-165` | `TemplateCtx::scope` itself — "the scope the walk's decisions are made in" |
| `template.rs:611-650` (`template_static_inside_scope`) | the frozen half: `static_class_inside_scope` against the class's source scope, reached through a same-module `static_origin` |
| `crates/lichen-lowlevel/src/static_module/apply.rs:558+` (`static_node_apply`) | the frozen mirror: `:460-479`, `:578-588` re-home an open cell by the class's scope; `:832-836` builds the fresh closure scope; `:867` walks the closure's own scope |

`scope_summary` is a node field (`crates/lichen-lowlevel/src/lib.rs:965`) written by
`refresh_class_summary`; `mark_scope_root` (`module.rs:304-306`) is the block-level
`scope_root`, which is a different thing and must not be confused with `ApplyCtx::scope_root`.

### 3.4 The artifact node table — where a new node field is persisted

- Format number: `crates/lichen-language/src/persist.rs:38`
  `const ARTIFACT_FORMAT_VERSION: u32 = 32;`, written at `persist/container.rs:191` and
  refused at `:439` (`unknown artifact format version`).
- The freeze's node record: `crates/lichen-lowlevel/src/static_module/freeze.rs:126-162`.
  A node contributes `value`, `low_shape` (the class's, `:135`), `operation` (`:136-143`),
  `equality` (`:144-149`), `owner` (`:151-155`), `runned` (`:158`),
  `evaluated_deep` (`:159`), `wildcard_fill` (`:160`). `StaticNode`/`StaticFunction` live in
  `static_module.rs`; `StaticFunction` (`lib.rs:885+`) mirrors `parameter`, `r#return`,
  `return_type`, `return_attribute`, `asserts`, `nodes`, `parent`.
- A flag that must survive freeze and reload belongs beside `evaluated_deep` in `freeze.rs`
  and in `StaticNode`'s codec; the contract note already says the format bumps.

### 3.5 The prime surface, end to end

| layer | site |
| --- | --- |
| lexer | `RawToken::Prime` at `crates/lichen-language-lex/src/lib.rs:453` (`#[token("'")]`), mapped at `:906`; adjacency enforced at `:526` and `:654` |
| parser | `crates/lichen-language-parser/src/parse.rs:1109-1120` — `Prime.then(name())` → `Expr::Formal`, else `name()` → `Expr::Name`; `:1161-1170` the `.'name` field shorthand |
| IR | `ExprKind::Formal` at `crates/lichen-highlevel/src/ir.rs:390`; "the IR carries no name strings" (`:154`) |
| checker | `crates/lichen-highlevel/src/checker.rs:1775-1790` (a `Formal` resolves to its binding's slot); `checker/lambda.rs:206-232` (`formal_leaves`); `checker/lambda.rs:64-82` (the cells) |
| other `Formal` readers | `checker/annotations.rs:176`, `checker/structs.rs:716`, `checker/templates.rs:143` |

There is **no `@polymorphic` anywhere in the code**: `grep -rn polymorphic crates/*/src` finds
only prose about polymorphic *templates* in `lichen-compute`, `lichen-render` and
`function.rs:284`/`:352`. The Any-migration's template binder
([any-removal-migration](any-removal-migration.md)) is the surface the flag's encoding rides
on, and the code has not gained it yet.

## 4. The propagation gap: what the clone walk reads that the deep pass never marks

Ruling 1 wants "deep evaluate precedes clone; the flag is set on every node the walk reads",
guarded by a `debug_assert!`. The two walks do **not** have the same domain, so the assert as
literally stated would fire. This section is a **code reading** (§3.2 against §4.1), not a
measurement: no test prints `evaluated_deep` per visited node, and instrumentation was not
written because this slice is read-only.

### 4.1 What the clone walk reads

`node_apply` (`function.rs:846`), `value_apply` (`function.rs:1009`) and `clone_template`
(`template.rs:147`):

| edge | site | reaches |
| --- | --- | --- |
| `ctx.remap` hit | `function.rs:847-849` | already-cloned nodes |
| the node's own facts | `function.rs:852-862` | `value`, `operation`, `evaluated_deep`, `function`, `block`, `origin` |
| class-wide facts | `function.rs:873` (`class_low_shape`), `:875` (`template_root`), `:904` (`node_class_inside_scope` → `class_root`) | **class members and class representatives** |
| operation operand | `function.rs:938-943` `operand: operation.operand.map(|operand| self.node_apply(operand, ctx))` | the operand, recursively |
| value → array items | `function.rs:1022-1033` | every item, `shallow` included (`:1031` copies `..item` and clones the node regardless) |
| value → table key/value | `function.rs:1045-1060` | both |
| value → dynamic closure | `function.rs:1085-1135` | the closure's whole `nodes` scope (`scope.iter()`, the walk that follows `:1120`), its `r#return`, `parameter`, `return_type`, `asserts` |
| template closure map | `template.rs:202-230` (`template_closure_map`), children at `:298` | payload children, traced children, and each node's operation operand |

### 4.2 The nodes the deep pass does not reach

Each row is a place the clone walk reads a node whose `evaluated_deep` may be `None`.

1. **Class members.** `node_apply` asks class facts (representative, low shape, scope) but
   never walks a class's members; the deep pass has no class edge at all. Under the refactor
   this is precisely the new recursion-set edge (`SPEC §Recursion`: "members in its class"),
   and it is D18's hazard: the fix is to propagate a *boolean* over this edge, never to
   deep-evaluate along it.
2. **Operands of non-`External` operators.** The pass runs the node (`evaluation.rs:931`)
   but descends only into an `External` operand (`:960-976`); the clone walk descends into
   *every* operand (`function.rs:938-943`). The distinction is exact: `evaluate_node_body`
   *does* evaluate an operand (`:560-660` reads `self.evaluate_node(operand, …)` in every
   operator arm), so the operand may hold a value — but only `evaluate_node_deep_inner`
   writes `evaluated_deep` (`:980`), so an operand reached this way carries **no verdict**,
   and `node_apply`'s `proven_concrete` read (`function.rs:882`) sees `None` for it.
3. **Closure bodies.** `value_apply`'s dynamic-closure branch walks the whole `Function.nodes`
   of the closure; the deep pass descends into "items of the node's own value"
   (`:933-945`), and a `LowValue::Function` is not an array, so a closure's body nodes are
   never deep-evaluated by that edge. (The checker's per-function return pass,
   `checker.rs:557`, evaluates each function's return value — a different thing from marking
   every body node `evaluated_deep`.)
4. **Shallow array positions.** The pass skips them (`evaluation.rs:941`); the clone walk
   clones their nodes anyway (`function.rs:1028-1030`). This one is deliberate on both sides,
   so the flag must define what a shallow item's polymorphic flag is rather than assert.
5. **Nodes reached only through a scope-root read.** `is_scope_root_read`
   (`evaluation.rs:143-160`, used at `:892`) ends the scope and **bypasses the cached
   verdict**, so the read is not the pass's answer for that node.
6. **Struct identity items.** `value_apply` references them in place by design
   (`function.rs:1028`), so they are read but not walked; they need a verdict only if the
   flag's rule makes them polymorphic.
7. **`External`'s operand's own operands** — one level deep only (`:960-976`), while the
   walk recurses arbitrarily.

### 4.3 What this means for the assert

Ruling 1's guarantee ("the clone walk never meets an unset flag") holds only if *every* node
the walk reads is on a path the deep pass already marked. Rows 1, 2, 3 and 5 are paths where
it is not, so a bare `debug_assert!(flag.is_some())` at `node_apply`'s entry
(`function.rs:846-862`) would fire on ordinary programs.

There is a second, sharper consequence about the flag's *representation*. A two-valued
`bool` cannot distinguish "not polymorphic" from "the pass never visited this node", so
`debug_assert!(flag)` on a `bool` is a tautology and `debug_assert!(flag.is_none())` on an
`Option<bool>` is a claim about the pass's totality — which §4.2 says is false today. So
either the flag is tri-state (and an unvisited node is an assert failure, making the gap a
blocker), or the flag is two-valued and the ordering ruling is stated about the nodes that
*have* a verdict. The three shapes that would satisfy the ruling without weakening it:

- **assert at the decision, not at the entry**: assert only where the flag *decides* (the
  selection point that replaces `node_class_inside_scope` at `function.rs:904`), and let a
  node the pass never reached read a defined "not polymorphic" answer;
- **make creation the source and propagation the addition**: the flag is written when the
  node is created (`checker/lambda.rs:64-82` is the only site that creates a lambda's
  parameter cells), and the deep pass only ever *adds* it, so "unset" never has to mean
  "unvisited"; or
- **close the domain**: add the class-member and operand edges to the pass's flag
  propagation (boolean only, depth-bounded by `evaluate_depth_limit`) so that every node the
  walk can reach has a verdict before the walk starts.

Which one the migration takes is a decision for the maintainer; the measurement that matters
is that the gap is real and enumerable, and it is listed above with the walk path for each
row.

## 5. Open items this recon hands on

1. **The nested-apply corpus must be re-frozen** before the migration needs it as an
   acceptance set: §1.2's twenty rows are reconstructions, and a behaviour change should be
   measured against programs kept in the tree, not against `.scratch-core/` which a cleanup
   already took once.
2. **The twelve red language tests** (§1.3) — establish whether they are one cause (several
   print formal-reference diagnostics) before gating the migration on that binary.
3. **Ruling 3's surface** (§2.4): with no unprimed binder expressible, the flag's
   creation-time value is a constant until a monomorphic binder marker exists, and the spec's
   sugar row is unexercised. This needs the maintainer's decision, not more measurement.
4. **The `debug_assert`'s home and the flag's arity** (§4.3): entry or decision point; two-valued or tri-state; and the domain closed by class-member and operand propagation or not.
5. **The stale fixture** (§2.3): `inplace_write_refusal.lichen` no longer resolves; it is
   dead weight beside its live inline twin, and only its own tree can decide whether to
   delete it or prime it.

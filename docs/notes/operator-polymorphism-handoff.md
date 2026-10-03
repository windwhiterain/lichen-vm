# Handoff: operator polymorphism (the refinement branch)

Where this stands, what is measured, what is decided, and what a successor
should do first.  The design record is
[operator-polymorphism](operator-polymorphism.md); this note is the *state* of
it.

## 1. Where things are

- **The branch is closed.**  Every commit below is on **`dev`** (the mainline —
  this repository has no `main`); the worktree `.worktrees/operator-polymorphism`
  and the branch `feature/operator-polymorphism` were deleted once the agreed
  scope (Phases 0–2) plus the two extras that followed it (the set value and
  §8.1) were landed and synced.  Phase 3 is a **new workstream**: open a fresh
  worktree for it — the decisions it needs are already recorded (the set value it
  must write, the `@in` keyword it must implement, and the R2/R1 routing fork of
  §7 of the plan note).
- Commit trail (oldest first): `674d048` operand tie · `9a3983d` the class domain
  as a value + `InDomain` + the condition · `7e33938` kernel-boundary record +
  example declarations · `78fa33e` `@assert` frees `!` · `ec480fc` the
  attribute's shape · `e92a75f` the refinement attribute · `d910f49` the
  single-constraint-slot record · `27cfc27` static class naming · `debbdef` the
  diagnostic flavour · `f253724` an attribute naming a value · `d6ebe8f` the
  `if` desugar reverted · `5d0d213` the set value · `1c291a1` its documentation ·
  `c8cdd2a` the refinement's spelling · `bc0c4d4` §8.1's documentation.
- The scratch samples every measurement below was taken with lived in the closed
  worktree's `.scratch-poly/` (excluded through the repository's local
  `info/exclude`, never committed) and went with it.  §3's table **is** the
  record: each row is a whole program, so recreate the ones a successor needs —
  operators (`a_ints` … `f_both_sites`), refinement attribute (`g_refinement_ok`,
  `h_refinement_refused`), the doc label (`i_label`), the `if` measurements
  (`j_if_hetero`, `k_if_lazy`, `l_tuple_if`), the set measurements (`n_set` …
  `t_set_sig`), the refinement's spelling (`u_refinement_name`,
  `v_refinement_unnamed`), and the two rendering regressions (`w_perspective`,
  `aa_struct_doc`).

## 2. What is landed

`+ - * /` and the four order comparisons are **polymorphic over `Int` and
`Float`**, and the contract that confines them is a **refinement**.

- `check_binop` (`crates/lichen-highlevel/src/checker/operators.rs`) pins the
  operands to one class only when one of them has **stated** a class; when
  neither has, the two cells are made one class instead (the polymorphic case).
- The class domain is a **value**, and the language spells it: **`set{Int,
  Float}`** (`crates/lichen-highlevel/src/set.rs`, `crates/lichen-language-parser`
  for the form).  A set's *value* is its members — an ordinary array node, **no
  tag** — and its *type* is `set<T>`, a kinded type whose shape is the element
  type alone (`TypeSet`, kind-marker codec tag `10`).  The separate kind is a
  soundness requirement: an array-typed set passed into an `array<T, n>`
  parameter and indexed inside the callee would read its element 0, and a
  value-level tag is invisible there.  `s[i]` is refused by the ordinary
  container unify.  The old encoding's `default` element is **gone** — nothing
  read it (`class_set::default_class` had no caller) and a general set has no
  privileged member; a reader that must commit takes the **first** member, which
  for `set{Int, Float}` is `Int`, the operators' historical default.
- `TypeOperator::InDomain` (codec tag `17`) is the **structural** membership test
  (`==` cannot do it: `TypeOperator::Eq` goes through `ValueExt::value_eq`, which
  for an array is *handle* identity, so a class node out of another module would
  compare unequal).
- The **refinement attribute** (`crates/lichen-highlevel/src/refinement.rs`):
  `e : T ! p` carries **one predicate function** in the pair's tail; `combine`
  returns a fresh unbound cell (**no propagation**); `missing_value` is
  `Parameterized` (**not** a concrete `Void` — the reconciliation is a plain
  unify, and a concrete absent value could not be written and would conflict);
  `unify_slots` is that plain unify (over-strict on purpose).
- Enforcement is the **existing assert channel**, through the new
  `AttrExt::constraint`, so the checker still names no concrete attribute.  A
  **parameter** refinement takes the general desugar `x ! p => e` →
  `x => { x ! p; e }`, which is why it needs no IR field and gets per-application
  re-checking for free.
- The assert spelling: `@assert e` (the `@` keyword sigil), freeing `!` for the
  refinement; `AssertSpelling` + `Diag::refinement_domain` make a refinement
  failure read `does not satisfy {Int, Float}`.
- An attribute can **name** the value it attaches to (`AttrExt::label` → the bare
  **name**; `Doc` for a string doc; `render::value_label` spells a labelled value
  `?name`), so `f = (x => x) ? "fibo"; f` prints `?fibo: ?a -> ?a`.
- A refinement **spells itself**: `Refinement::render` is `! ` plus the
  predicate's name, found by [`attr::pair_label`](../../crates/lichen-highlevel/src/attr.rs)
  — the general
  "a nested pair's name" search, which needs no schema tail because it asks
  **every** attribute of the composed set whether it names each of the pair's
  tail slots (canonical order, first answer wins).  The hook therefore carries
  one more parameter: `AttrExt::render(module, slot, attrs)`, `attrs` being the
  composed registry.  Measured: `in_num = (v => v > 0) ? "in_num"; 5 : Int !
  in_num` → `5 ! in_num: Int`; an unlabelled predicate spells nothing.
- Fixed on the way, and useful beyond this feature: **a static (frozen) cell is
  now named by its equality class**, not by its ref
  (`Module::static_equality_representative` + the type printer).  The operator
  pin had been hiding it — a pinned class renders its committed value, so both
  members of a class printed `Int`.

## 3. Measured behaviour (the acceptance table)

`cargo run -q -p lichen-compiler -- <file-or-dir>`:

| program | `dev` before | now |
|---|---|---|
| `add = x => y => x + y; add 1 2` | `3: Int` | `3: Int` |
| `add 1.5 2.5` | **failed** | **`4.0: Float`** |
| `add 1 1.5` | `expected Int, found Float` at `2:7` | **identical** |
| `add "a" "b"` | `expected Int, found string` | `does not satisfy {Int, Float}` at the operator |
| `(add, add 1.5 2.5)` | — | `<?a -> ?a -> ?a, Float>` |
| `f = x : Int ! (v => v > 3) => x; f 5` | — | `5: Int` |
| the same `f 2` | — | refused, `assertion failed: expected 1, found 0` at the annotation |
| `(10, "ten")(1 == 1)` | — | `"ten": string` (the opt-in dependent read) |
| `if (1 == 1) then 7 else 1 / 0` | — | `7: Int` (the untaken arm is not evaluated) |
| `f = (x => x) ? "fibo"; f` | — | `?fibo: ?a -> ?a` |
| `s = set{1, 2}; s` | — | `set{1, 2}: set<Int>` |
| `num = set{Int, Float}; num` | — | `set{Int, Float}: set<Type>` (the domain value) |
| `s = set{1, 2}; s[0]` | — | refused: `expected array<?a, ?b>, found set<Int>` |
| `s = set{1, "a"}` | — | refused: `expected Int, found string` (a set is homogeneous) |
| `s = set{}` | — | `set{}: set<?a>` |
| `a = set{1, 2}; b = set{1, 2}; (a == b, a == a)` | — | `(0, 1)` — membership is handle identity, the array rule |
| `f = x => set{1, 2}; f` | — | `?a -> set<Int>` (the type prints; it has no source spelling) |
| `in_num = (v => v > 0) ? "in_num"; 5 : Int ! in_num` | — | `5 ! in_num: Int` (the refinement's spelling) |
| the same with the predicate unlabelled | — | `5: Int` (nothing invented) |
| `always = (v => 1) ? "always"; (x => x) : _ ! always` | — | `Function ! always: ?a -> ?a` |
| `5 ? {name = "five"}` | `5 ? name = "five": Int` | identical (a struct doc still describes) |
| `5 # 4` | `5 # 4: Int` | identical (the render hook's new parameter changed nothing) |

Two mechanism facts that came out of measuring, and both are load-bearing:

- **The apply clone preserves equality classes.**  Tying the operands in the
  *template* is therefore what makes one application's arguments share a class,
  which is why `add 1 1.5` is still refused with today's exact text — no extra
  condition is needed for "the operands are one class".
- **An unknown class is not an error but an undecided value.**
  `TypeOperator::run` answers `Parameterized` for a pair it cannot compute, so
  before the refinement `add "a" "b"` *ran* and yielded an unresolved value.
  The assert is the only channel that turns "undecided" into "refused".

## 4. Rejected, with the measurement (do not retry these)

- **The set used as a *type*** (a kind marker in *type* position, unifying a
  member at every use — not to be confused with the landed `set<T>`, which is the
  type a set *value* has).  A set in type position must commit a member, and that
  unify is the lowlevel's, so the rule has to be a `Program` hook consulted from
  `unify_inner` — which, being a merge, folds a per-occurrence domain node into
  the member's *canonical* class, and `class_committed_value` then scans past the
  set.  Six test targets fail, order-dependently.
- **Desugaring `if` to the dependent read** `(e, t)(c)`.  The branch unification
  an `if` performs is what the type system depends on: the array desugar's shared
  element cell is what a class question reads, and with a *dynamic* condition the
  dependent form's type is never decided.  Measured inside a kernel,
  `x => if x <= 3 then 10 else 20` rendered `10: ?a` where `if` renders
  `10: Int`, caught by `jit_conditional_then`/`jit_conditional_else`.  `if` stays
  the unifying conditional; the dependent read is **opt-in** and needs no work.
- **Branch-pending deferral.**  Not needed: an arm is an apply, `check_app` does
  no check-time argument unify, and a template's body is never evaluated at
  definition, so an unselected arm's constraints never fire.

## 5. Known reds, and whose they are

- `lichen-language`'s `compute`: `a_kernel_value_and_type_render_by_name` — `.sig`
  renders `?c -> ?c` with a `none` artifact.
- `lichen-language`'s `runtime_only_package` — `launch`'s signature gate reads
  the domain lazily out of an open `.sig` and cannot resolve it.
  Both are **the same missing piece**: a kernel is currently lowered from a
  *template*, and the specialize-before-JIT pass
  ([kernel-class-crossing-fixes](kernel-class-crossing-fixes.md) §6) is what
  fixes it — another workstream, explicitly not this one's.  **The interface is
  one-way and clean: the class domain this branch puts in the graph as a value is
  what types that pass's placeholder** ("applied to a placeholder typed by the
  annotated domain").  Do **not** paper over it by writing a defaulted class into
  the parameter cell at `jit`: that is both the other workstream's job and
  unsound, because the cell is shared while the kernel is not
  (`f = y => y + y; k = compute.jit f; f 1.5` must keep working).
- `lichen-compute-gpu`'s tests do not compile on `dev` (pre-existing, and
  `kernel-class-crossing-fixes` §7 says not to repair them here).  Run the
  workspace with `--exclude lichen-compute-gpu`.

## 6. What to do next, in order

1. **Phase 3**: move `Num` and the operator bindings into `lichen-std`, routing
   R3 → R2/R1 (§7 of the plan note).  The refinement and the domain value are
   routing-agnostic, and the domain's surface form is landed: `Num = set{Int,
   Float}` is an ordinary binding.  Two things Phase 3 still needs, and both are
   decisions, not work:
   - **A membership spelling for the predicate.**  Decided: the `@`-keyword
     **`@in`** (`in_num = v => type_of v @in Num`), joining `@loop` and `@assert`
     — not a new binary operator, and not a `$` native (that sigil is
     plugin-private, so it cannot carry a user-writable std).  `@in`'s arity and
     level are settled with the implementation.
   - **How std reaches a program at all** (R2's intrinsic registry vs R1's
     implicit prelude import).  This is the real fork: R1 is the principled end
     state and needs the language's first implicit import.
2. **A refinement in a *type* string is not reachable, and this is understood,
   not pending.**  `f = x : _ ! in_num => x` prints `Int -> Int`: a refinement is
   the attribute of the parameter *expression* inside the lambda, and a function
   type is just `[domain, codomain]`, so `?a ! in_num -> ?a` cannot be built from
   the type.  Printing it needs the *lambda's* own rendering (the parameter's
   attribute lives in the template), which is a separate piece of work.  The
   refinement **is** shown wherever the annotated *value* is printed (§3's
   `Function ! always: ?a -> ?a`), which is the operator's own end state.
3. Then §8.2 (`Num`'s home), §8.3 (the panic arm's spelling), §8.6
   (generalising the single constraint slot to a per-marker set).
4. **Set follow-ups, none of them needed by this feature** (recorded so a
   successor does not read them as bugs): the set *type* has no source spelling
   (`f = x => set{1, 2}` prints `?a -> set<Int>`, but `x : set<Int>` does not
   parse — a `set<T>` type form is the follow-up if a parameter ever needs one);
   a set has no membership, no dedup, no order-insensitivity and no content
   equality (`==` is the array rule, *handle* identity: two `set{1, 2}`s compare
   `0`, one binding to itself compares `1`); and the members are homogeneous,
   like an array literal.
5. **The editor grammar is stale, and this is not new.**  `tree-sitter-lichen/
   grammar.js` still spells the assert as the prefix `!` (this branch moved it to
   `@assert`) and has no `set{…}`; nothing in the Rust workspace compiles it, and
   no generated `parser.c` is committed.  Whoever owns the editor grammar should
   take both at once.

## 7. Traps a successor will hit

- **An apply node *is* its return `[value, type]` pair.**  A condition registered
  on the apply node hands the assert channel an *array*, which is neither `1` nor
  lazy, so it reports a failure even while the applied value is still open (and
  twice).  A generated condition must name the pair's **element 0**
  (`Ctx::op_node(Index)` over the pair, the same read `Checker::value_of` builds).
- **`ValueExt::value_eq` compares array *handles*.**  Anything that must ask "are
  these the same type/class" has to decode structurally
  (`shape::low_type_of_slot`) — never `==`.
- **A value's encoding must not look like a type.**  The class domain used to
  need three elements for that reason (the tag is gone now, and the *set's* value
  is an ordinary array, so nothing new can be misread); `is_struct_marker_any`
  still accepts any two-element array, which is why a kinded type's shape is
  wrapped in an array rather than placed bare in the shape slot.
- **`set` is a keyword, so it is reserved.**  A program that bound a variable
  named `set` no longer parses; that is the one breaking change the set form
  introduced (the `!` → `@assert` move was the other, earlier one).
- **A set's type is `[shape, [TypeSet, K]]` with a 1-element shape.**  A parser
  form for the *type* does not exist, so `set<Int>` in source is a parse error
  ("expected `{`") even though the printer produces exactly that spelling.
- **The domain's spelling is the caller's, not the printer's.**  A set's value
  carries no tag, so the type printer cannot tell a class domain from any array
  of type values; `lichen-language/src/render.rs`'s `class_domain` is the one
  place that formats `{Int, Float}`, because only the refinement's registration
  knows it holds a domain.
- **`path.rs`'s child indices are reserved role slots** and the attribute order
  is a compatibility contract: a new attribute is **appended** (the refinement
  took slot 4; `Perspective` stays 2 and `Doc` stays 3), which the
  `the_canonical_order_is_the_persisted_pair_layout` test pins.
- Two test expectations were updated because they pinned the *pinned* signature:
  the two `examples/import/*.lichen` `output =` declarations (via the repo's own
  `sync-readme`, which mirrors them into `README.md`) and
  `imported_field_access_hovers_with_value_and_type` (asserted per field now).
- `cargo fix --allow-dirty` + `cargo fmt` before committing.  (In the closed
  worktree the `.git` file was a pointer — never append to it; a fresh worktree
  has the same shape.)

## 8. Verification, in commands

```bash
# Compilation.
cargo check --workspace --exclude lichen-compute-gpu

# Everything except the pre-existing gpu breakage: expect exactly the two
# §5 kernel targets red, and nothing else.
cargo test --workspace --exclude lichen-compute-gpu --no-fail-fast

# The acceptance table of §3.  The samples went with the worktree, so first
# write each row's program into its own file (one program per file — that is
# where the `n: T` line comes from) and run the directory:
cargo run -q -p lichen-compiler -- <the directory>
```

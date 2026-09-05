# Plan: make the doc attribute a plain, generic struct-typed value

> Status: **implemented** in the isolated worktree (`doc-attr-rewrite`,
> `C:\resource\lichen-vm-docrewrite`) off clean `v1`; merged to `v1`.
> Target: `crates/lichen-language/{lex,parse,ast,compile,checker,render,run,program,doc}.rs`,
> `crates/lichen-language/tests/doc.rs`, `crates/lichen-language-server/src/analysis.rs`,
> `crates/lichen-highlevel/src/attr.rs`, `docs/notes/attributes.md`, `docs/language-spec.md`.

## Decided design (confirmed with user)

- **Syntax stays `expr [# expr] [? expr]`.** `#` fills the perspective (constraint) slot,
  `?` fills the doc (label) slot.
- **Drop the builtin keyword/syntax** `? doc{…}`: no `KwDoc`, no `Question`-as-doc-literal
  desugaring, no `doc_literal`, no prelude, no canonical shared `Doc` node. `doc` is no
  longer a keyword; the user **defines/imports `Doc = struct<.name string, .description string>`
  themselves** and constructs an instance by hand (a plain struct value).
- **Both `#` and `?` take a general expression.** `? e` stores `e`'s value node in the doc
  slot, exactly as `# e` stores a value in the perspective slot. The `?` operand is already a
  general expression in the parser — only the `? doc{…}` sugar goes away.
- **`Doc` attribute marker stays, but the slot is fully generic**: the value is just the
  `?` expression's value (a plain struct instance), no doc-specific shape the checker
  understands beyond "it's a label."
- **Force all fields is automatic**: `Doc` is a real 2-field struct, so a `Doc{ name = … }`
  (missing `description`) is an ordinary struct-instantiation arity error. No extra doc
  check.
- **Renderer must see the full value type chain** to render the doc correctly (and more
  correctly, everything): the doc value is a struct instance whose *type* kind marker carries
  the `.name`/`.description` names, so `? name = …, description = …` comes from reading the
  type chain, not hardcoding.
- **Doc is still a label** (metadata, never a constraint): `is_label() == true`, no
  combine/unify conflict, `? b` overrides an earlier doc `a`, and it passes through on use.

## Why this is different from today

Today `? doc{…}` lowers to `Expr::RecordBlock` → `alloc_record` → `check_record`, which builds
a **fresh anonymous nominal struct every occurrence** and stores the field names only in the
*type's* kind marker. The `Doc::render` hook gets only the *value* node, so it renders
positionally (`? doc{ "five", "an int" }`) and has no access to the names.

The rework removes the `? doc{…}` literal, makes `?` accept any expression, and changes
rendering so the label render reads the value's **type chain** for names. The value is now a
user-made struct instance (`Doc(…)` / `Doc(.name …, .description …)`), a single nominal type,
arity-checked by the struct itself.

## Open sub-decisions (this span, and how they were resolved)

1. **How the label render gets the type chain.** `AttrExt::render(module, slot)` only receives
   the *value* node. To render field names from the struct type, the render needs the slot
   value's *type* node.
   - **RESOLVED (option b):** per the user — "attributes should be an array of exprs, so each
     slot should be a `[value, type]` or referencing to it." The runtime *render* slot for a
     label carries the annotation expression's `[value, type]` term pair (`self.term[pe]`).
     The constraint (perspective) slot keeps its bare lifted value because the lattice
     machinery reads `attr[e]`, never the render slot.
2. **Where the whole-type-chain render lives.**
   - **RESOLVED:** a dedicated `render_struct_fields_named(module, value_node, ty_node)` in
     `render.rs` reads the struct type's name table and renders `name = value, …`; `Doc::render`
     opens the `[value, type]` slot and delegates to it.

## Concrete changes (this span)

1. **`crates/lichen-language/src/lex.rs`** — removed `KwDoc` token, its `describe` arm, its
   logos `#[token("doc")]`, and its `raw_to_kind` arm. `Question` (`?`) stays.
2. **`crates/lichen-language-server/src/analysis.rs`** — removed `KwDoc` from
   `classify_token_kind`.
3. **`crates/lichen-language/src/parse.rs`** — deleted `doc_literal` (and its atom usage);
   deleted the now-dead `to_record_block`; updated doc comments to `? e`.
4. **`crates/lichen-language/src/ast.rs`** — `Expr::Annotation` `doc: Option<Box<Expr>>` is now
   a general expression (no doc literal); updated comment.
5. **`crates/lichen-language/src/compile.rs`** — annotation arm already generic; only a comment
   update. Schema-transplant fix stays.
6. **`crates/lichen-language/src/program.rs`** — unchanged (`Doc(Doc)` in the manifest,
   `&Doc` dispatch stay).
7. **`crates/lichen-language/src/doc.rs`** — `Doc::render` now opens the `[value, type]`
   slot and renders via `render_struct_fields_named` (the doc value's struct type chain); the
   label semantics (`slot()==3`, `is_label`, `combine`→no-doc, relaxed `unify_slots`,
   `is_subtype`→true) stay.
8. **`crates/lichen-highlevel/src/checker.rs`** — `check_ann` label branch: the runtime render
   slot for a label is the annotation value expression's `[value, type]` term pair
   (`self.term[pe]`); a constraint keeps its bare lifted value. `attr[e]` (constraint) is
   still the bare value.
9. **`crates/lichen-language/src/render.rs`** — added `render_struct_fields_named` (renders a
   struct-instance value's named fields from its type chain); updated `AttrExt::render` doc in
   `attr.rs`.
10. **`crates/lichen-language/tests/doc.rs`** — rewritten to the model: the user defines
    `Doc = struct<.name string, .description string>` and annotates `? Doc(.name …, .description
    …)`. Output is `? name = "five", description = "an int"`. A one-field doc is a struct arity
    error. All doc tests pass (10).
11. **Docs** — `attributes.md` (Syntax now `expr [: expr] [# expr] [? expr]`, a Labels
    section, updated non-goals/decision log), `language-spec.md` (grammar `annotated`, keywords,
    precedence), `attr.rs` (`render` doc).

## Verification

- `cargo test -p lichen-language --test doc` — 10 passed.
- `cargo test -p lichen-language` — doc (10), perspective (15), parse, render, compute, unit
  suites all pass (102 + 1 + 12 + 1 + 9 + …).
- `cargo build --workspace` — clean, no warnings.
- `cargo test -p lichen-language --test perspective` — 15 passed (constraint path intact).
- Manual: `Doc = struct<.name string, .description string>; 5 ? Doc(.name "five", .description
  "an int")` renders `5 ? name = "five", description = "an int": Int`; a `Doc` with one field
  is an arity error.

## Workspace strategy

Implemented in an isolated worktree off clean `v1` (`C:\resource\lichen-vm-docrewrite`, branch
`doc-attr-rewrite`), then merged to `v1`. The main worktree carries unrelated uncommitted WIP in
`render.rs`, `static_module.rs`, `analysis.rs` (the user's hover work), which is committed on
`v1` and left untouched.

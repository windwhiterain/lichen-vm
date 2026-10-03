# `core`: the built-in prelude

> Status: **current** — the module is embedded in the language crate
> (`crates/lichen-language/src/core.lichen`), registered as a built-in virtual
> package ([`PackageStore::register_core`](../../crates/lichen-language/src/package.rs)),
> and seeded into every source the preprocessor sees
> ([`crate::preprocess::preprocess`](../../crates/lichen-language/src/preprocess/mod.rs),
> `PackageStore::prelude_import`).  Its source is a **file** the store
> materializes under the cache root (§4), which is what makes a failure inside it
> attributable and a definition jumpable.
> The contract it carries is [operator-polymorphism](operator-polymorphism.md) §3;
> the migration it completes is §9 Phase 3.
> Points at: `crates/lichen-language/src/core.lichen`, `package.rs`
> (`register_core` / `core_terms` / `core_direct` / `prelude_import`),
> `crates/lichen-language/src/preprocess/mod.rs`, `resolve.rs`
> (`seed_imports`), `persist.rs` (`virtual_file_id`).

## 1. The module

```lichen
Num = set{Int, Float}
in_num = t => t @in Num
add = x => y => { x : (_ ! in_num); y : (_ ! in_num); x + y }
```

Ten bindings: the class domain `Num`, the predicate `in_num` that consults it,
and one per polymorphic operator (`add`, `sub`, `mul`, `div`, `less`, `greater`,
`less_or_equal`, `greater_or_equal`).  Every one is an ordinary lichen
definition — no compiler support, no native call — and the **contract** is the
class refinement `x : (_ ! in_num)`, whose predicate is applied to the operand's
*type* (`in_num = t => t @in Num`), so no type read is involved
([operator-polymorphism](operator-polymorphism.md) §3 records the mechanism, the
`! in_num` value form it replaced, and the `raw[?a, ?b]` signature an open class
prints).

The unit is a **package**: its bindings are the fields of its exported record, so
a program that wants the module value can also write `core = import "core"` and
reach `core.add` — the same handle serves both, which is what keeps the explicit
import and the implicit prelude one code path (`PackageHandle::direct` is what
the prelude adds, not a second module).

## 2. The three decisions

- **It is a prelude, not a library.**  The names are in scope with **no import**:
  every source a host compiles is seeded with the module
  (`crates/lichen-language/src/preprocess/mod.rs`), so `add 1.5 2.5` is `4.0`
  with an empty file.  That is the point of the phase: an operator's contract is
  a fact about the *language*, not something each program imports.
- **It is shadowable, not reserved.**  The prelude import is seeded **first**, so
  a program's own binding or import of the same name is resolved later and wins:
  `add = x => y => 99; add 1 2` is `99`.  This is the opposite choice from `set`
  and `@in` (reserved words), and deliberately so — the prelude is *library*, and
  a program that wants different arithmetic must be able to say so.
- **It is not a package the store counts.**  `PackageStore::compiled` observes the
  packages a *program* asked for (an explicit `import`, a `depend`), which is what
  the cache and identity tests measure; the prelude is the language compiling
  itself and every store pays it exactly once, so `register_core` does not
  increment the counter.  (It is also never published to the device — see §4.)

## 3. The mechanism

- **`direct` is the seeding hook, and this is its first user.**
  `ResolvedImport::direct` — a package's `(name, export)` bindings, bound as
  base-scope names alongside the import's own name — was declared for the compute
  package's `jit`/`launch`/`Kernel` and **never populated** until now.  The
  prelude's `direct` list is one entry per top-level binding.
  A record's *value* holds its fields' **values** (their types live in the
  struct's kind), so a field's `[value, type]` pair is not recoverable from the
  frozen struct: `core_terms` reads each binding's pair from the **checked
  build** (`ir.stmt_roots` → `state[..].term`) and `core_direct` maps it through
  the freeze.  A name/binding count mismatch is an error, not a truncation, so an
  edit to `core.lichen` cannot silently lose a name.
- **The built-in modules are exempt from the prelude.**  `core` cannot be its own
  prelude (the load would re-enter itself) and `compute.lichen` is a plugin's
  private source compiled against that plugin's own native registry — it spells
  the read it needs and must not depend on a prelude.  Both are recognised by
  their virtual path (`is_builtin_source`).
- **An embedded dependency contributes the all-zero sentinel.**  The prelude is
  never published to the device store (like `compute`), and
  `DeviceRegistry::artifact_identity` already folds an unpublished dependency as
  zeros *on both sides*, so a dependent's cache still hits
  (`crates/lichen-registry/src/device.rs`).  A package's artifact therefore
  records the prelude among its dependencies, and re-keys when the prelude's
  source changes — which is the correct invalidation for a prelude.
- **A registered built-in is reused, never re-frozen.**  A host may build a
  `PackageStore` **per run** over **one shared registry** — the editor's worker
  does exactly that, one store per analysis so its `packages` map is that
  document's import closure (`server.rs`) — so the *second* store would compile
  the prelude again and freeze it under the *same* device key, which
  `Registry::freeze_mapped` refuses ("the same content must not be compiled
  twice").  `registered_builtin` therefore checks the shared registry first and
  adopts the module already filed under that key; what a handle needs is in the
  package meta (`HighPackageMeta::export` plus `direct`, which is why `direct`
  was added there).  Before the prelude, no built-in was compiled by *every*
  store, so the collision was latent (`compute` had it too — it now takes the
  same reuse path).

## 4. The built-in's own file

A built-in is a **file**: the store materializes its source under the cache root
(`<cache>/builtin/core.lichen`, `…/compute.lichen`) and keeps a **source record**
in the package meta — the path, the text, and the position of every frozen node
(`HighPackageMeta::source`, built by `located_nodes` + `builtin_source` from the
build's `node_edges` and the frontend's `span_index`).  Two things follow, and
both are the reason the record exists:

- **A failure inside the module is attributed to the line that wrote it.**  A
  failed assert whose condition was cloned out of a static module names it by a
  `StaticNodeId` (`AssertError::template`); `Build::diagnostics` now emits that
  static ref instead of dropping the failure, and the package layer resolves it
  through the record to a position in *that* file.  Measured — `add ["a", "b"]`
  (the element outside the class):

  ```
  error: assertion failed: expected 1, found 0
    --> ~/.lichen/compilers/<key>/builtin/core.lichen:3:39
     |
   3 | add = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
     |                                       ^
  ```

  The wording is the assert channel's own rather than `does not satisfy {Int,
  Float}` — §5's domain spelling.  A module with **no** kept source — an ordinary
  imported package, whose own build already reported the failure when it
  compiled — drops as before, so
  `a_failed_assert_in_an_imported_package_still_reports_a_diagnostic` keeps its
  meaning.  An in-memory store has no root to materialize under: the file is
  named by its own path (`core.lichen`) and still carries positions.
- **A jump lands in it.**  The record is what an editor needs to point a
  definition at the built-in's line rather than at nothing.  **Landed** in the
  language server: the prelude import's names are seeded into the base scope as
  definitions whose `file` is this record and whose position is the name's own
  binding in it, so a use of `add` resolves to `core.lichen` and
  `textDocument/definition` answers with `Url::from_file_path` and the position
  in *that* file — which the client can open, because the file exists on disk.
  A failure inside the built-in is published the same way, as its own
  `publishDiagnostics` for that file rather than as a diagnostic on the document
  (`crates/lichen-language-server/src/analysis.rs`, `src/server.rs`).
  Two things that made it work are worth keeping in mind: a built-in definition
  must not enter any table keyed by a *document* span (the prelude import's own
  span is the synthetic `(1, 1)`, and its bindings' positions are `core.lichen`'s
  — either would collide with a document position), and the position is read from
  the record's **text**, not from its frozen-node `spans`: those cover the nodes a
  *failure* can name, which is not every binding's export node.

## 5. What is open, and how far "done" is verified

- **The built-in's names are not *hovered* with a type.**  A jump and a completion
  item for a prelude name name the built-in's file; the hover shows the definition
  line in it (`defined at line 3 of core.lichen`).  It does not render
  `value : type`, because that snapshot is the *built-in's* build — the record
  keeps positions and text, not checker facts — and the compiler is not this
  layer's to re-run.  Read from the record if it ever needs to be shown.
- **The domain spelling is not carried across modules.**  A refusal *inside* the
  prelude reads `assertion failed: expected 1, found 0` where the same assertion
  written in the document reads `does not satisfy {Int, Float}`: the spelling
  (`AssertSpelling::Refinement { domain }`) and the domain node live in the
  *built-in's* build, and the record keeps positions, not checker facts.
  Carrying them (the spelling table plus the domain's node, printed through the
  module the failure names) is the next step on this leg.
- **The call site is not attached yet.**  The diagnostic names the built-in's
  line; it does not also name the application in the document that failed the
  contract, because an assert's clone records its *template* and not the apply
  that made it (`AssertError` has no origin).  Recording that origin — the apply
  node, at the two clone sites (`static_module/apply.rs`, `function.rs`) — and
  resolving it through `Build::apply_edges` is what would add the "related"
  location.  (`Diag::related` is plumbed for it and nothing fills it.)
- **What "jump" and "publish" are verified against.**  `Doc`'s unit tests hold the
  in-memory case: a use of a prelude name resolves to a definition whose file
  records `core.lichen` and whose position is the binding's own line in it, and a
  refusal inside the built-in (`add ["a", "b"]`) lands in the document's *file*
  diagnostics, not its own.  The integration test
  (`tests/lsp_incremental.rs`) runs the real binary, so the file path the store
  materializes under the cache root is the one `Url::from_file_path` names and the
  failure is published as a second `publishDiagnostics` for it.  Not verified: a
  client actually opening that URI (the server never opens documents itself).
- **The built-in is compiled by every store and not device-cached.**  It is a
  virtual module (`persist::virtual_file_id`), compiled fresh in memory like
  `compute`; its values are ordinary (functions and a set of type values), so
  caching it is *possible* and simply not done.
- **The surface operators do not route here yet.**  `core`'s bindings are the
  contract, and the *checker's* builtin still implements the surface operators
  (routing R3, [operator-polymorphism](operator-polymorphism.md) §7).  Routing them
  onto these bindings has been implemented and withdrawn twice: first because an
  apply of a *static* function did not instantiate its signature per call — a root
  since **fixed** (`6e9c409`; a static apply's residual clones now belong to the
  caller's template) — and second because the routed form is a *call*, and the
  binding's operand-group element is the placeholder's `[class, kind]` pair, so an
  arithmetic result's type surfaces that pair (`(43, 44): <raw[Int, Type],
  raw[Int, Type]>`) and a statement's concrete-value snapshot disappears.  The
  recommended form is therefore an *expansion of the binding's body* at the call
  site, which keeps the builtin operator in the caller's body — no representation
  cost, no lost folding, and nothing for the kernel path to learn
  ([operator-polymorphism](operator-polymorphism.md) §7.1).
- **A kernel body cannot call into a built-in.**  A routed operator *as a call* is
  an apply of a static function and the kernel path refuses it
  (`compute.jit: static refs are not kernel-compilable v1`), so the call form would
  leave `examples/compute_jit.lichen` (whose `y + y` is that shape) red until the
  kernel workstream's specialize-before-JIT pass folds the apply back to a machine
  leaf.  An expanded body has no such cost: it *is* the machine-leaf shape.

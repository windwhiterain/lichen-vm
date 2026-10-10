# `core`: the built-in prelude

> Status: **current** — the module is embedded in the language crate
> (`crates/lichen-language/src/core.lichen`), registered as a built-in virtual
> package ([`PackageStore::register_core`](../../crates/lichen-language/src/package.rs)),
> and seeded into every source the preprocessor sees
> ([`crate::preprocess::preprocess`](../../crates/lichen-language/src/preprocess/mod.rs),
> `PackageStore::prelude_import`). Its source is a **file** the store
> materializes under the cache root (§4), which is what makes a failure inside it
> attributable and a definition jumpable.
> The contract it carries is [operator-polymorphism](operator-polymorphism.md) §2–§5.
> Points at: `crates/lichen-language/src/core.lichen`, `package.rs`
> (`register_core` / `core_terms` / `core_direct` / `prelude_import`),
> `crates/lichen-language/src/preprocess/mod.rs`, `resolve.rs`
> (`seed_imports`), `persist.rs` (`virtual_file_id`).

## 1. The module

```lichen
Num = set{Int, Float}
in_num = t => t @in Num
add = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
-- …and one binding per remaining polymorphic operator
```

Ten bindings: the class domain `Num`, the predicate `in_num` that consults it,
and one per polymorphic operator (`add`, `sub`, `mul`, `div`, `less`, `greater`,
`less_or_equal`, `greater_or_equal`). Every one is an ordinary lichen
definition — no compiler support, no native call — and the **contract** is the
class refinement `(_ ! in_num)` inside the operand array's element type, whose
predicate is applied to the operand's *type* (`in_num = t => t @in Num`), so no
type read is involved. The one-argument operand group is what gives the operator
its **tie** (the array's shared element type) and its **arity** (the array's
length).

The unit is a **package**: its bindings are the fields of its exported record, so
a program that wants the module value can also write `core = import "core"` and
reach `core.add` — the same handle serves both, which is what keeps the explicit
import and the implicit prelude one code path (`PackageHandle::direct` is what
the prelude adds, not a second module).

## 2. The three decisions

- **It is a prelude, not a library.** The names are in scope with **no import**:
  every source a host compiles is seeded with the module, so `add 1.5 2.5` is
  `4.0` in an empty file. An operator's contract is a fact about the *language*,
  not something each program imports.
- **It is shadowable, not reserved.** The prelude import is seeded **first**, so a
  program's own binding or import of the same name resolves later and wins:
  `add = x => y => 99; add 1 2` is `99`. That is the opposite choice from `set`
  and `@in` (reserved words), and deliberately so — the prelude is *library*, and
  a program that wants different arithmetic must be able to say so.
- **It is not a package the store counts.** `PackageStore::compiled` observes the
  packages a *program* asked for (an explicit `import`, a `depend`), which is what
  the cache and identity tests measure; the prelude is the language compiling
  itself and every store pays it exactly once, so `register_core` does not
  increment the counter. It is also never published to the device (§4).

## 3. The mechanism

- **`direct` is the seeding hook, and this is its first user.** A package's
  `direct` list is its `(name, export)` bindings, bound as base-scope names
  alongside the import's own name; the prelude's list is one entry per top-level
  binding. A record's *value* holds its fields' **values** (their types live in
  the struct's kind), so a field's `[value, type]` pair is not recoverable from
  the frozen struct: `core_terms` reads each binding's pair from the **checked
  build** and `core_direct` maps it through the freeze. A name/binding count
  mismatch is an error, not a truncation, so an edit to `core.lichen` cannot
  silently lose a name.
- **The built-in modules are exempt from the prelude.** `core` cannot be its own
  prelude (the load would re-enter itself) and `compute.lichen` is a plugin's
  private source compiled against that plugin's own native registry — it spells
  the read it needs and must not depend on a prelude. Both are recognised by
  their virtual path (`is_builtin_source`).
- **An embedded dependency contributes the all-zero sentinel.** The prelude is
  never published to the device store (like `compute`), and
  `DeviceRegistry::artifact_identity` already folds an unpublished dependency as
  zeros *on both sides*, so a dependent's cache still hits. A package's artifact
  therefore records the prelude among its dependencies and re-keys when the
  prelude's source changes — the correct invalidation for a prelude.
- **A registered built-in is reused, never re-frozen.** A host may build a
  `PackageStore` **per run** over **one shared registry** — the editor's worker
  does exactly that, one store per analysis so its packages map is that
  document's import closure — so the *second* store would compile the prelude
  again and freeze it under the *same* device key, which `Registry::freeze_mapped`
  refuses. `registered_builtin` therefore checks the shared registry first and
  adopts the module already filed under that key; what a handle needs is in the
  package meta (`HighPackageMeta::export` plus `direct`).

## 4. The built-in's own file

A built-in is a **file**: the store materializes its source under the cache root
(`<cache>/builtin/core.lichen`, `…/compute.lichen`) and keeps a **source record**
in the package meta — the path, the text, and the position of every frozen node
(`HighPackageMeta::source`, built by `located_nodes` + `builtin_source` from the
build's `node_edges` and the frontend's `span_index`). Two things follow:

- **A failure inside the module is attributed to the line that wrote it.** A
  failed assert whose condition was cloned out of a static module names it by a
  `StaticNodeId`; the diagnostics emit that static ref instead of dropping the
  failure, and the package layer resolves it through the record to a position in
  *that* file. Measured — `add ["a", "b"]` (the element outside the class):

  ```
  error: assertion failed: expected 1, found 0
    --> ~/.lichen/compilers/<key>/builtin/core.lichen:3:39
     |
   3 | add = operands => { operands : array<(_ ! in_num), 2>; operands[0] + operands[1] }
     |                                       ^
  ```

  The wording is the assert channel's own rather than `does not satisfy {Int,
  Float}` (the open item below). A module with **no** kept source — an ordinary
  imported package, whose own build already reported the failure when it
  compiled — drops as before. An in-memory store has no root to materialize
  under: the file is named by its own path (`core.lichen`) and still carries
  positions.
- **A jump lands in it.** The prelude import's names are seeded into the base
  scope as definitions whose file is this record and whose position is the name's
  own binding in it, so a use of `add` resolves to `core.lichen` and
  `textDocument/definition` answers with that file's URI and position — which the
  client can open, because the file exists on disk. A failure inside the built-in
  is published the same way, as its own `publishDiagnostics` for that file rather
  than as a diagnostic on the document. Two things make it work: a built-in
  definition must not enter any table keyed by a *document* span (the prelude
  import's own span is synthetic, and its bindings' positions are `core.lichen`'s),
  and the position is read from the record's **text**, not from its frozen-node
  spans (those cover the nodes a *failure* can name, which is not every binding's
  export node).

## 5. What is open

- **A failure inside the prelude is unattributed from the document's point of
  view.** The line inside the built-in is named (§4), but **the domain spelling
  is not carried across modules**: a refusal inside the prelude reads
  `assertion failed: expected 1, found 0` where the same assertion written in the
  document reads `does not satisfy {Int, Float}`, because the spelling
  (`AssertSpelling::Refinement { domain }`) and the domain node live in the
  *built-in's* build, and the record keeps positions, not checker facts. Carrying
  them — the spelling table plus the domain's node, printed through the module the
  failure names — is the next step on this leg.
- **The call site is not attached.** The diagnostic names the built-in's line; it
  does not also name the application in the document that failed the contract,
  because an assert's clone records its *template* and not the apply that made it.
  Recording that origin — the apply node, at the two clone sites — and resolving
  it through `Build::apply_edges` is what would add the "related" location
  (`Diag::related` is plumbed for it and nothing fills it). Half of that mechanism
  has landed for a failure's *own* location: the static materializer records the
  apply it materialized a clone for as that clone's origin, so a runtime failure
  inside a materialized imported function — a deferred instantiation's table miss —
  points at the argument the caller passed. The clone the *dynamic* walk makes
  still records only its template, and neither path produces a `related` location.
- **The built-in's names are not hovered with a type.** A jump and a completion
  item for a prelude name name the built-in's file; the hover shows the definition
  line in it (`defined at line 3 of core.lichen`). It does not render
  `value : type`, because that snapshot is the *built-in's* build — the record
  keeps positions and text, not checker facts — and the compiler is not this
  layer's to re-run.
- **What the jump and the publish are verified against.** `Doc`'s unit tests hold
  the in-memory case; the integration test runs the real binary, so the file path
  the store materializes under the cache root is the one the server names and the
  failure is published as a second `publishDiagnostics` for it. Not verified: a
  client actually opening that URI (the server never opens documents itself).
- **The built-in is compiled by every store and not device-cached.** It is a
  virtual module (`persist::virtual_file_id`), compiled fresh in memory like
  `compute`; its values are ordinary (functions and a set of type values), so
  caching it is *possible* and simply not done.
- **The surface operators route here.** `+`, `-`, `*`, `/` and the four order
  comparisons lower onto this module's bindings — the resolver hands out the
  prelude's binders and the lowering applies one to the operand group — so
  `1 + 1.5` is refused by *this file's* tie and `1 + 2` is this file's `add`, with
  no builtin contract restated in Rust. `==`/`!=` (unconstrained), `%` and the
  bitwise trio (`Int`-only, a single class the checker pins) are deliberately not
  routed. The built-in module's **own** body keeps the machine operators — it has
  no prelude, since the prelude is what it is — which is what makes the module the
  implementation rather than a caller of itself.
- **A kernel body cannot call a binding *in an argument position*.** A routed
  operator *is* an apply of a static function, and a kernel body may use one where
  it is the body's own result, on a call's result, or inside an inlined helper —
  but an operator inside a cross-kernel call's argument (`k0 (x + 1)`) or inside
  `compute.launch`'s argument has no machine node behind it and is **refused by
  name**, which `crates/lichen-language/tests/compute.rs` pins. Widening the
  boundary is the kernel workstream's specialize-before-JIT pass, which folds the
  apply back to a machine leaf.

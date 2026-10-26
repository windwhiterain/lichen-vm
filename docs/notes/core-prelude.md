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
Num := set(Int, Float)
in_num := t => t @in Num
let Pair := _x => (_ ! in_num, _ ! in_num)
add := operands => { operands <: Pair; operands(0) + operands(1) }
-- …and one binding per remaining polymorphic operator
```

Eleven bindings: the class domain `Num`, the predicate `in_num` that consults it,
the shared constructor `Pair`, and one per polymorphic operator (`add`, `sub`,
`mul`, `div`, `less`, `greater`, `less_or_equal`, `greater_or_equal`). Every one
is an ordinary lichen definition — no compiler support, no native call — and
the **contract** is the class refinement `(_ ! in_num)` inside the operand
tuple's two positions, whose predicate is applied to the operand's *type*
(`in_num = t => t @in Num`), so no type read is involved. `Pair` is a
**constructor**: the match `operands <: Pair` applies it once, and the two
positions it returns are the requirements the operands must meet. The
two-element operand group is what gives the operator its **tie** (the group is
homogeneous) and its **arity** (the group has two positions).

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
  the struct's kind), so a field's `[value, type]` expr is not recoverable from
  the frozen struct: `core_terms` reads each binding's expr from the **checked
  build** and `core_direct` maps it through the freeze. A name/binding count
  mismatch is an error, not a truncation, so an edit to `core.lichen` cannot
  silently lose a name.
- **The built-in `core` module is exempt from the prelude.** It cannot be its own
  prelude (the load would re-enter itself), recognised by its virtual path
  (`is_builtin_source`). A plugin's own package needs no exemption: an embedded
  wrapper is compiled by `register_native`, which passes no imports at all, so
  the prelude is never in scope for it.
- **An embedded dependency contributes the all-zero sentinel.** The prelude is
  never published to the device store (like any native plugin's package), and
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
(`<cache>/builtin/core.lichen`, and `<cache>/builtin/compute.lichen` for a host
that composes the compute sub-language) and keeps a **source record**
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
   3 | add = operands => { operands : array{(_ ! in_num), 2}; operands[0] + operands[1] }
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

- **A failure inside the prelude now carries its spelling across the boundary — closed.**
  The line inside the built-in was already named (§4); what did not cross modules was the
  **domain spelling**, so a refusal inside the prelude read `assertion failed: expected 1,
  found 0` where the same assertion written in the document reads `does not satisfy {Int,
  Float}`. The spelling (`AssertSpelling::Refinement { domain }`) and its domain node live
  in the built-in's build, which is the only place they can be printed; so that build renders
  them, and `PackageSource` carries `(frozen template index, text)` beside the positions.
  When a diagnostic resolves through a static template, the reading build uses the carried
  text for a spelling its own table has no entry for. Measured: `add ["a", "b"]` reports the
  class failure as **`does not satisfy {Int, Float}`** at `core.lichen:3:56` (was the
  channel's generic wording), beside the annotation's own `assertion failed: expected 1,
  found 0` at `3:39` — that one is an `AssertSpelling::Condition` by registration
  (`checker/annotations.rs`) and is meant to stay generic.
- **The call site is now attached — closed.** A failure cloned out of the prelude
  names the built-in's line **and** the failing use in the document. The static
  materializer already recorded the apply a clone was materialized for as that
  clone's origin; the static-template diagnostic had been dropping the clone, so
  the origin was never resolved. It now carries the clone (`Diag::clone`), the
  report resolves the clone's origin through the build's `node_loc` and the
  document's `span_index`, `Diag::related` holds the caller's use, and the renderer
  prints it as a `note:` block in the document's coordinates. Measured:
  `add ["a", "b"]` reports `core.lichen:3:39` / `3:56` **and** `1:5` (the array
  argument). The dynamic walk's clone still records only its template, so a
  prelude body cloned at run time would have no related use.
- **The built-in's names are hovered with their type — closed.** A jump and a
  completion item for a prelude name name the built-in's file, and the hover now
  renders the export's type beside its definition line. The type is not recomputed by
  the editor: the built-in's build renders each exported binding's type with its own
  module's printer, `PackageSource` carries it keyed by the frozen export expr (beside
  the positions and the assert spellings), and the analysis resolves a name through
  the import's `direct` list to that expr. Measured: hovering `add` reads
  `` `add` — `array{raw[?b, ?c], 2} -> raw[?b, ?c]` — defined at line `3` of `core.lichen` ``.
- **What the jump and the publish are verified against.** `Doc`'s unit tests hold
  the in-memory case; the integration test runs the real binary, so the file path
  the store materializes under the cache root is the one the server names and the
  failure is published as a second `publishDiagnostics` for it.
- **A client opens the URI the jump names — verified.** The last gap was the
  client's side: the server never opens documents itself. A probe built a `Doc`
  with a persistent cache root, resolved `add`'s definition, took the server's own
  conversion (`Url::from_file_path(&file.path)`, as `server.rs`'s goto-definition
  does), then did what a client does — parsed the URI, turned it back into a path,
  and read the file. Measured:
  `recorded path = …\.probe-cache\builtin\core.lichen`; `server uri =
  file:///…/builtin/core.lichen`; the client's path equals the recorded one and
  `std::fs::read_to_string` answers **750 bytes containing `add = operands`**.
  So the URI is openable and its file is the prelude source, not an empty stub.
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
  name**, which `crates/lichen-compute/tests/lichen_compute/compute.rs` pins. Widening the
  boundary is the kernel workstream's specialize-before-JIT pass, which folds the
  apply back to a machine leaf.

## Recovered measurements

- **Presenting a built-in's own diagnostics.** A failure inside a built-in is a
  property of **that file**, so `Diag::file` names it: `render_diagnostics`
  filters those out of the document's set (`d.file.is_none()`) and
  `file_diagnostics` renders each group against the built-in's own text and line
  starts. The server publishes every such set against
  `Url::from_file_path(path)` **and remembers the URIs it published**, because a
  client keeps a file's diagnostics until an *empty* list replaces them — without
  that memory a fixed error would sit on the built-in's line for the rest of the
  session. A diagnostic with a `file` but no `span` is dropped rather than given
  the document's coordinates: the document must never be blamed for a position it
  does not contain. The same rule runs through go-to-definition
  (`Definition::file` decides which file's `line_starts` render the range) and
  through completion, where a **built-in** name has no document span at all, so
  its `detail` names the built-in's file rather than a checked type.

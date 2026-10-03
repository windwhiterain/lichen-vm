# `core`: the built-in prelude

> Status: **current** — the module is embedded in the language crate
> (`crates/lichen-language/src/core.lichen`), registered as a built-in virtual
> package ([`PackageStore::register_core`](../../crates/lichen-language/src/package.rs)),
> and seeded into every source the preprocessor sees
> ([`crate::preprocess::preprocess`](../../crates/lichen-language/src/preprocess/mod.rs),
> `PackageStore::prelude_import`).
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

## 4. What this costs, and what is open

- **A failure inside the prelude is unattributed.**  `add "a" "b"` is refused,
  but the diagnostic reads *"the build failed, but the failing check could not be
  attributed to an expression in this source"*: the failing assert's **template**
  belongs to the `core` module, and the importer's build has no expression to
  blame.  That is the pre-existing, pinned behaviour for an assert inside an
  imported package
  (`crates/lichen-language/tests/registry.rs`'s
  `a_failed_assert_in_an_imported_package_still_reports_a_diagnostic`) — what the
  prelude changes is that it is now the *first* thing a program meets, for a
  contract nobody wrote locally.  Two fixes are open and both are diagnostics
  work: attribute a failure whose template belongs to another module to the
  importer's **call site** (the apply channel already records the argument's
  span), or at least name the module the template came from.
- **The contract is enforced twice while the routing is the checker's.**  `+`
  still lowers to the machine leaf and the checker still registers the builtin's
  domain assert (routing R3, [operator-polymorphism](operator-polymorphism.md)
  §7), so `core`'s class refinement and the builtin's assert both fire on a
  wrong-class operand — the *message* the user sees is still the builtin's
  `does not satisfy {Int, Float}` when it can be attributed at all.  Moving the
  surface operator onto this binding (R3 → R2/R1) is what would make `core` the
  single authority.
- **The prelude is compiled by every store and not device-cached.**  It is a
  virtual module (`persist::virtual_file_id`), compiled fresh in memory like
  `compute`; its values are ordinary (functions and a set of type values), so
  caching it is *possible* and simply not done — a store's first compile pays one
  small module.

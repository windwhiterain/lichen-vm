# The plugin taxonomy: native plugins vs. compiler plugins

> Status: current
> Points at: `crates/lichen-highlevel/src/plugin.rs` (`NativePlugin`),
> `crates/lichen-compute` and `lichen-std-native` (the reference native plugins),
> `crates/lichen-perspective` (the established compiler plugin),
> `crates/lichen-language/src/program.rs` (`lang_compose_vocabulary!`),
> `crates/lichen-language/src/package.rs` (`register_native`), and
> `crates/lichen-compute/src/language.rs` (the compute sub-language).

The lichen system extends itself through a compile-time composition, not a
loadable ABI. There are **two kinds of extension**, split by one question:
**does the language layer have to change (codesign) to accommodate it, or not?**

## Native plugin (pluggable into a fixed language layer)

A **native plugin** is a crate that extends the core — `lichen-utils`,
`lichen-lowlevel`, `lichen-highlevel` — through the program-generic extension
points, and composes into a *fixed* host layer **without the host codesigning
with it**. It never names the host's concrete `Program` marker, its IR, its
grammar, or its on-disk format.

It contributes, in any combination:

- **vocabulary leaves** — value/operator enums the host combines with
  `lichen_utils::enum_ext!`;
- **native operators** — `NativeOp<P>` impls, exposed through a
  plugin-provided `#[macro_export] macro_rules! <name>_native_ops` that, for a
  host program `$program`, expands to a `NativeOps` value (the host invokes it
  to build its private per-module registry);
- **an attribute** — an `AttrSpec` marker + `AttrExt<P>` provider, listed in
  the host composition's `attrs` manifest, which is also where its pair slot
  comes from (the manifest's declaration order *is* the canonical attribute
  order — see [attributes.md](attributes.md));
- **a `GlobalExt` component** — composed by the host with
  `lichen_utils::compose_ext!`.

Crucially, a native plugin is **program-generic**: every entry point is
bounded by the lowlevel/highlevel `Program` marker and the extension-point
traits, so it never depends on a specific language crate. It also **adds no
syntax** — no grammar production, no IR node, no schema-tail literal, no
persist discriminator.

`lichen-compute` is the reference native plugin: it contributes
`ComputeValue`/`ComputeOperator` leaves, an `OperatorExt<P>`/`NativeOp<P>`
impls, and a `compute_native_ops!` macro. Those leaves, operators and ops stay
program-generic; the crate is also the one that **composes them into a
sub-language** (§ 5).

**The "fixed slot".** Because enum composition is a compile-time expansion, a
native plugin set is fixed at **build** time, not runtime. The language layer
exposes a single manifest — `lang_compose_vocabulary!` (in `lichen-language`)
— that declares the whole plugin set. Its own manifest
(`crates/lichen-language/src/program.rs`) composes the language's leaves and
**no plugin's**: the bare layer is a program on its own, so it can be built and
tested without any plugin linked in. A host composes on top of it:

```
lang_compose_vocabulary! {
    attrs = [ Perspective as Perspective; Doc as Doc; ];
    [ P::Operator: From<GcdOp> + From<LowOperator> ];
    values = [ LowValue as LowValue; TypeValue as TypeValue;
               lichen_compute::ComputeValue as ComputeValue; ];
    operators = [ LowOperator as LowOperator; TypeOperator as TypeOperator;
                  GcdOp as GcdOp;
                  lichen_compute::ComputeOperator as ComputeOperator; ];
    render = [ lichen_compute::language::value_name ; ];
}
```

and assembles each plugin's private per-module registry with its
`<name>_native_ops!` macro (`licher_compute::compute_native_ops!(LangProgram)`),
handing the store the plugin's embedded package as a `NativePackage`
(`lichen_language::package`). The frontend, checker, and VM are reused
unchanged. **Adding a native plugin is one manifest line** (and one
op-registry line).

`lang_compose_vocabulary!` generates the *whole* composed program, not just the
enums: for a plugin set it also emits the `ValueExt` / `ValueType` /
`OperatorExt` impls (the type-constant markers delegated to the core
`TypeValue` leaf and the
operator union's `run` dispatching every leaf), the per-leaf artifact codec, and
the `LangProgramShape` impl that binds the marker to the language's tooling.
So a composed program marker is a live, executable `Program` — its operators
actually run, and a plugin compiler can be driven over it.

The `render = [...]` slot is where a composition states the value spellings only
it knows. It reaches the printers through
`LangProgramShape::value_name`, so `render::print_type_lang` /
`print_value_lang` name a plugin's leaves without the language naming them: a
manifest that declares no `render` takes the shape's default, which names
nothing and leaves the printer to write `?`.

The composed marker it emits is a fresh **nominal type** around
`ProgramImpl<LangValue, LangOperator, LangAttr>`, deliberately not a type alias. A
composition site inside the language crate may implement a trait for an alias of its own
types, but an external one (a plugin-built compiler crate, a test host program) cannot:
`impl ProgramCodecOf for <alias-of-ProgramImpl>` is E0117, because the alias unwraps to a
foreign `ProgramImpl` and the trait is foreign. The newtype is what makes the generated
`Program` / `HighProgram` / `ProgramCodecOf` / `OperatorExt` impls orphan-legal from any
crate that composes the vocabulary.

A native plugin contributes its leaves through a `#[macro_export]` leaf macro
named `<crate_ident>_leaves` (e.g. `lichen_compute_leaves`,
`lichen_std_native_leaves`), and the manifest lists it as
`plugins = [ <crate_ident> as <crate_ident>_leaves; ... ]`.  The name is
per-plugin and crate-derivable by design: two `#[macro_export]` macros named
identically across the dependency graph collide in the extern prelude, so a
shared `liche_leaves` name would break any host composing more than one plugin.

This is the "package manager pulls a crate and builds a new compiler" story:
the reusable layer is the generic core + the composition manifest, and a built
compiler is just a particular plugin set substituted into that manifest.

## Compiler plugin (codesigned with the language layer)

A **compiler plugin** is a feature the language layer must be **written to know**.
It needs one or more of: a **grammar production**, an **AST node**, an **IR
form**, a **schema-tail literal**, or a **persist discriminator**. Because the
language's frontend is not attribute/feature-generic, the language and the
feature are designed *together* — you cannot package-manager-pull it.

`Perspective` (the `# p` attribute) is the reference compiler plugin: its
*micro-semantics* (the `AttrExt`, `GcdOp`, `gcd`/`divides`) are
native-plugin-shaped, but the feature also requires

- the `#` grammar production (`AnnPiece::Perspective` in `parse.rs`),
- AST fields (`perspective` / `parameter_perspective`),
- `IR<Perspective>` + `Schema { tail: vec![Perspective] }` (`compile.rs`),
- the `LangOperator::GcdOp(GcdOp::Gcd)` persist discriminator (`persist.rs`).

So `Perspective` is a compiler plugin: the language layer owns and codesigns
those sites. Its **semantic core** — the program-generic `AttrExt<P>`,
`OperatorExt<P> for GcdOp`, `gcd`/`divides`, and `persp_attr_ext::<P>()` — is
established as its own crate, `lichen-perspective`, so the lattice meaning is
shared across hosts and testable without one. The codesign sites (grammar, AST,
IR, persist) stay in the language layer; that is exactly why it is not a native
plugin — it invents syntax/IR/persist, so no fixed host can pull it unchanged.

## The decision rule

> If a feature needs a new grammar production, an IR node, a schema-tail
> literal, or a persist node, it is a **compiler plugin** (the language
> codesigns with it). If it only supplies vocabulary leaves + extension-point
> values (and adds no syntax), it is a **native plugin** (composable into a
> fixed layer, regenerable by a package manager).

## Why the split matters

- A native plugin is **reusable by any host**: a test harness, a different
  frontend, a future compiler, all compose the same leaves unchanged. The
  *leaves* depend on no language crate; only the composition that names them
  does.
- The **language layer** shrinks: it owns the grammar/IR/persist, the
  composition manifest, and the checker wiring — not whole subsystems, and not
  any plugin's vocabulary.
- The **plugin boundary is checkable**: "program-generic + no syntax" is an
  objective test, so a crate either is or is not a native plugin.

## 5. The compute sub-language

A native plugin's leaves become a language only where someone composes them.
`lichen-compute` composes its own, in `crates/lichen-compute/src/language.rs`:
the bare language's leaves plus `ComputeValue`/`ComputeOperator`, the `render`
slot that names the compute values, and the embedded `compute.lichen` served as
a native virtual package (`native_packages`, `new_store`, `register_compute`).
`lichen-compiler`, `lichen-language-server` and `lichen-tools` drive that
composition; `lichen-language` does not name it.

**What else moved with it.** A plugin's leaves and the passes that read them are
one thing, so the graph-fusion machinery — `region_merge`, `automatic_tiling`,
`fusion_claim`, `placement_pack`, `floor` and the pack loader `packs` — is in
`lichen-compute` beside the leaves it consumes, not in the language layer beside
the vocabulary it composes. The pass channel a run reports travels with them:
`run::PassReport::Reported` carries the **categories** the host named, and
`package::Wiring` carries what a command line's `--pack` installs, so the CLI
names no pass and the language names no diagnostic category.

**The invariant this buys.** `lichen-language` links no plugin, so its own
build, tests and examples run against the language alone — the frontend,
checker, session and artifact cache with no compute vocabulary present at all.
A program that imports `compute.lichen` is a *host* decision, and the boundary
is one manifest line per plugin.

## Current state

- `lichen-compute` is a native plugin (its own crate; `NativePlugin` marker +
  `compute_native_ops!`) **and** the host that composes its own leaves into the
  compute sub-language (§ 5).
- `lichen-std-native` is a native plugin (its own crate; `NativePlugin` marker +
  `SortOp` leaf, `native-op` impl, `lichen_std_native_ops!`, and an embedded
  `std.lichen` wrapper source served through a native virtual package).  It is
  **not** in the shipping compiler's vocabulary manifest; a host composes it
  and drives it the way a package manager plugs a native plugin in.
- `lichen-perspective` is a compiler plugin (its own crate): the program-generic
  semantic core (`AttrExt<P>`, `OperatorExt<P> for GcdOp`, `gcd`/`divides`,
  `persp_attr_ext::<P>()`).
- `lichen-doc` is a compiler plugin (its own crate): the program-generic
  semantic core of the doc attribute (`AttrExt<P>`, `doc_attr_ext::<P>()`) —
  the `? expr` **label** that attaches struct metadata to an expression.
- `lichen-render` is a **shared** program-generic printer crate (not a plugin):
  the `TypePrinter` / `ValuePrinter` / `print_type` / `print_value` /
  `render_attributes` / `render_struct_fields_named` pretty-view of the
  generic core.  A plugin that spells its own attribute slot in a host's
  syntax (e.g. `lichen-doc`'s `? name = "…"`) reuses it instead of carrying
  its own printer.
- `lichen-language` composes **no plugin at all**: its manifest in
  `program.rs` is the language's own leaves, and `LangProgramShape` is the impl
  that composition emits, so a plugin's leaves reach the tooling only through a
  host that names them.  The compiler plugins' leaves (`Perspective` + `GcdOp`,
  `Doc`, `Refinement`) are the language's own codesign sites, so they are in
  that manifest.  Its `render` module re-exports `lichen-render`
  and layers the host-specific shells (the caret diagnostic, the
  checker-message wording) on top.
- **The language layer's tooling is generic over the program**: the package
  store, run, session, render and the end-to-end `testing` helpers are
  parameterized by the program shape
  (`lichen_language::LangProgramShape`), whose value/operator vocabularies vary
  (`CompiledProgram<V, O>`) while the attr type is fixed to the language's
  `LangAttr`.  So a compiler over an additional native plugin has the generic
  library available to it; the composed compiler is driven through
  `lichen_compiler::cli` (its own crate, out of the library) over its own
  composed vocabulary, and the composition
  macro emits a **per-leaf
  [`ProgramCodec`]** (`lichen_language::persist`) so a built compiler writes a
  real device cache.  The compiler's **artifact cache is scoped per plugin
  set**: `lichen_compiler::cli` takes an explicit cache root, and a generated
  compiler passes its own plugin-set slot (`<lichendir>/compilers/<key>`), so
  its compile artifacts never collide with (or reuse) another plugin set's —
  each vocabulary reads/writes its own `artifacts/` store.
- The per-leaf artifact codecs — `OperatorCodec`, and `ValueCodec` for a value leaf — are
  what a native plugin implements when it wants its *built* compiler to keep a persistent
  `~/.lichen` cache. A scalar operator's codec is a one-tag identical round-trip: `SortOp`'s
  is `Sort` ↔ `0`.
- `Perspective` and `Doc`'s codesign sites (grammar `# p` / `? expr`, AST
  fields, `IR<…>` schema tails, the `GcdOp` persist discriminator) stay in
  `lichen-language`.

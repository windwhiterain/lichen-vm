# The plugin taxonomy: native plugins vs. compiler plugins

> Status: current
> Points at: `crates/lichen-highlevel/src/plugin.rs` (`NativePlugin`),
> `crates/lichen-compute` (the reference native plugin),
> `crates/lichen-perspective` (the established compiler plugin),
> `crates/lichen-language/src/program.rs` (`lang_compose_vocabulary!`),
> and `crates/lichen-language/src/package.rs` (`compute_native_ops!`).

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
  [`enum_ext!`](lichen_utils::enum_ext);
- **native operators** — `NativeOp<P>` impls, exposed through a
  plugin-provided `#[macro_export] macro_rules! <name>_native_ops` that, for a
  host program `$program`, expands to a `NativeOps` value (the host invokes it
  to build its private per-module registry);
- **an attribute** — an `AttrSpec` marker + `AttrExt<P>` provider;
- **a `GlobalExt` component** — composed by the host with
  [`compose_ext!`](lichen_utils::compose_ext).

Crucially, a native plugin is **program-generic**: every entry point is
bounded by the lowlevel/highlevel `Program` marker and the extension-point
traits, so it never depends on a specific language crate. It also **adds no
syntax** — no grammar production, no IR node, no schema-tail literal, no
persist discriminator.

`lichen-compute` is the reference native plugin: it contributes
`ComputeValue`/`ComputeOperator` leaves, an `OperatorExt<P>`/`NativeOp<P>`
impls, and a `compute_native_ops!` macro. It is host-agnostic.

**The "fixed slot".** Because enum composition is a compile-time expansion, a
native plugin set is fixed at **build** time, not runtime. The language layer
exposes a single manifest — `lang_compose_vocabulary!` (in `lichen-language`)
— that declares the whole plugin set. A host (now, or a package manager later)
composes:

```
lang_compose_vocabulary! {
    attr = Perspective;
    values = [ LowValue as LowValue; TypeValue as TypeValue;
               lichen_compute::ComputeValue as ComputeValue; ];
    operators = [ LowOperator as LowOperator; TypeOperator as TypeOperator;
                  GcdOp as GcdOp;
                  lichen_compute::ComputeOperator as ComputeOperator; ];
}
```

and assembles each plugin's private per-module registry with its
`<name>_native_ops!` macro (`licher_compute::compute_native_ops!(LangProgram)`).
The frontend, checker, and VM are reused unchanged. **Adding a native plugin is
one manifest line** (and one op-registry line).

`lang_compose_vocabulary!` generates the *whole* composed program, not just the
enums: for a plugin set it also emits the `ValueExt` / `ValueType` /
`OperatorExt` impls (the type-constant markers delegated to the core
`TypeValue` leaf, function-kind markers via each leaf's `FunctionKind`, and the
operator union's `run` dispatching every leaf).  So a composed program marker is
a live, executable `Program` — its operators actually run, and a plugin compiler
can be driven over it.

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
  frontend, a future compiler, all compose `lichen-compute` unchanged. It never
  depends on a specific language crate.
- The **language layer** shrinks: it owns the grammar/IR/persist, the
  composition manifest, and the checker wiring — not whole subsystems.
- The **plugin boundary is checkable**: "program-generic + no syntax" is an
  objective test, so a crate either is or is not a native plugin.

## Current state

- `lichen-compute` is a native plugin (its own crate; `NativePlugin` marker +
  `compute_native_ops!`).
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
- `lichen-language` composes the plugins: the native plugin's leaves and op
  registry via `lang_compose_vocabulary!` / `compute_native_ops!`, and the
  compiler plugins' leaves (`Perspective` + `GcdOp`, `Doc`) via
  `lang_compose_vocabulary!`.  Its `render` module re-exports `lichen-render`
  and layers the host-specific shells (the caret diagnostic, the
  checker-message wording) on top.
- `Perspective` and `Doc`'s codesign sites (grammar `# p` / `? expr`, AST
  fields, `IR<…>` schema tails, the `GcdOp` persist discriminator) stay in
  `lichen-language`.

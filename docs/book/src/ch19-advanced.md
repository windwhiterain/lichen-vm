# Advanced Features: Extending the Language

lichen's extension story is unusual. A plugin is not a dynamically loaded
library; it is a crate **composed into the compiler at build time**. The
language layer is the fixed slot, and a plugin fills it with vocabulary,
operators, attributes, and sometimes a whole sub-language. This chapter
explains the two kinds of extension, the `$name` operator form they
enable, and what the shipping compiler is composed of.

## The two kinds of extension

The split is one question: **does the language layer have to change to
accommodate it?**

- A **native plugin** extends the core (`lichen-lowlevel`,
  `lichen-highlevel`) through program-generic extension points and composes
  into a *fixed* host layer without the host changing for it. It adds no
  syntax: the plugin set is fixed when the compiler is built.
- A **compiler plugin** is codesigned with the language layer: the host's
  composition manifest names it, and it may contribute an attribute, an
  operator leaf, or a value kind that the language layer then carries.

`lichen-perspective` is the established compiler plugin: it contributes
the perspective attribute (Chapter 13) and the `gcd` meet its lattice
needs. `lichen-compute` is the reference **native plugin**: it contributes
the compute sub-language (Chapter 17) — value kinds such as `Buf` and
`Kernel`, operators, and its own embedded source — through the generic
extension points.

## Native operators: `$name(args)`

A plugin may expose operations that are not expressible in the bare
language. Its own source calls them with the `$` form:

```lichen std.lichen
sort = a => {a: array{Int, _}; r = $sort(a); r: array{Int, _}; r}
```

This is the whole of `lichen-std-native`'s source: `$sort` is a native
operator the plugin registers, and the surrounding code is ordinary
lichen. The `$` form is an atom — `$jit(f)`, `$launch(k, x)`,
`$noop()` — and it resolves only against the **private native-operator
registry of the module being compiled**. The shipping compiler has no
`sort` operator, so it refuses the same source:

```text
error: unresolved native operator 'sort' — this module composes no plugin registering it
  --> 1:36
   |
 1 | sort = a => {a: array{Int, _}; r = $sort(a); r: array{Int, _}; r}
   |                                    ^
```

(The output continues with a cascade error, because the unresolved operator
leaves a type gap; the first diagnostic is the one to read.) With a
compiler built from a plugin set that registers `sort`, the same source
resolves.

Two properties follow from the private registry. First, **`$name` is not a
name**: it can be neither bound nor referenced without the prefix, and what
a statement binds is the operator's result. Second, **plugins cannot
collide**: two plugins may each register a `$jit` and each resolves only
for its own source. The checker compiles the arguments, adopts the
`[value, type]` pair the plugin's builder returns, and knows nothing else
about what the operator does. That is a private contract between a plugin
and its own source.

The compute package is the largest example: its embedded source uses
`$jit(f)` to compile a function to a kernel and `$launch(...)` to run one
(Chapter 17). The bare language knows neither.

## The composition manifest

The plugin set is fixed at build time because the composition is a
macro. The language layer exposes one manifest that declares the whole
set: the vocabulary leaves (values and operators), the attribute slots,
and the renderers. A host composes on top of the bare layer:

```text
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

The bare layer's own manifest composes **no plugin's** entries, so the
bare language is a program on its own: it can be built and tested without
any plugin linked in. A host — the shipping compiler is one — adds the
plugins it wants, and the resulting binary has exactly that vocabulary.

This is why "a plugin adds no syntax": the grammar, the IR nodes, the
attribute-tail literals and the persistence tags are the language layer's,
and a plugin plugs into them without owning any of them.

## What a native plugin contributes

In any combination:

- **vocabulary leaves** — value and operator enums the host combines with
  `enum_ext!`;
- **native operators** — `NativeOp` implementations, exposed through a
  plugin-provided macro that expands to a `NativeOps` value for a host
  program;
- **an attribute** — an attribute marker and its behaviour (how it
  combines, how it unifies, its subtype order), listed in the host's
  composition, whose declaration order is the canonical attribute order;
- **a global extension component** — state the host composes with
  `compose_ext!`.

The perspective plugin's attribute is exactly this: its order is
divisibility, its combine is `gcd` (Chapter 13), and the host names it in
the manifest.

## Placement packs

A program can steer the fusion machinery without changing a kernel's
meaning. A **placement pack** is a runner's plan for a node or a region:
where the staged values live, which region's operations become one kernel.
It is installed for a run with `--pack` (Chapter 18), and it is
**guidance, not an implementation** — a pack never says what anything
computes. A pack whose hash or constraints do not match is inert, so the
engine's own choice answers.

Packs are the mechanism behind the fusion notes: the graph path
(Chapter 17) reads a pack's schema and region decisions, and the operator
examples record which claims merge.

## The shipping compiler

The `lichen-compiler` binary this book used is the bare language composed
with the compute vocabulary, the perspective attribute, and the doc
attribute. That composition is why `compute.lichen` is importable with no
file, why `#` works, and why `$jit` is available to the compute source —
and why `$sort` is not, unless the compiler is rebuilt with that plugin.

A project that wants a different composition declares a `plug` in its
header (Chapter 15); the package manager fetches the plugin and rebuilds
the compiler, and the rebuilt binary carries the plugin's vocabulary. The
language a program is compiled by is, in this sense, a build artifact.

## Summary

- There are two kinds of extension: program-generic **native plugins** and
  **compiler plugins** codesigned with the language layer.
- A plugin's own source calls its operations with `$name(args)`; the name
  resolves only against that module's private registry, so plugins never
  collide and the bare language does not know them.
- The plugin set is fixed at build time by a composition manifest; the bare
  layer composes none.
- Native plugins contribute vocabulary leaves, native operators, attributes,
  and global extension components.
- A placement pack steers fusion placement for a run; it is guidance, never
  an implementation.
- The shipping compiler is the bare language plus compute, perspective and
  doc; a different composition is a different compiler.

## Exercises

1. Explain why `$sort` fails in the shipping compiler and succeeds in a
   compiler built with the std-native plugin.
2. Write down the two properties the private registry gives `$name`, and
   why each matters.
3. Name the two kinds of extension and give the repository's example of
   each.
4. Explain why a plugin "adds no syntax".
5. Read the composition in the chapter and say which part supplies the
   attribute slots and which supplies the compute values.
6. Describe what a placement pack may and may not say.

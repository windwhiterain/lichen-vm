# The compiler-plugin model

> Status: current
> Points at: `crates/lichen-utils/src/extend.rs` (`enum_ext!`),
> `crates/lichen-lowlevel/src/lib.rs` (`Program`, `OperatorExt`, `ValueExt`, `LowShape`),
> `crates/lichen-highlevel/src/program.rs` (`Ctx`, `ProgramImpl`, `TypeValue`/`TypeOperator`),
> `crates/lichen-highlevel/src/native.rs` (`NativeOp`/`NativeOps`/`NativeArg`/`NativeApply`),
> and `crates/lichen-compute/src/compute.rs` (the worked example: `lichen-compute`).

A **compiler plugin** here is a compile-time *composition*, not a loadable ABI. A
plugin extends the lichen system by three separable means, and the whole thing is glued
together by one type: the **`Program`** marker that fixes the program's vocabularies.
You do not load a `.so`; you add variants to the value/operator enums and register a
name→operator table for the plugin's own source.

## The `Program` marker fixes everything

```rust
pub trait Program {
    type Value:    ValueExt + From<LowValue> + AsEnum<LowValue> + Clone;
    type Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>;
    type GlobalExt: GlobalExt;
    type PackageMeta: Default;
}
```

A concrete program (e.g. `LangProgram = ProgramImpl<LangValue, LangOperator, Perspective>`,
and `HighProgram` adds `type Attr` / `type Literal` on top) is what the whole checker and
VM are generic over. The plugin participates by contributing member types to exactly
these associated types. Everything else — the VM, the registry, static modules, GC —
is reused unchanged.

`Self::Attr` is the one thing the highlevel adds on top of `Value` / `Operator` /
`GlobalExt` / `PackageMeta`, and the checker never names a concrete attribute — only
`Self::Attr`. A language frontend plugs in its own (e.g. `Perspective`); a program with no
attribute extension takes the inert default `NoAttr`. Every literal node carries a
`Self::Literal` value, built through `LiteralExt::build`.

## Extension point 1: the value / operator vocabularies

The lowlevel ships the *structural* core: `LowValue` (`USize`/`Array`/`Table`/`Function`/
`None`/`Error`/`Parameterized`) and `LowOperator` (`Index`/`Apply`/`TableGet`). A plugin composes
its own plain enums in as **sibling carry variants** of one flat union with
`lichen_utils::enum_ext!`:

```rust
enum_ext! {
    pub enum LangOperator {}
    + LowOperator as LowOperator;      // the structural core
    + TypeOperator as TypeOperator;    // the highlevel's type-level ops
    + GcdOp as GcdOp;                  // a language plugin (perspective gcd)
    + ComputeOperator as ComputeOperator;  // a compute plugin (Jit/Launch)
}
```

`enum_ext!` emits the enum plus, per extension, the `From<Ext>` and `AsEnum<Ext>` pair
the `Program` contract requires. The effect is that a value or operator union is a
**flat union** with no `Ext` wrapper and no nesting — an extension is one variant deep.

Two contracts gate membership:
- `Program::Value: ValueExt + From<LowValue> + AsEnum<LowValue>` — `ValueExt` is the
  cheap structural equality / handle carrier; and the layer provides `ValueType`
  (the value→type contract: marker constants, `type_id`).
- `Program::Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>`.

The VM dispatches the structural `LowOperator`s through `AsEnum` first; everything it
doesn't recognise reaches `OperatorExt::run`.

Chaining and merging are the same operation: a downstream vocabulary lists every ancestor
layer's enum plus its own in one invocation, and two vocabularies over the same ancestors
merge by listing both extensions. There is no nesting and no delegation between layers, so
no value is ever representable under two branches and `PartialEq` stays exact.

Every extension is `path as Variant`, naming the carry variant; a bare `+ Ext;` is shorthand
for a single `+ Ext as Ext;`. An extension should be a plain per-layer enum: composing an
already composed enum is legal but nests it (a carry variant holding a union), which defeats
the one-representation property. A carry variant colliding with one of the base's own variant
names is reported by rustc as a duplicate. `as_enum` clones the extension value out of the
borrowed base value, so every extension enum must be `Clone`.

## Extension point 2: the native-call IR + private registry

This is the "the checker knows nothing about me" extension. A plugin's own embedded
source calls `$name(args)`; the frontend parses it to a general `ExprKind::NativeCall`
node; the checker delegates to the *current module's* private registry.

```rust
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

pub trait NativeOp<P> {
    fn build(&self, ctx: &mut dyn Ctx<P>, e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply;
}
pub struct NativeArg  { pub expr: ExprId, pub value: NodeId }
pub struct NativeApply { pub value: NodeId, pub decided: bool }
```

- The args are **already compiled** (each carries its value node and never a type — an
  operator sees raw values), so `build` only checks the
  operator's types and emits the op node — through the curated `Ctx`, never raw lowlevel
  nodes. Its `NativeApply` answers with the expression's value node and whether that is a
  decided value or a computation the runtime reads.
- `Ctx` is the checker's encoding surface: `fresh` (a new undecided cell), `array_node`,
  `op_node`, `pair`, `kind_expr`, `universe`, the marker nodes, and `check_unify(_relaxed)`.
- `Ctx` is the encode/parse boundary: the highlevel owns the `[value, type]` grammar, and an
  extension expresses *content* through these semantic encoders rather than hand-assembling
  lowlevel nodes. Each encoder builds a well-formed structure by construction — a `Ctx::pair`
  is always `[value, type]`, a `Ctx::kind_expr` always `[marker, Type]` — and the active block
  is managed internally, so an extension can never build into the wrong block. The one
  deliberate exception is the lowlevel-only constructs a native operator genuinely needs — a
  fresh cell, an op node, an array shape; those are the *shape* of the grammar, not a bypass.
- `Ctx::arrow` builds `[[domain, codomain], [FunctionType, Type]]` — the single spelling of
  the arrow encoding — but does **not** register it in the checker's `arrows` set. That set
  drives the type *printer* (it is what renders a pair as `T -> U` instead of `<T, T>`), so
  only a genuinely source-level arrow belongs in it: an arrow built as a unification *pattern*,
  such as the function-ness guard's `[[?d, ?c], [FunctionType, K]]`, must stay out, and the
  caller that prints inserts it.
- The call's `[value, type]` pair is the checker's to build, never the builder's: `build`
  returns the value node it emitted. A builder that assembled a term by hand would be a
  second source of truth for the encoding.
- `NativeOp::build` need not state a type at all: `SortOp` emits a bare `Sort` operator node
  over the argument's value and answers `decided: true`. The user-facing types come from the
  embedded wrapper's annotations — `a : array<Int, _>` gates the argument, and the result
  annotation states a length-unknown `array<Int, _>` rather than an opaque native
  application; a sort preserves the length, so the length is only observed where it is read.
- **Privacy**: the registry is per-module and only the plugin's own file is compiled against
  it, so `$jit` resolves privately — a second plugin's `$jit` never collides. Every other
  file compiles with `no_native_ops()`.

The plugin's own source is an embedded `&str`, compiled at registration (e.g.
`register_compute` in `package.rs`) into a frozen module that `import "…"` resolves to.

## Extension point 3: runtime dispatch (`OperatorExt::run`)

The compile-time `NativeOp::build` handles *checking and emitting*. The runtime behaviour
of each operator lives in `OperatorExt::run`:

```rust
fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value;
```

A plugin's `Operator` variants are the ones `AsEnum<LowOperator>` doesn't recognise, so
they land here. `run` sees the possibly-lazy operand and returns a (possibly
`Parameterized`) value — staying lazy on an undecided operand is the disciplined behaviour,
leaving the type-error reporting to the definition pass.

`run` has one sibling, `low_type`, which states what the operator's computation *produces*
for the low-type pass (see [lowlevel-low-types](lowlevel-low-types.md)):

```rust
fn low_type(&self, arguments: &[Option<LowShape>]) -> Option<LowShape>;
```

`arguments` is one entry per element of the operand array; `None` returned means the
operator declines and its result stays undecided. The default declines, so a plugin that
needs no low-type statement implements nothing. Like `run`, it lives on the operator
because the meaning of an operator belongs to whoever defined it.

The type-level operators are delegated whole to the program-generic `OperatorExt` impl for
`TypeOperator`, so any composed union reuses the same semantics; the union's `low_type` is
the same uniform dispatch — each leaf states what its own computation produces — except
that the structural leaf answers `None`, because the pass owns the `LowOperator` transfers.
That delegate call is written qualified, since a transfer mentions no `P`-typed argument and
an unqualified call could not infer which program's impl is meant.

## Extension point 4: recording a diagnostic (`Module::extension_diagnostics`)

A plugin regularly decides something the lowlevel cannot describe on its own — a backend
that cannot lower a shape, a compiler that cannot compile a body. `Module::record_extension_diagnostic`
is the general channel for it:

```rust
fn record_extension_diagnostic(&mut self, category: &'static str, node: Option<NodeId>, message: impl Into<String>);
```

Every other channel on a `Module` (`unify_errors`, `eval_errors`, `assert_errors`,
`apply_errors`) is a typed channel for a fact the VM itself produces, so each is fixed by
the VM's own vocabulary. This one is not: the lowlevel stores the entry and knows nothing
about what `category` means, so **a new external error kind never means a new channel
here**. `compute.jit` uses it to say why a kernel stayed lazy instead of discarding the
reason. What a host *renders* from these entries is the host's own decision.

The host here is `lichen-language`, and it renders them in two places, because a `Module`
outlives its check in two ways: the report assembly (`lib.rs`) carries them into a failed
build's diagnostics, and `run::render_build` reports them when the program produced no
value — a `plrun` whose count is past the bound, whose refusal *is* the explanation for the
`parameterized` output. A refusal is an explanation and not a verdict, so a program that
produced a real value drops them: the checker evaluates speculatively, and a `$jit` whose
parameter domain is not decided yet records a refusal that a later attempt supersedes.
Recording is idempotent for an identical `(category, node, message)` — a node is deep-
evaluated several times, and refusing twice is not two findings.

## Extension point 5: owning a payload (`ValueExt`'s ext-handle contract)

A plugin value that carries **data** — as opposed to naming code or a marker — can
own it in the **block arena** rather than in a process registry, which is what
`lichen-compute`'s buffers do since `D15`:

```rust
impl ValueExt for ComputeValue {
    fn is_handle(&self) -> bool { matches!(self, ComputeValue::Buffer(..)) }
    fn handle(&self) -> AnyHandle<[u8]> { /* the `[i64]` payload as bytes */ }
    fn set_handle(&mut self, payload: AnyHandle<[u8]>) { /* re-view it */ }
    fn alignment() -> usize { std::mem::align_of::<i64>() }
}
```

The payload is allocated with `Module::alloc_payload` (the generic sibling of
`alloc_array`/`alloc_table`), so it lives in a block's bump arena and dies with
that block. The crate's copy path relocates it: a program-specific value is
routed to `copy_ext`, which consults `is_handle` — so **a leaf that owns a
payload must answer `true`**, or the handle survives the copy pointing into a
block that is about to be released. The composition does the dispatch, so a
composed vocabulary inherits this for free, and `alignment()` must report the
strictest alignment among the leaves because the freeze layout derives one
alignment for the whole vocabulary.

The value stays `Copy` — an `AnyHandle<T>` is `Copy` for any `T` — which is what
makes this cheaper than any owner carried in the value: `Copy` is a
vocabulary-wide trait bound (`ValueExt: Debug + Copy + PartialEq`), so a single
non-`Copy` variant would cost the whole lowlevel's value handling.

## Extension point 5b: keeping nodes alive (`ValueExt::traced`)

`is_handle`/`handle`/`set_handle` cover a value that owns **data**. A value that
keeps **module objects** alive needs a second, separate answer, and it is the
obligation no existing extension had because no existing extension had the
problem:

```rust
fn traced(&self, context: &dyn TraceContext, out: &mut Vec<NodeId>) {
    // nothing by default
}
```

A value appends the nodes it keeps. The GC walks each with the same
`garbage_collect_node` it uses for an array item, so **a value names nodes and
nothing else** — a function is kept alive by naming the node it is the value of,
and the walk's shape dispatch resolves it from there. `out` rather than a returned
slice because no real holder's references are one contiguous run: a compiled
graph interleaves them with kernel ids, counts and element data. `TraceContext`
rather than `&Module<P>` because everything worth looking at here — a node's
block, a block's node list, a function's scope — is spelled in lowlevel's own
types, so `P` stays off `ValueExt` and off the `ValueType` bounds above it.

**Why this is a seam at all.** The GC follows an array's items, a table's
entries, a function's scope, and an *unevaluated* node's operand. An operator's
result is cached, and a cached node's operand is deliberately not followed. So a
value holding a reference the GC cannot see loses it at the end of the very block
evaluation that produced it — `drop_block` deletes by block membership, not
reachability, so there is no diagnostic. A compiled graph holding the closures it
will call later is the first value in the tree to need this.

**It is not enforced.** Nothing checks the answer, because nothing can: the
lowlevel cannot see what a value holds. An unlisted node is not a detectable
omission, it is a node that quietly disappears. The contract is held by review
and by `lichen-lowlevel/tests/basic/compaction.rs`.

## Extension point 6: global extension state (`GlobalExt`)

A plugin can carry per-module, program-global state in the module's `global_ext` slot.
`GlobalExt` is a marker over a host struct whose components are composed with
`lichen_utils::compose_ext!` and read/mutated through `lichen_utils::compose::AsField` —
the highlevel's `HighGlobal` (the fresh nominal-type-id counter) is the example. A plugin
that needs no module-global state (like `lichen-compute`, whose kernel registry is
process-global `static`s) omits this.

`HighGlobalExt` is the highlevel's global extension state, injected into the lowlevel
`Module`'s `global_ext` slot and threaded through the extension operators. `compose_ext!`
composes several extensions into one *tuple* host struct — a host field is a bare extension
type, so there is no field name to collide — and generates `impl AsField<T>` per component;
the host is read or mutated per extension by pulling that extension out through `AsField`
and calling the extension's own methods, and the macro wires no per-extension accessor
trait. `get`/`get_mut` are ambiguous when a host has several components, so qualify with
`AsField::<T>::get(&host)`.

A downstream composes more state by listing the upstream's components **flat** (by symbol)
alongside its own — one `compose_ext!` invocation, no per-upstream wrapper. Flat is
load-bearing, not cosmetic: per-type `AsField` access resolves only against a flat tuple, and
the highlevel's own operators read `HighGlobal` that way (the fresh nominal-type-id counter
`Fresh` uses), so nesting the upstream host would break them. `next_type_id` is the one
stateful piece: it reads the counter, increments it, and returns the previous value, so each
call yields a distinct id.

`ProgramImpl`'s `Debug`/`Clone`/`Copy`/`PartialEq` are structural: its single `PhantomData`
field satisfies all four for *any* type argument, so the impls carry only the struct's own
bounds and never demand `G: Debug` / `G: Copy` — which keeps `GlobalExt` flexible, since it
only promises `Default`. The default `HighProgramOperator` is what the checked highlevel
builder emits, and a downstream that needs additional lowlevel operators composes its own
operator enum with `enum_ext!` for use as `Module<ProgramImpl<V, MyOperator, MyAttr>>`, after
which the runtime, static-module and registry machinery is reusable with the extended set.

## What a plugin looks like (the worked example: `lichen-compute`)

The whole plugin lives in the `lichen-compute` crate (`crates/lichen-compute/src/compute.rs`);
it is program-generic, so it never names a concrete host `Program`.  Its pieces:

- two `Copy` enums — `ComputeValue` (`Kernel`/`ParKernel`/`Buffer`),
  `ComputeOperator` (`Jit`/`Launch`/`Call`/`Parallel`/`ParLaunch`/`BufferGet`/`BufferCollect`);
- an `OperatorExt<P>` `run` impl (the wasm compile/execute, process-global kernel/buffer
  registries), bounded by the same associated-type constraints a host's `enum_ext!`
  vocabulary satisfies;
- several `NativeOp<P>` impls — `JitOp`, `LaunchOp`, `CallOp`, `ParallelOp`,
  `ParLaunchOp`, `BufferGetOp`, `BufferCollectOp` (the gates + typed result through `Ctx`);
- the embedded `compute.lichen` (the source that calls `$jit`/`$launch`/`$call`/…) copied as
  the virtual `compute.lichen` package;
- no `GlobalExt` (registry is process-global).

Then a host composes it: `lichen-language`'s `program.rs` composes
`ComputeValue`/`ComputeOperator` into `LangValue`/`LangOperator`, and `package.rs` builds the
plugin's private `NativeOps<LangProgram>` registry over `JitOp`/`LaunchOp` and registers the
`compute.lichen` import.

## Extension point 7: a compile-time attribute

A plugin can contribute an *attribute* — a marker (`AttrSpec`) plus its lowering
behaviour (`AttrExt<P>`: missing value, combine, unify, subtype, label, render),
listed in the host composition's `attrs` manifest. The checker then carries the
attribute as a compile-time `Schema` and materialises its slot at lowering; the
marker never names a slot number, because the manifest's order *is* the
canonical attribute order (see [attributes.md](attributes.md)).

## The shape of the contract

The contraction is that the core is **ignorant**: the lowlevel knows only the shape of
"a value/operator that can be composed and dispatched", the highlevel knows only the shape
of "a `$name(args)` call that should delegate to your registry", and a plugin supplies the
meaning. That is what makes the system extensible without a new kind system: a kernel is
typed like a function, a plugin's value is one variant of the value union, and the checker
adopts whatever pair the plugin returns.

`ValueType` is the value→type contract a value vocabulary must satisfy to flow through the
checker: the type-constant markers it installs, the value→type mapping for constants, and
the kind classification the structural type checks dispatch on. Every value union — the
highlevel's own `HighProgramValue` or an extended one — implements it, and the checker is
generic over it. The highlevel's 9 kind markers are closed, but the marker *concept* is
open: `LeafKindMarkers` is an extension value leaf's contribution to that open set, and the
composed `ValueType::is_kind_marker` consults it for each extra leaf; a leaf with no type
constants implements it returning `false` throughout. Every `ValueType` implementation spells
only `type_id` and `type_id_value`; the marker methods are registry-derived with default
bodies over `From<TypeValue>`.

## Costs and tradeoffs

- A plugin is **compile-time**: it adds variants to the `Value`/`Operator` enums, so the
  whole frontend recompiles; there is no dynamic loading.
- The native-op binding is **syntactic** (`$name`); it's private per module, so names are
  a plugin-local contract, not a global namespace.
- A host-owned scalar value (a `KernelId`) is a deliberate choice to stay out of the arena,
  so GC / static-freeze / `ValueExt` are untouched — at the cost that such values are
  runtime-only and not serializable into a shipped package.

## Recovered measurements

### Packaging a native plugin into a compiler

- A plugin-set build generates a crate under the compiler cache slot, and it is **bin-only**:
  `src/main.rs` holds the composition *and* `main`, because in a lib+bin package `crate::`
  refers to the *binary* crate, which would carry no composition — so
  `lichen_compiler::cli::main::<crate::LangProgram>()` (compiler) and
  `server::main::<crate::LangProgram>(&cache_root)` (server) would not resolve.
- The generated `Cargo.toml` names its dependencies by what is reachable from here: a **path**
  dep when `core_repo` is a directory on this machine, else a **git** dep naming the
  workspace member with `package = <crate_name>` (the repo root is a virtual workspace, so
  without it cargo looks for a package at the root and fails). Each plugin follows the same
  rule — path when its `url` exists locally, else git pinned to the resolved `rev`; the
  plugin's own core deps are git too, so cargo resolves that whole subtree from git.
- Against a **non-default** `core_repo` the manifest also emits `[patch.<canonical repo>]` for
  the core crates a native plugin git-deps. Without it cargo reaches the canonical repo over
  the network and, worse, the plugin's git core crates are **distinct crate instances** from
  the compositor's own, so the plugins' trait impls do not unify. Only the crates a plugin
  git-deps are patched, and the canonical repo itself needs no patch.
- A plugin-built compiler's `main` hands
  `lichen_compiler::cli::main_with_native_packages` one `(virtual_path, wrapper, ops)` tuple
  per plugin: `("<alias>.lichen", <crate_ident>::WRAPPER_SOURCE,
  <crate_ident>::<crate_ident>_ops!(crate::LangProgram))`. `<crate_ident>` is the Cargo.toml
  dependency key with `-` replaced by `_`, so a native plugin must export a `WRAPPER_SOURCE`
  const and a `<crate_ident>_ops!` macro. The wrapper is compiled on the *same store the
  program evaluates against* — exactly as the reference `std_native` test's `register_native`
  plug does — which is what makes the plugin's `$sort` resolve privately against its own
  registry.

### Extension point 1b: the literal vocabulary

- The same flat `enum_ext!` composition extends the *literal* vocabulary: a downstream
  composes the built-in int/type-constant literal structs with its own and implements
  `LiteralExt` for the composed enum, exactly as an operator vocabulary composes, and the
  checker runs on the result generically. `LiteralExt` is the operator-like value-extension
  point, and each built-in type-constant literal rebuilds its value/type nodes fresh per
  occurrence.
- A downstream literal may store nothing and build a kind marker paired with `Type` by
  emitting one value node through `Ctx`.

### Per-package metadata

- `HighPackageMeta` is the per-package metadata slot: the package layer stores the single
  exported `[value, type]` pair ref and the directly-exposed `(name, export)` bindings there,
  while the lowlevel's `Package` only carries it as an opaque `Default` slot, which keeps
  highlevel concepts out of the lowlevel registry machinery. The direct bindings are recorded
  so a *later* store over the same shared registry can rebuild the handle instead of
  recompiling the module — which the registry refuses, a key naming one artifact. A built-in
  additionally keeps its **own source**: the file it is exposed at and the position of every
  frozen node, so a runtime failure that names a cloned condition by `StaticNodeId` can be
  turned into a position in a file the user can open ([core-prelude](core-prelude.md)). An
  ordinary package caches as an opaque artifact and keeps no source.

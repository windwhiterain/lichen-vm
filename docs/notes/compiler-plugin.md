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

## Extension point 1: the value / operator vocabularies

The lowlevel ships the *structural* core: `LowValue` (`USize`/`Array`/`Table`/`Function`/
`None`/`Parameterized`) and `LowOperator` (`Index`/`Apply`/`TableGet`). A plugin composes
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

## Extension point 2: the native-call IR + private registry

This is the "the checker knows nothing about me" extension. A plugin's own embedded
source calls `$name(args)`; the frontend parses it to a general `ExprKind::NativeCall`
node; the checker delegates to the *current module's* private registry.

```rust
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

pub trait NativeOp<P> {
    fn build(&self, ctx: &mut dyn Ctx<P>, e: ExprId, args: &[NativeArg], loc: Loc) -> NativeApply;
}
pub struct NativeArg  { pub expr: ExprId, pub value: NodeId, pub ty: NodeId }
pub struct NativeApply { pub node: NodeId, pub val: Option<NodeId>, pub ty: NodeId }
```

- The args are **already compiled** (value/type wired), so `build` only checks the
  operator's types and emits the op node — through the curated `Ctx`, never raw lowlevel
  nodes.
- `Ctx` is the checker's encoding surface: `fresh` (a new unbound cell), `array_node`,
  `op_node`, `pair`, `kind_expr`, `universe`, the marker nodes, and `check_unify(_relaxed)`.
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
`Parameterized`) value — staying lazy on an unbound operand is the disciplined behaviour,
leaving the type-error reporting to the definition pass.

## Extension point 4: global extension state (`GlobalExt`)

A plugin can carry per-module, program-global state in the module's `global_ext` slot.
`GlobalExt` is a marker over a host struct whose components are composed with
`lichen_utils::compose_ext!` and read/mutated through `lichen_utils::compose::AsField` —
the highlevel's `HighGlobal` (the fresh nominal-type-id counter) is the example. A plugin
that needs no module-global state (like `lichen-compute`, whose kernel registry is
process-global `static`s) omits this.

## What a plugin looks like (the worked example: `lichen-compute`)

The whole plugin lives in the `lichen-compute` crate (`crates/lichen-compute/src/compute.rs`);
it is program-generic, so it never names a concrete host `Program`.  Its pieces:

- two `Copy` enums — `ComputeValue` (`Kernel`/`ParKernel`/`Buffer`/`TypeBuffer`),
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

## The shape of the contract

The contraction is that the core is **ignorant**: the lowlevel knows only the shape of
"a value/operator that can be composed and dispatched", the highlevel knows only the shape
of "a `$name(args)` call that should delegate to your registry", and a plugin supplies the
meaning. That is what makes the system extensible without a new kind system: a kernel is
typed like a function, a plugin's value is one variant of the value union, and the checker
adopts whatever pair the plugin returns.

## Costs and tradeoffs

- A plugin is **compile-time**: it adds variants to the `Value`/`Operator` enums, so the
  whole frontend recompiles; there is no dynamic loading.
- The native-op binding is **syntactic** (`$name`); it's private per module, so names are
  a plugin-local contract, not a global namespace.
- A host-owned scalar value (a `KernelId`) is a deliberate choice to stay out of the arena,
  so GC / static-freeze / `ValueExt` are untouched — at the cost that such values are
  runtime-only and not serializable into a shipped package.

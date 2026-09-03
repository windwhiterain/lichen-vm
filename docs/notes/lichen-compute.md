# lichen-compute: a native "compute" extension package

> Status: current (implemented 2026; see the module rustdoc for the live contract)
> Points at: `crates/lichen-language/src/compute.rs` (the native part),
> `crates/lichen-language/src/program.rs` (`LangValue`/`LangOperator` composition),
> `crates/lichen-highlevel/src/native.rs` (the `NativeExt` extension point),
> `crates/lichen-highlevel/src/checker.rs` (`check_app` hook), and
> `crates/lichen-language/src/package.rs` (the virtual `compute.lichen` package).

`lichen-compute` is lichen's version of a Python *native wrapper package* (think
`numpy`): a compiled native core plus a thin typed source surface. The native core
injects a new **value** `Kernel` and two **operators** `Jit`/`Launch`; the `.lichen` side
is a `compute.lichen` package that re-exports `jit`/`launch`/`Kernel`. Type checking is
the hard requirement, so a **kernel's type is its signature** — mirroring a function type
but with a `Kernel` kind head.

```
@{
  compute = import "compute.lichen"
@}
k = jit (x => x + 1)     -- k : Kernel<Int -> Int>   (compiles f to wasm)
launch k 5               -- 6 : Int                   (runs the kernel)
```

## 1. The vocabulary injection

The native part provides two plain enums, composed as sibling leaves into the language's
value/operator vocabularies with `lichen_utils::enum_ext!`:

- **`ComputeValue`**: `Kernel(KernelId)` (a compiled wasm artifact), `Native(Jit)` /
  `Native(Launch)` (first-class operator values), `TypeKernel` (the kind marker of a
  kernel type = `[signature, [TypeKernel, Type]]`), `TypeNativeJit`/`TypeNativeLaunch`/
  `TypeLaunchTarget` (the callee *type markers* that route an `Apply` to an operator),
  and `LaunchTarget { kernel, domain, codomain }` (the curried `launch` intermediate).
- **`ComputeOperator`**: `Jit` (function → Kernel) and `Launch` (`[kernel, arg]` → result),
  whose `OperatorExt::run` does the wasm compile/execute and a process-global kernel
  registry (`KERNELS`).

The language's `LangValue = LowValue + TypeValue + ComputeValue` and
`LangOperator = LowOperator + TypeOperator + GcdOp + ComputeOperator`; `LangProgram` is
`ProgramImpl<LangValue, LangOperator, Perspective>`.

## 2. Binding native operators to source (Option B)

`jit`/`launch` are **values whose type tags them**; `jit f` is an ordinary `Apply`. The
checker, in `check_app`, reads the callee's type and, when it is a native-operator type,
delegates to the matching `NativeExt` instead of a runtime function apply. The VM's
`LowOperator::Apply` never sees them.

```rust
// check_app (highlevel):
if let Some(native) = self.class_value(function_ty).and_then(|t| (self.native_ext)(&t)) {
    let r = native.check_apply(self, e, function_value, function_ty, argument_value, argument_ty, argument, span);
    self.term[e] = Some(r.node); self.val[e] = r.val; self.ty[e] = Some(r.ty);
    return r.node;
}
```

`native_ext` is a new registry (like `attr_ext`): `&P::Value -> Option<&'static dyn NativeExt<P>>`
— see `crates/lichen-highlevel/src/native.rs`. This is the one new core extension point.

## 3. The import bridge — why direct names, not just a tuple

The compute package exports a tuple `[Jit, Launch, Kernel]` (**and** the three items as
direct `[value,type]` pairs). The direct pairs are essential: `compute(0)` is a lazy `Index`
whose element type the checker cannot read statically, so it could never detect a native
callee at check time. Direct `[value,type]` pairs are statically known, so `jit`/`launch`/
`Kernel` are detectable wherever they're used. `PackageStore` serves `compute.lichen` from
a registered native module (no disk file); `import` binds `compute` *and* injects the three
direct names (`ResolvedImport::direct` → `compile_with_imports`).

## 4. Kernel type = signature

`compile` a kernel's type is `[signature, [TypeKernel, Type]]`, where `signature` is the
arrow `[domain, codomain]`. Everything the checker knows how to do with functions transfers:

- **`Jit f`** re-heads `f`'s arrow with `TypeKernel` (function-ness gate via a fresh
  function type + `check_unify`; scalar-v1 signature `Kernel<Int -> Int>`).
- **`launch k`** (stage 1) gates on kernel-ness (fresh kernel type + `check_unify`) and
  produces a `LaunchTarget(kernel, domain, codomain)` typed `TypeLaunchTarget`.
- **`(launch k) a`** (stage 2) unifies `a` against the kernel's domain and yields the
  kernel's codomain — a function-style apply over a kernel.

The operator's own runtime `run` enforces the scalar monomorphization (a body outside the
kernel-safe subset stays `Parameterized`).

## 5. Codegen + execution (wasm, wasmi)

`Jit::run` walks the function's body graph (`function.return` → its `[value,type]` pair's
value element, `function.parameter`) and lowers the scalar subset to a wasm module
`(func "main" (param i64) (result i64))` via `wasm-encoder`:
integer literals → `i64.const`; `Add`/`Sub`/`Leq`/`Eq` → the wasm arith/compare forms;
`Index(param_pair, 0)` (the parameter value) → `local.get 0`. `Launch::run` instantiates the
bytes with `wasmi` and calls `main`, marshalling `usize` ↔ `i64`.

## 6. Extension points in the existing code

**New (`lichen-language/src/compute.rs`):** `ComputeValue`, `ComputeOperator`, `NativeOp`,
`LaunchTarget`, `OperatorExt::run` (codegen + registry), the two `NativeExt`s, `native_registry`,
and `build_compute_module` (the virtual package's frozen module).

**Core — `lichen-highlevel/src/native.rs`:** `NativeExt` trait + `NativeApply` + `no_native_ext`.

**Core — `lichen-highlevel/src/checker.rs`:** the `native_ext` field/registry,
`build_in_attr_native`, the `check_app` hook, and public accessors
(`class_value`, `type_expr_node`, `int_type_node`, `value_node`, `fresh_cell`).

**Core — `lichen-highlevel/src/program.rs`:** `ProgramImpl` generic over `GlobalExt` (so a
program may carry a different per-module global state).

**Core — `lichen-language`:** `program.rs` (`LangValue`/`LangOperator` + `ValueType`),
`lib.rs` (`build_in_attr_native` + the native registry), `package.rs` (virtual `compute.lichen`,
`register_compute`), `preprocess/mod.rs` (`ResolvedImport::direct`), `compile.rs` (direct
name binding), `persist.rs` (compute arms), `render.rs` (renamed to `LangValue`),
`Cargo.toml` (`wasmi`, `wasm-encoder`).

**Tests:** `tests/compute.rs` (new) plus the `HighProgramValue` → `LangValue` rename in the
existing value-vocabulary tests.

## 7. Design decisions & tradeoffs

- **Kernel type = signature** — the single decision that makes type checking "covered":
  kernels reuse the arrow machinery, so function-style apply/gating transfer without a new
  kind system.
- **Kernel is host-owned** (a `Copy` `KernelId` into a process-global registry) — never an
  arena payload, so GC/static-freeze/`ValueExt` are unchanged; but a kernel is runtime-only,
  so a package ships source, not kernels (`persist::write_value` rejects compute values).
- **Direct names + tuple** — the direct pairs are what makes check-time native detection
  reliable; the tuple is retained for positional-`compute(0)`-style value access.
- **Dedicated `NativeExt` extension point** — mirrors `AttrExt`; the core stays generic with
  one consult point, and a program without native operators uses `no_native_ext()`.
- **Scalar-only v1** — the kernel-safe subset is `Int -> Int`; richer signatures are phase 2.

## 8. Non-goals (phase 2)

- Non-scalar kernel signatures (tuples/arrays) and a generic-kernel pinning rule.
- Higher-order kernels and recursion inside the compiled region.
- Launch-config type checking (grid/block dims, device).
- A `GlobalExt`-based `ComputeGlobal` (currently a process-global kernel registry).

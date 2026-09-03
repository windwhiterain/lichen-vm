# lichen-compute: native-call IR + private registration (plan for review)

Status: proposal, depends on the `Ctx` extraction refactor landing first.

## Goal

Rework `lichen-compute` so native operators (`jit`, `launch`, `Kernel`) are
exposed through a real lichen source "plugin" (`compute.lichen`) as ordinary
typed lichen functions, instead of being injected as native *value* operators
dispatched by callee type. The typechecker should not understand compute at all:
it only delegates a single general native-call IR node to the current module's
private plugin registry, and the plugin's own registration does the lowering and
type construction.

This removes the earlier design's `NativeExt` callee-type hook, the native
operator *values* (`Native(Jit)`/`Native(Launch)`), the `LaunchTarget` value, the
`TypeNative*` markers, and the direct-name binding machinery.

## Target architecture

### 1. General native-call IR (works for any plugin, not just JIT)

`crates/lichen-highlevel/src/ir.rs`: add

```rust
enum ExprKind<L> {
    // ...
    /// `$jit(f)` / `$launch(k, a)` — a call to a native operator registered
    /// by the compiling module's plugin.  Op is a private name, resolved only
    /// against that module's registry; args are the (already-checked) argument
    /// expressions.
    NativeCall { op: String, args: Vec<ExprId> },
}
```

Add it to the children traversal (its `args` are children).

### 2. Private, per-compilation registration

`crates/lichen-highlevel/src/native.rs`:

```rust
/// The result of a native op lowering: the compiled pair plus its value/type
/// halves (the same shape as the existing `NativeApply`).
pub struct NativeCall { pub node: NodeId, pub val: Option<NodeId>, pub ty: Option<NodeId> }

/// A native operator.  `build` is the plugin's private contract with its own
/// source: it emits the operator node and builds the typed result through the
/// curated `Ctx` (never raw lowlevel nodes).
pub trait NativeOp<P: HighProgram>
where P::Value: ValueType {
    fn build(&self, ctx: &mut dyn Ctx<P>, e: ExprId, args: &[ExprId], loc: Loc) -> NativeCall;
}

/// The registry attached to ONE module's checker.  Empty for a normal file.
/// Because a plugin's file is compiled on its own, this slice is naturally
/// private: `$jit` resolves only within this module's slice, so two plugins
/// each registering `$jit` never collide.
pub type NativeOps<P> = &'static [(&'static str, &'static dyn NativeOp<P>)];

pub fn no_native_ops<P: HighProgram>() -> NativeOps<P> where P::Value: ValueType { &[] }
```

`Checker::<P>` gains `native_ops: NativeOps<P>` (default `no_native_ops()`).

### 3. The checker delegates; it knows nothing about compute

`crates/lichen-highlevel/src/checker.rs`, in `check_term`:

```rust
ExprKind::NativeCall { op, args } => self.check_native_call(e, op, args),

fn check_native_call(&mut self, e, op: &str, args: &[ExprId]) -> NodeId {
    for arg in args { self.check_expr(*arg); }
    let built = self.native_ops.iter()
        .find(|(name, _)| *name == op)
        .map(|(_, op)| op.build(self, e, args, self.loc(e, 0)));
    let Some(built) = built else {
        // Unknown native op in this module — a resolve error, not a panic.
        // The frontend is the place to report it with a span; the checker
        // stays lazy (returns a fresh cell) and records the error elsewhere.
        unreachable!("native op must be validated by the frontend");
    };
    self.term[e] = Some(built.node);
    self.val[e] = built.val;
    self.ty[e] = built.ty;
    built.node
}
```

Notes:
- The checker never names `Jit`/`Launch`/`TypeKernel`/`sig`. It just adopts the
  pair the plugin hands back.
- The `check_app` callee-type hook is deleted entirely.

### 4. The plugin's builders do the lowering + typed result (through `Ctx`)

`crates/lichen-language/src/compute.rs`. Keep the wasm codegen, wasmi `run`,
and the process-global `KERNELS` registry. Replace the value-dispatch layer with
two static `NativeOp<LangProgram>` impls:

```rust
pub fn native_ops() -> NativeOps<LangProgram> {
    &[("jit", &JIT_OP), ("launch", &LAUNCH_OP)]
}

impl NativeOp<LangProgram> for JitOp {
    fn build(&self, ctx, e, args, loc) -> NativeCall {
        // args = [f]
        // 1. Function-ness gate: unify f's type with an arrow shape.
        // 2. sig = the arrow's [in, out] shape.
        // 3. result type = [sig, [TypeKernel, Type]].
        // 4. Emit ComputeOperator::Jit over f's value (Kernel value).
    }
}

impl NativeOp<LangProgram> for LaunchOp {
    fn build(&self, ctx, e, args, loc) -> NativeCall {
        // args = [k, a]
        // 1. Kernel-ness gate: k's type is [sig, [TypeKernel, Type]].
        // 2. domain = Index(Index(k_ty, 0), 0); codomain = Index(Index(k_ty, 0), 1).
        // 3. Unify a's type with domain; result type = codomain.
        // 4. Emit ComputeOperator::Launch over [k, a] value.
    }
}
```

`ComputeValue` shrinks to `Kernel(KernelId) | TypeKernel` (drop `Native`,
`TypeNative*`, `LaunchTarget`); `ComputeOperator` stays `Jit | Launch`.

### 5. `compute.lichen` is a real embedded source wrapper, served as a tuple

`crates/lichen-language/src/package.rs`, `register_compute()`:

- Compile the embedded wrapper source (a `&str` in `compute.rs`) with `compute::native_ops()`.
- The wrapper (real lichen source):

```lichen
jit(f)    = $jit(f)                     // : (F -> G) → Kernel
launch(k) = fn(a) { $launch(k, a) }     // : Kernel → F → G
compute   = (jit, launch)               // single exported value; positional
```

- Users import `compute` and call `(compute 0) (x => x + 1)` / `(compute 1) k 5`.
- The package still exports `compute` as a frozen `[value, type]` pair; the
  `direct`-name binding machinery in `PackageHandle`/`ResolvedImport`/`compile.rs`
  is removed (no longer needed since the elements are ordinary lichen functions).
- Every other file compiles with `no_native_ops()`; `$jit` there is a frontend
  resolve error.

### 6. Companion updates

- `crates/lichen-language/src/parse.rs` / `ast.rs`: lex/parse `$name(args)` →
  `ExprKind::NativeCall { op, args }`. `$` must be a new lexer token (check it
  is not already used). If the op name is not in the current module's private
  registry, produce a resolve diagnostic.
- `crates/lichen-language/src/compile.rs`: emit `NativeCall` from the parsed form
  (collected into the module; validate names against the module's registry).
- `crates/lichen-language/src/program.rs`: shrink `ComputeValue`; adjust
  `ValueExt::is_handle` etc.
- `crates/lichen-language/src/persist.rs`: adjust the `panic!` arms; a compute
  value/operator is still runtime-only and never frozen.
- `crates/lichen-highlevel/src/lib.rs`: export `NativeOp`, `NativeCall`,
  `NativeOps`, `no_native_ops`.
- Tests: `crates/lichen-language/tests/compute.rs` updated to the tuple-based
  API and check-time gate errors.

## Dependency on the `Ctx` interface

`NativeOp::build` goes through `&mut dyn Ctx<P>` (consistent with the already
refactored `AttrExt`/`NativeExt`). To lower jit/launch it needs:

- a **fresh type cell** (the arrow `[in, out]` for the gate; the kernel `sig`);
- **`op_node(P::Operator, operand)`** to emit `Jit`/`Launch`;
- **`array_node`/`pair`/`kind_expr`/`value_node`/marker nodes** to assemble
  `[sig, [TypeKernel, Type]]`;
- a **lazy `Index` type read** (`op_node(Index, …)`) to derive `sig.in`/`sig.out`
  from the kernel type for `launch`;
- **`check_unify`** for the gate (via the checker's own diary-attributed unify).

The current `Ctx` doc comment already names "a fresh cell, an op node, an array
shape" as the deliberate exceptions it exposes; I need those to be actual methods
(or a documented way to make a fresh cell). This is the main thing to confirm
against the final `Ctx` surface before I wire the builders.

## Open questions for the reviewer

1. Is exposing a fresh-cell mechanism (and a lazy `Index` type read) on `Ctx`
   acceptable, or should the native builder keep those at the checker boundary?
2. Do we drop the `Kernel` name from the v1 tuple (`compute = (jit, launch)`) —
   since jit/launch fully cover scalar usage — or expose it as a named type
   alias for annotations?
3. Should `$name` op resolution errors be reported by the frontend (needs the
   module's registry) or the checker? Leaning frontend, since it owns spans and
   the `$` form's desugaring.

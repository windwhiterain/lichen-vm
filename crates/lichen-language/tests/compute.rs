//! End-to-end tests for the `lichen-compute` extension: `jit` a function to a
//! kernel (wasm), `launch` it with an argument, and the kernel/codomain type
//! checks.  The plugin is exposed as a named struct namespace (`compute`):
//! `compute.jit` and `compute.launch`.

use lichen_language::package::PackageStore;

/// Compile and run `source` (resolving imports through a fresh store),
/// returning the rendered `value: type` output.
fn run(source: &str) -> String {
    let mut store = PackageStore::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .unwrap_or_else(|diags| panic!("expected {source:?} to check and run, got: {diags:?}"))
}

/// Compile `source` and assert it *fails*; return the rendered diagnostics.
fn fail(source: &str) -> Vec<String> {
    let mut store = PackageStore::new();
    let errs = lichen_language::run::evaluate_raw(source, None, &mut store)
        .expect_err("expected this program to fail");
    errs.into_iter().map(|d| d.message).collect()
}

#[test]
fn jit_then_launch_scalar() {
    // `compute.jit` is `jit` — compiles the lambda to a wasm kernel; `launch k 5`
    // runs it and yields `6`, typed `Int`.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (x => x + 1)
compute.launch k 5
"#);
    assert_eq!(out, "6: Int", "jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_op_signature() {
    // A body of several scalar operations: `x + 1 + 2`.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (x => x + 1 + 2)
compute.launch k 5
"#);
    assert_eq!(out, "8: Int", "multi-op jit+launch produced: {out:?}");
}

#[test]
fn jit_rejects_a_non_function() {
    // `jit` requires a function argument (the function-ness gate).
    let diags = fail(
        r#"
@{
  compute = import "compute.lichen"
@}
compute.jit 5
"#,
    );
    assert!(
        !diags.is_empty(),
        "jit 5 must be a type error, got diagnostics: {diags:?}"
    );
}

#[test]
fn launch_rejects_a_non_kernel() {
    // `launch` requires a kernel target (the kernel-ness gate).
    let diags = fail(
        r#"
@{
  compute = import "compute.lichen"
@}
compute.launch (x => x + 1) 5
"#,
    );
    assert!(
        !diags.is_empty(),
        "launch of a non-kernel must be a type error, got: {diags:?}"
    );
}

#[test]
fn jit_multi_arg_tuple() {
    // A tuple-domain kernel: `(p : (Int, Int) => p(0) + p(1))` compiles to
    // a wasm `(i64, i64) -> i64` and launches with a 2-tuple argument.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : (Int, Int) => p(0) + p(1))
compute.launch k (5, 3)
"#);
    assert_eq!(out, "8: Int", "tuple-domain jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_ternary() {
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : (Int, Int, Int) => p(0) + p(1) + p(2))
compute.launch k (5, 3, 2)
"#);
    assert_eq!(out, "10: Int", "ternary jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_sub() {
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : (Int, Int) => p(0) - p(1))
compute.launch k (10, 3)
"#);
    assert_eq!(out, "7: Int", "tuple subtraction produced: {out:?}");
}

#[test]
fn launch_rejects_wrong_arity() {
    // Launching a `(Int, Int) -> Int` kernel with a single scalar is a check
    // error: the `launch` gate unifies the argument against the domain
    // `(Int, Int)`, so a scalar `Int` fails.
    let diags = fail(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : (Int, Int) => p(0) + p(1))
compute.launch k 5
"#,
    );
    assert!(
        !diags.is_empty(),
        "launch of a 2-arg kernel with 1 arg must be a type error, got: {diags:?}"
    );
}

#[test]
fn jit_closes_over_constant() {
    // A kernel body may reference a module-level constant binding (non-function
    // values are graph-shared, so the body references the value node in place
    // and the JIT lowers it to `i64.const`).
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
a = 42
k = compute.jit (x => x + a)
compute.launch k 1
"#);
    assert_eq!(out, "43: Int", "closure-over-constant produced: {out:?}");
}

#[test]
fn jit_multi_arg_all_ops() {
    // A tuple-domain body mixing `+`, `-`, `<=` and a constant.
    // (5 + 3) - (5 <= 3) = 8 - 0 = 8.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : (Int, Int) => (p(0) + p(1)) - (p(0) <= p(1)))
compute.launch k (5, 3)
"#);
    assert_eq!(out, "8: Int", "mixed-op tuple produced: {out:?}");
}

#[test]
fn jit_conditional_then() {
    // `if x <= 3 then 10 else 20` lowers to `[20, 10][x <= 3]` — a 2-element
    // array index the JIT lowers to a wasm `select`.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 2
"#);
    assert_eq!(out, "10: Int", "conditional (then) produced: {out:?}");
}

#[test]
fn jit_conditional_else() {
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 5
"#);
    assert_eq!(out, "20: Int", "conditional (else) produced: {out:?}");
}

#[test]
fn jit_nested_tuple_domain() {
    // A nested tuple domain `((Int, Int), Int)`: the parameter flattens to
    // three wasm i64 locals, and `p(0)(0) + p(0)(1) + p(1)` reads them at
    // their flattened offsets (0, 1, 2).  Exercises recursive LowShape.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k = compute.jit (p : ((Int, Int), Int) => p(0)(0) + p(0)(1) + p(1))
compute.launch k ((2, 3), 4)
"#);
    assert_eq!(out, "9: Int", "nested tuple jit+launch produced: {out:?}");
}

#[test]
fn jit_cross_kernel_call() {
    // Style 2: `k1`'s body calls kernel `k0` (`k0 (x + 1)`).  Launch assembles
    // k1's *relative launch set* — k1 plus the kernel it cross-calls, k0 — into
    // one wasm module, so the cross-kernel call is an in-module `call`:
    //   launch k1 5 = k0(5 + 1) = k0(6) = 7.
    // The bare `k x` apply leaves a direct kernel apply's codomain `?a` (the
    // checker only resolves it via `$launch`), so the value is asserted.  The
    // wrapper form `compute.launch k0 (x + 1)` *does* give `Int` — covered by
    // `jit_cross_kernel_wrapper` below.
    let out = run(r#"
@{
  compute = import "compute.lichen"
@}
k0 = compute.jit (y => y + 1)
k1 = compute.jit (x => k0 (x + 1))
compute.launch k1 5
"#);
    assert!(
        out.starts_with("7:"),
        "cross-kernel call produced 7, got: {out:?}"
    );
}

#[test]
fn jit_cross_kernel_subexpr() {
    // A cross-kernel call result used as a sub-expression: `k0 (x) + 1`.  The
    // checker peels the call result via `Index(apply, 0)` (a `value_of`
    // extraction), which the JIT now looks through to emit the kernel call
    // directly:   launch k1 5 = k0(5) + 1 = 6 + 1 = 7.
    let out = run(r#"
@{ compute = import "compute.lichen" @}
k0 = compute.jit (y => y + 1)
k1 = compute.jit (x => k0 (x) + 1)
compute.launch k1 5
"#);
    assert!(out.starts_with("7:"), "subexpr produced: {out:?}");
}

#[test]
fn jit_inline_lichen_function() {
    // Style 1: a lichen-function call in a kernel body.  The checker's deep
    // pass already *reduces* the same-module call (`helper x` → `x + 2`), so
    // the JIT traces the reduced graph; a substituted parameter cell resolves
    // to the enclosing kernel's parameter (via its unified equality class) and
    // becomes a `local.get`.  `helper x + 1` → `(x + 2) + 1`:
    //   launch k 5 = (5 + 2) + 1 = 8.
    let out = run(r#"
@{ compute = import "compute.lichen" @}
helper = y => y + 2
k = compute.jit (x => helper x + 1)
compute.launch k 5
"#);
    assert!(out.starts_with("8:"), "inline produced: {out:?}");
}

#[test]
fn jit_cross_kernel_wrapper() {
    // Style 3: the wrapper/`$launch` form `compute.launch k0 (x + 1)` inside a
    // kernel body.  `launch = k => a => $launch(k, a)` is a *two-step* native
    // (assemble the module, then call it), so its argument is a run-time value
    // and arrives as a `Parameterized` cell at codegen time.  The cell is
    // unified with the defining `x + 1` computation, and the JIT emits that
    // through the cell's equality class:  launch k1 5 = k0(5 + 1) = 7.
    // Unlike the bare `k x` apply, the wrapper's result is typed `Int`.
    let out = run(r#"
@{ compute = import "compute.lichen" @}
k0 = compute.jit (y => y + 1)
k1 = compute.jit (x => compute.launch k0 (x + 1))
compute.launch k1 5
"#);
    assert!(out.starts_with("7:"), "wrapper produced: {out:?}");
}

#[test]
fn jit_inline_nested_function() {
    // Nested inline: the deep pass reduces `b x` (which calls `a`) through to
    // the leaf arithmetic, so `b x + 1` → `(x + 1) + 1 + 1`:
    //   a = y => y + 1;  b = y => a y + 1;  launch k 5 = (((5 + 1) + 1) + 1) = 8.
    let out = run(r#"
@{ compute = import "compute.lichen" @}
a = y => y + 1
b = y => a y + 1
k = compute.jit (x => b x + 1)
compute.launch k 5
"#);
    assert!(out.starts_with("8:"), "nested inline produced: {out:?}");
}

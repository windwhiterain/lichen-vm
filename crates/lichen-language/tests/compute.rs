//! End-to-end tests for the `lichen-compute` extension: `jit` a function to a
//! kernel (wasm), `launch` it with an argument, and the kernel/codomain type
//! checks.  The plugin is exposed as a positional tuple namespace (`compute`):
//! `compute(0)` is `jit`, `compute(1)` is `launch`.

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
    // `compute(0)` is `jit` — compiles the lambda to a wasm kernel; `launch k 5`
    // runs it and yields `6`, typed `Int`.
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (x => x + 1)
compute(1) k 5
"#,
    );
    assert_eq!(out, "6: Int", "jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_op_signature() {
    // A body of several scalar operations: `x + 1 + 2`.
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (x => x + 1 + 2)
compute(1) k 5
"#,
    );
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
compute(0) 5
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
compute(1) (x => x + 1) 5
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
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (p : (Int, Int) => p(0) + p(1))
compute(1) k (5, 3)
"#,
    );
    assert_eq!(out, "8: Int", "tuple-domain jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_ternary() {
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (p : (Int, Int, Int) => p(0) + p(1) + p(2))
compute(1) k (5, 3, 2)
"#,
    );
    assert_eq!(out, "10: Int", "ternary jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_sub() {
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (p : (Int, Int) => p(0) - p(1))
compute(1) k (10, 3)
"#,
    );
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
k = compute(0) (p : (Int, Int) => p(0) + p(1))
compute(1) k 5
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
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
a = 42
k = compute(0) (x => x + a)
compute(1) k 1
"#,
    );
    assert_eq!(out, "43: Int", "closure-over-constant produced: {out:?}");
}

#[test]
fn jit_multi_arg_all_ops() {
    // A tuple-domain body mixing `+`, `-`, `<=` and a constant.
    // (5 + 3) - (5 <= 3) = 8 - 0 = 8.
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (p : (Int, Int) => (p(0) + p(1)) - (p(0) <= p(1)))
compute(1) k (5, 3)
"#,
    );
    assert_eq!(out, "8: Int", "mixed-op tuple produced: {out:?}");
}

#[test]
fn jit_conditional_then() {
    // `if x <= 3 then 10 else 20` lowers to `[20, 10][x <= 3]` — a 2-element
    // array index the JIT lowers to a wasm `select`.
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (x => if x <= 3 then 10 else 20)
compute(1) k 2
"#,
    );
    assert_eq!(out, "10: Int", "conditional (then) produced: {out:?}");
}

#[test]
fn jit_conditional_else() {
    let out = run(
        r#"
@{
  compute = import "compute.lichen"
@}
k = compute(0) (x => if x <= 3 then 10 else 20)
compute(1) k 5
"#,
    );
    assert_eq!(out, "20: Int", "conditional (else) produced: {out:?}");
}

//! End-to-end tests for the `lichen-compute` extension: `jit` a function to a
//! kernel (wasm), `launch` it with an argument, and the kernel/codomain type
//! checks.  The plugin is exposed as a named struct namespace (`compute`):
//! `compute.jit` and `compute.launch`.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;

/// Compile and run `source` (resolving imports through a fresh store),
/// returning the rendered `value: type` output.
fn run(source: &str) -> String {
    let mut store = PackageStore::<LangProgram>::new();
    lichen_language::run::evaluate_raw(source, None, &mut store)
        .unwrap_or_else(|diags| panic!("expected {source:?} to check and run, got: {diags:?}"))
}

/// Compile `source` and assert it *fails*; return the rendered diagnostics.
fn fail(source: &str) -> Vec<String> {
    let mut store = PackageStore::<LangProgram>::new();
    let errs = lichen_language::run::evaluate_raw(source, None, &mut store)
        .expect_err("expected this program to fail");
    errs.into_iter().map(|d| d.message).collect()
}

#[test]
fn parallel_rejects_an_unknown_backend_name() {
    // The language has no enum type yet, so the backend is a string and the
    // parse is strict: a typo is reported with what was written and what is
    // accepted, rather than defaulted to a backend the author did not name.
    let diags = fail(
        r#"
---
  compute = import "compute.lichen"
---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + i]
}
k = compute.parallel f "Gpu"
out = compute.plrun k (4,)
compute.read [out, 0]
"#,
    );
    let all: Vec<&str> = diags.iter().map(String::as_str).collect();
    let joined = all.join(" | ");
    assert!(
        all.iter().any(|message| message.contains("\"Gpu\""))
            && all.iter().any(|message| message.contains("\"cpu\""))
            && all.iter().any(|message| message.contains("\"gpu\"")),
        "the refusal quotes the value and the accepted backends: {joined}"
    );
}

#[test]
fn parallel_rejects_a_non_string_backend() {
    // The static gate catches the *shape*; which strings are backends is the
    // runtime parse's authority. A number is not a backend name.
    let diags = fail(
        r#"
---
  compute = import "compute.lichen"
---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + i]
}
k = compute.parallel f 4
out = compute.plrun k (4,)
compute.read [out, 0]
"#,
    );
    assert!(
        !diags.is_empty(),
        "a non-string backend is refused: {diags:?}"
    );
}

#[test]
fn jit_then_launch_scalar() {
    // `compute.jit` is `jit` — compiles the lambda to a wasm kernel; `launch k 5`
    // runs it and yields `6`, typed `Int`.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => x + 1)
compute.launch k 5
"#);
    assert_eq!(out, "6: Int", "jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_op_signature() {
    // A body of several scalar operations: `x + 1 + 2`.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
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
---
  compute = import "compute.lichen"
---
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
---
  compute = import "compute.lichen"
---
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
    // A tuple-domain kernel: `(p : <Int, Int> => p(0) + p(1))` compiles to
    // a wasm `(i64, i64) -> i64` and launches with a 2-tuple argument.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
compute.launch k (5, 3)
"#);
    assert_eq!(out, "8: Int", "tuple-domain jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_ternary() {
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int, Int> => p(0) + p(1) + p(2))
compute.launch k (5, 3, 2)
"#);
    assert_eq!(out, "10: Int", "ternary jit+launch produced: {out:?}");
}

#[test]
fn jit_multi_arg_sub() {
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => p(0) - p(1))
compute.launch k (10, 3)
"#);
    assert_eq!(out, "7: Int", "tuple subtraction produced: {out:?}");
}

#[test]
fn launch_rejects_wrong_arity() {
    // Launching a `<Int, Int> -> Int` kernel with a single scalar is a check
    // error: the `launch` gate unifies the argument against the domain
    // `<Int, Int>`, so a scalar `Int` fails.
    let diags = fail(
        r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
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
---
  compute = import "compute.lichen"
---
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
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => (p(0) + p(1)) - (p(0) <= p(1)))
compute.launch k (5, 3)
"#);
    assert_eq!(out, "8: Int", "mixed-op tuple produced: {out:?}");
}

#[test]
fn jit_lowers_the_arithmetic_comparison_and_bitwise_operators() {
    // The operators added for real algorithms, through the **wasm** backend —
    // one kernel per operator, so each is on the lowering path rather than
    // hidden behind another's result: a product, an unsigned division and
    // remainder, both directions of the order comparison that were missing,
    // inequality, and the bitwise pair that is how a `0`/`1` result is combined.
    //
    // `k1` is the whole arithmetic group in one body: ((4 * 3) % 7) + (4 / 2) +
    // ((4 < 5) & (4 > 1)) = 5 + 2 + 1 = 8.
    //
    // The tuple is written on one line on purpose: a comma *and* a newline
    // between two elements is **two** separators, which the tuple grammar does
    // not tolerate (a pre-existing wart, unrelated to these operators — the
    // same program fails on `dev`), so this test does not depend on it.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k1 = compute.jit (x => ((x * 3) % 7) + (x / 2) + ((x < 5) & (x > 1)))
k2 = compute.jit (p : <Int, Int> => p(0) > p(1))
k3 = compute.jit (p : <Int, Int> => p(0) >= p(1))
k4 = compute.jit (p : <Int, Int> => p(0) != p(1))
k5 = compute.jit (p : <Int, Int> => (p(0) < p(1)) | (p(0) == p(1)))
k6 = compute.jit (p : <Int, Int> => (p(0) > p(1)) ^ (p(0) == p(1)))
(compute.launch k1 4, compute.launch k2 (5, 3), compute.launch k3 (3, 3), compute.launch k4 (5, 3), compute.launch k5 (5, 5), compute.launch k6 (5, 5))
"#);
    assert_eq!(
        out, "(8, 1, 1, 1, 1, 1): <Int, Int, Int, Int, Int, Int>",
        "the new operators jitted produced: {out:?}"
    );
}

#[test]
fn jit_conditional_then() {
    // `if x <= 3 then 10 else 20` lowers to `[20, 10][x <= 3]` — a 2-element
    // array index the JIT lowers to a wasm `select`.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 2
"#);
    assert_eq!(out, "10: Int", "conditional (then) produced: {out:?}");
}

#[test]
fn jit_conditional_else() {
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 5
"#);
    assert_eq!(out, "20: Int", "conditional (else) produced: {out:?}");
}

#[test]
fn jit_nested_tuple_domain() {
    // A nested tuple domain `<<Int, Int>, Int>`: the parameter flattens to
    // three wasm i64 locals, and `p(0)(0) + p(0)(1) + p(1)` reads them at
    // their flattened offsets (0, 1, 2).  Exercises recursive LowShape.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <<Int, Int>, Int> => p(0)(0) + p(0)(1) + p(1))
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
---
  compute = import "compute.lichen"
---
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
--- compute = import "compute.lichen" ---
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
--- compute = import "compute.lichen" ---
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
--- compute = import "compute.lichen" ---
k0 = compute.jit (y => y + 1)
k1 = compute.jit (x => compute.launch k0 (x + 1))
compute.launch k1 5
"#);
    assert!(out.starts_with("7:"), "wrapper produced: {out:?}");
}

#[test]
fn jit_cross_kernel_tuple_argument() {
    // A tuple-domain callee: `k0` is `(i64, i64) -> i64`, so the call pushes
    // one i64 per element of the callee's domain.  The argument here is a
    // concrete tuple *value*, so its elements are emitted one at a time:
    //   launch k1 5 = k0(5, 1) = 5 + 1 = 6.
    // The caller's own `x` is annotated: a bare kernel apply states no
    // signature, so nothing in the body decides `x` (the wrapper `launch` of
    // the test below does, through `.sig`).
    let out = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) + p(1))
k1 = compute.jit (x : Int => k0 (x, 1))
compute.launch k1 5
"#);
    assert!(
        out.starts_with("6:"),
        "tuple-argument cross-kernel call produced 6, got: {out:?}"
    );
}

#[test]
fn jit_cross_kernel_passes_the_parameter_through() {
    // The argument is a whole-parameter read, so it is passed through as the
    // parameter's own locals rather than materialized element by element — the
    // caller's `q` *is* the callee's `p`:
    //   launch k1 (9, 4) = k0(9, 4) = 9 - 4 = 5.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) - p(1))
k1 = compute.jit (q : <Int, Int> => k0 q)
compute.launch k1 (9, 4)
"#);
    assert!(
        out.starts_with("5:"),
        "parameter pass-through produced 5, got: {out:?}"
    );
}

#[test]
fn jit_cross_kernel_passes_a_sub_tuple_through() {
    // A *sub*-tuple read, against a nested callee domain.  The read's leaves
    // are contiguous in the flattened layout, so `r(1)` starts at local 1 and
    // the callee's three arguments are locals 1, 2 and 3:
    //   launch k1 (100, ((9, 4), 5)) = k0((9, 4), 5) = 9 - 4 + 5 = 10.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <<Int, Int>, Int> => p(0)(0) - p(0)(1) + p(1))
k1 = compute.jit (r : <Int, <<Int, Int>, Int>> => k0 r(1))
compute.launch k1 (100, ((9, 4), 5))
"#);
    assert!(
        out.starts_with("10:"),
        "sub-tuple pass-through produced 10, got: {out:?}"
    );
}

#[test]
fn jit_cross_kernel_tuple_argument_through_the_wrapper() {
    // Style 3 with a tuple argument: the wrapper's `launch` argument is a bare
    // `Parameterized` cell — concrete only at run time — so the tuple is
    // reached through the cell's equality class rather than as an array value:
    //   launch k1 5 = k0(5, 1) = 6.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) + p(1))
k1 = compute.jit (x => compute.launch k0 (x, 1))
compute.launch k1 5
"#);
    assert!(
        out.starts_with("6:"),
        "wrapper tuple-argument launch produced 6, got: {out:?}"
    );
}

#[test]
fn jit_inline_nested_function() {
    // Nested inline: the deep pass reduces `b x` (which calls `a`) through to
    // the leaf arithmetic, so `b x + 1` → `(x + 1) + 1 + 1`:
    //   a = y => y + 1;  b = y => a y + 1;  launch k 5 = (((5 + 1) + 1) + 1) = 8.
    let out = run(r#"
--- compute = import "compute.lichen" ---
a = y => y + 1
b = y => a y + 1
k = compute.jit (x => b x + 1)
compute.launch k 5
"#);
    assert!(out.starts_with("8:"), "nested inline produced: {out:?}");
}

// The two `.sig`-field assertions below are the acceptance test for an open
// defect: with `type_of` a standard-library function (rather than the removed
// builtin), a type read written in a struct field's *type* position resolves to
// the enclosing struct kind, so `.sig` renders `TypeStruct` instead of the
// signature.  Reproducer, mechanism and what was ruled out:
// `docs/notes/type-of-in-std.md` § "Open defect".  Remove the `ignore` when the
// checker fix lands.
#[test]
#[ignore = "open defect: a library type read in a field-type position resolves to \
            the enclosing struct kind — see docs/notes/type-of-in-std.md"]
fn a_kernel_value_and_type_render_by_name() {
    // A `jit` result's value is a kernel struct `[.native, .sig]`: the `.native`
    // artifact renders by name (via the compute vocabulary hook), the `.sig`
    // field carries the signature, so the type renders as the struct
    // `struct<.native <_>, .sig Int -> Int>`.  Dropping `TypeKernel` means no
    // renderer special-case — the struct's own fields carry the signature.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (y => y + y)
k
"#);
    assert_eq!(
        out, "(Kernel, parameterized): struct<.native raw[?a, ?b], .sig Int -> Int>",
        "kernel value/type: {out:?}"
    );
}

#[test]
fn jit_tuple_codomain_returns_several_values() {
    // The mirror of the multi-arity *domain*: a tuple codomain returns several
    // values at once.  The body is one wasm stack slot per leaf, so the
    // function is `(i64, i64) -> (i64, i64)` and the launch yields the tuple of
    // them — the two facts are the same count, read from the body at compile
    // time and from the run at launch time.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0), p(1)))
compute.launch k (5, 3)
"#);
    assert_eq!(
        out, "(5, 3): <Int, Int>",
        "tuple-codomain jit+launch produced: {out:?}"
    );
}

#[test]
fn jit_tuple_codomain_computes_each_leaf() {
    // Each leaf is its own scalar body, not a copy of the tuple: the first is
    // the identity and the second sums the domain, so a leaf that were emitted
    // as the wrong expression — or read from the wrong stack slot — would show
    // here.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0), p(0) + p(1)))
compute.launch k (5, 3)
"#);
    assert_eq!(
        out, "(5, 8): <Int, Int>",
        "per-leaf tuple codomain produced: {out:?}"
    );
}

#[test]
fn jit_tuple_codomain_elements_are_indexable() {
    // The tuple the launch returns is an ordinary lichen array value, so a
    // downstream read addresses a leaf by position with no special case.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0) + 10, p(1) + 100))
r = compute.launch k (2, 3)
(r(0), r(1))
"#);
    assert_eq!(
        out, "(12, 103): <Int, Int>",
        "indexing a returned tuple produced: {out:?}"
    );
}

#[test]
fn jit_three_value_codomain_returns_three_values() {
    // Three leaves, so the function is `(i64) -> (i64, i64, i64)` — a distinct
    // wasm signature from the two-value one, which is what forces the assembler
    // to key its type index on the (parameter arity, result arity) *pair*.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x : Int => (x, x + 1, x + 2))
compute.launch k 7
"#);
    assert_eq!(
        out, "(7, 8, 9): <Int, Int, Int>",
        "three-value tuple codomain produced: {out:?}"
    );
}

#[test]
fn jit_tuple_codomain_launches_through_the_cross_kernel_wrapper() {
    // The `compute.call k a` form reaches the same path (`run_kernel`) as
    // `launch`, so the multi-value result is a property of the *run*, not of the
    // one operator that usually spells it.  It renders as an array rather than
    // a tuple because `call` is the **untyped** form: `CallOp` types its result
    // as a fresh codomain cell (the callee's signature is read at assembly time,
    // not by the gate), so the type is undecided here — the same fact
    // `jit_cross_kernel_call` pins for the single-value case.  With no type to
    // read the value against, the result is a raw dump, marked `raw[…]`.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0) - p(1), p(0) + p(1)))
compute.call k (10, 4)
"#);
    assert_eq!(
        out, "raw[6, 14]: ?a",
        "compute.call on a tuple-codomain kernel produced: {out:?}"
    );
}

#[test]
fn jit_refuses_a_cross_kernel_call_to_a_multi_value_kernel() {
    // A callee that returns several values cannot be read as a *single* value
    // by a caller's body: the wasm `call` pushes one value per result, where
    // the body expects one.  So it is refused **by name** — naming the callee
    // and its result arity — rather than silently truncated to the first result
    // (which would answer `p(0)` here and look like it worked).
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => (p(0), p(1)))
k1 = compute.jit (q : <Int, Int> => compute.launch k0 q)
compute.launch k1 (5, 3)
"#,
    );
    assert!(
        !messages.is_empty(),
        "calling a multi-value kernel must be refused, got: {messages:?}"
    );
    let message = messages.join("; ");
    assert!(
        message.contains("more than one value"),
        "the refusal must name its own cause: {message:?}"
    );
    assert!(
        message.contains("compute.jit"),
        "the refusal must name the diagnostic it was recorded under: {message:?}"
    );
}

#[test]
#[ignore = "open defect: a library type read in a field-type position resolves to \
            the enclosing struct kind — see docs/notes/type-of-in-std.md"]
fn a_tuple_domain_kernel_type_renders_as_a_function() {
    // A tuple-domain kernel's signature is `[<Int, Int>, Int]`; the struct's
    // `.sig` field carries it, so the type renders as the struct
    // `struct<.native <_>, .sig <Int, Int> -> Int>`.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
k
"#);
    assert_eq!(
        out, "(Kernel, parameterized): struct<.native raw[?a, ?b], .sig <Int, Int> -> Int>",
        "tuple-domain kernel value/type: {out:?}"
    );
}

#[test]
fn wrapper_functions_render_with_named_type_variables() {
    // `jit` is a generic wrapper from the frozen `compute` module — its
    // domain/codomain cells are unbound at the module level, so they render as
    // *named cells* (`?a`/`?b`), and a `jit` result is a kernel struct whose
    // `.sig` field is that signature.  The wrapper itself stays generic; only
    // an *applied* result resolves to `Int -> Int`.
    assert_eq!(
        run(r#"
--- compute = import "compute.lichen" ---
compute.jit
"#),
        "Function: ?a -> ?b -> struct<.native raw[?c, ?d], .sig ?a -> ?b>",
        "jit wrapper value/type"
    );
    // `launch` reads the kernel's `.sig` lazily and returns its codomain, so it
    // stays a generic `? -> ? -> ?` (the codomain is an unbound cell at the
    // module level) — the concrete codomain only resolves when applied.
    assert_eq!(
        run(r#"
--- compute = import "compute.lichen" ---
compute.launch
"#),
        "Function: ?a -> ?b -> ?c",
        "launch wrapper value/type"
    );
}

#[test]
fn parallel_range_write_is_map() {
    // `compute.range n` yields the loop index `i ∈ [0, n)`; the index function
    // writes `i + i` into the output buffer at index `i`.  `plrun k cfg` runs
    // over `[0, cfg(0))` (the count is fixed at cfg position 0) and returns the
    // output buffer.  `out = [0, 2, 4, 6]`; `read [out, 2] = 4`.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + i]
}
k = compute.parallel f "cpu"
out = compute.plrun k (4,)
compute.read [out, 2]
"#);
    assert_eq!(out, "4: ?a", "parallel range/write map produced: {out:?}");
    // The element type renders as an unbound cell, and that is the honest
    // answer now: a buffer's element class is a fact of the *value*, so
    // `compute.read` no longer pins its result to `Int` — which is exactly the
    // pin that made a `buffer<Float>` inexpressible
    // (`docs/notes/floating-point.md` §3.7, §4.2).  The *value* is the map's,
    // unchanged from the `i + i` the kernel computed; only the static class of
    // a value that came back from a buffer is unresolved.
}

#[test]
fn parallel_read_input_buffer() {
    // A first kernel writes a buffer `[10, 11, 12]`; a second kernel reads it
    // (`cfg(1)(0)`, the input buffer tuple at cfg position 1) and doubles it.
    //   f2: out[i] = f1.out[i] + f1.out[i] = (i + 10) + (i + 10).
    let out = run(r#"
---
  compute = import "compute.lichen"
---
f1 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}
k1 = compute.parallel f1 "cpu"
inbuf = compute.plrun k1 (3,)
f2 = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  compute.write [n, i, a + a]
}
k2 = compute.parallel f2 "cpu"
out = compute.plrun k2 (3, (inbuf,))
compute.read [out, 1]
"#);
    assert!(
        out.starts_with("22:"),
        "parallel read/write produced: {out:?}"
    );
}

#[test]
fn parallel_write_only_collects_whole_buffer() {
    // `compute.collect out` materialises the whole output buffer into an array.
    let out = run(r#"
---
  compute = import "compute.lichen"
---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 1]
}
k = compute.parallel f "cpu"
out = compute.plrun k (3,)
compute.collect out
"#);
    assert!(
        out.starts_with("[1, 2, 3]:"),
        "parallel collect produced: {out:?}"
    );
}

#[test]
fn a_refused_plrun_count_says_why() {
    // `P1-30`: `plrun`'s element count is bounded (the count sizes the buffer
    // and the interpreted work), and a count past the bound is refused rather
    // than truncated.  The refusal must *say so*: the alternative is the lazy
    // marker alone, which prints `parameterized: Int` and tells the user
    // nothing about a number they can lower.  The count is the only problem
    // here, so the message has to name it.
    let messages = fail(
        r#"
---
  compute = import "compute.lichen"
---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + i]
}
k = compute.parallel f "cpu"
out = compute.plrun k (2000000,)
compute.read [out, 2]
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic, not one per evaluation: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("2000000") && message.contains("1048576"),
        "the refusal must name the count asked for and the limit: {message:?}"
    );
}

#[test]
fn a_refused_call_argument_element_says_why() {
    // `call` gates its argument against a *fresh* domain cell, so a tuple whose
    // element is a string passes the checker and is refused at run time, where
    // it used to print `parameterized: ?a` and say nothing at all.  The
    // element is a **nested** one on purpose: a tuple-of-tuples argument has no
    // other way to be pointed at than a path into it, which is what the
    // message must name.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x => x + 1)
compute.call k (1, (2, "three"))
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.starts_with("compute.kernel_launch:"),
        "the refusal carries the launch path's own category: {message:?}"
    );
    assert!(
        message.contains("argument element 1.1")
            && message.contains("a string")
            && message.contains("not a concrete Int"),
        "the refusal must name the element, what it is, and what it had to be: {message:?}"
    );
}

#[test]
fn a_refused_call_argument_shape_says_why() {
    // The same arm's other refusal: the argument is not a parameter vector at
    // all.  A bare string type-checks (the domain is a fresh cell) and reached
    // the run as `parameterized`, so the message has to say what a launch
    // argument must be and what this one was.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x => x + 1)
compute.call k "hello"
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("must be a concrete Int or a tuple of them")
            && message.contains("a string"),
        "the refusal must name the required shape and the value that broke it: {message:?}"
    );
}

#[test]
fn a_refused_call_run_says_why() {
    // The run itself refusing: `call`'s unchecked arity lets three arguments
    // reach a two-parameter kernel, so this message is the **only** account of
    // the mistake — `CallOp` gates the argument against a fresh cell and lets
    // the count through on purpose.  Compute holds both numbers (the callee's
    // domain leaves, and the vector built from the argument), so the refusal
    // has to state them rather than report that the count was wrong.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
compute.call k (1, 2, 3)
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("takes 2 arguments") && message.contains("supplied 3"),
        "the refusal must name the callee's arity and the count supplied: {message:?}"
    );
    assert!(
        message.contains("callee kernel"),
        "the refusal must name the callee it is about: {message:?}"
    );
}

#[test]
fn parallel_multi_output_writes_every_output_in_one_pass() {
    // The index function's codomain is a **tuple of writes**, so one `plrun`
    // produces two output buffers: write `k` of the body is output buffer `k`
    // (`out(k)`), and both come out of the single pass over the indices.
    let out = run(r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i], compute.write [n, i, i + i])
}
k = compute.parallel f "cpu"
outs = compute.plrun k (3,)
(compute.read [outs(0), 2], compute.read [outs(1), 2])
"#);
    assert_eq!(
        out, "(2, 4): <?a, ?b>",
        "multi-output parallel map produced: {out:?}"
    );
}

#[test]
fn parallel_multi_output_collects_each_output() {
    // Three outputs, one of which reads an input buffer: `collect` materialises
    // one output buffer whole, which is the point of a multi-output kernel — a
    // single pass emitting several result columns.
    let out = run(r#"
--- compute = import "compute.lichen" ---
f1 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}
k1 = compute.parallel f1 "cpu"
inbuf = compute.plrun k1 (3,)
f2 = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  (compute.write [n, i, a], compute.write [n, i, a + a], compute.write [n, i, i])
}
k2 = compute.parallel f2 "cpu"
outs = compute.plrun k2 (3, (inbuf,))
compute.collect outs(1)
"#);
    assert!(
        out.starts_with("[20, 22, 24]:"),
        "collecting the second output produced: {out:?}"
    );
}

#[test]
fn a_multi_output_parallel_run_is_identical_sequential_and_parallel() {
    // A parallel run **partitions** the index range over worker threads: each
    // worker owns a disjoint span of every output buffer and the `write` import
    // rebases the global index by the worker's base.  The consequence to pin is
    // that the partition is invisible in the result — the same kernel over a
    // count *below* `SEQUENTIAL_PARALLEL_ELEMENTS` (one worker, the calling
    // thread) and over a count *above* it (every worker) must agree element for
    // element.
    //
    // The count decides which regime a run gets (on a machine with a single
    // available processor both counts are the same one-worker run), so this pins
    // both sides of that one rule: the collected prefix of the big run must be
    // exactly the small run's whole result, and the big run's tail must be the
    // same arithmetic.  (Which regime a *given* count is in is
    // `lichen-compute`'s `parallel_worker_count`, unit-tested there; no
    // lichen-level value can distinguish the two, which is the point.)
    let small = run(r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i + 3], compute.write [n, i, i + i])
}
k = compute.parallel f "cpu"
outs = compute.plrun k (4,)
(compute.collect outs(0), compute.collect outs(1))
"#);
    assert_eq!(
        small, "([3, 4, 5, 6], [0, 2, 4, 6]): <array<?a, ?b>, array<?c, ?d>>",
        "the sequential multi-output run produced: {small:?}"
    );
    let big = run(r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i + 3], compute.write [n, i, i + i])
}
k = compute.parallel f "cpu"
outs = compute.plrun k (4096,)
(compute.collect outs(0), compute.collect outs(1))
"#);
    assert!(
        big.starts_with("([3, 4, 5, 6, 7, 8, 9, 10,"),
        "the parallel run's first elements must be the sequential run's: {big:?}"
    );
    assert!(
        big.ends_with("8190]): <array<?a, ?b>, array<?c, ?d>>"),
        "the parallel run's last element must be the same arithmetic: {big:?}"
    );
}

#[test]
fn a_parallel_run_over_the_threshold_covers_every_index() {
    // 4096 indices is over the fan-out threshold, so the run is spread over
    // workers whose chunk boundaries are a function of the count and the worker
    // count.  The boundary between two workers is where a rebased `write` would
    // go wrong, so this reads the first index, one in the middle and the last of
    // **both** output buffers: a worker that wrote into the wrong span, or one
    // that was skipped, cannot produce those values.
    let out = run(r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i + 3], compute.write [n, i, i + i])
}
k = compute.parallel f "cpu"
outs = compute.plrun k (4096,)
(compute.read [outs(0), 0], compute.read [outs(0), 2048], compute.read [outs(0), 4095], compute.read [outs(1), 0], compute.read [outs(1), 2048], compute.read [outs(1), 4095])
"#);
    assert_eq!(
        out, "(3, 2051, 4098, 0, 4096, 8190): <?a, ?b, ?c, ?d, ?e, ?f>",
        "a fan-out over 4096 indices produced: {out:?}"
    );
}

/// The signature-carrying wrapper over the ordinary tuple-shaped parameter.
///
/// What this pins is the wrapper's **third currying layer**, not the struct
/// parameter: `parallel_sig = f => b => s => …$parallel(f, b)` used to leave
/// `$parallel`'s operand `Parameterized`, because the innermost closure of a
/// frozen template was handed on without being re-instantiated per call — a
/// nested static closure's captures were invisible to the re-home check, and
/// its parent chain reached no dynamic ancestor, so the body kept reading a
/// previous apply's generation of the backend cell (`docs/notes/`
/// `compute-param-struct-handoff.md` §5). The tuple shape keeps the failure
/// surface on the wrapper mechanics alone.
#[test]
fn a_tuple_kernel_runs_through_the_signature_carrying_wrapper() {
    let out = run(r#"
--- compute = import "compute.lichen" ---
type_of = x => {t = _; x: t; t}
f = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 1]
}
k = compute.parallel_sig f "cpu" (type_of f)
out = compute.plrun k (4,)
compute.collect out
"#);
    assert_eq!(
        out, "[1, 2, 3, 4]: array<?a, ?b>",
        "parallel_sig over a tuple-shaped kernel produced: {out:?}"
    );
}

/// The named struct parameter end to end: a producer kernel fills an input
/// buffer, the consumer's parameter is `struct<.n Int, .in …, .out …>`, its
/// reads name their buffers (`k.in.a`), and the launch goes through
/// `parallel_sig` with the JIT'd signature.
///
/// This is the probe `docs/notes/compute-param-struct-handoff.md` §2 was
/// written around — blocker 1 (a named read resolves to its position in the
/// parameter **type's** field order) and blocker 2 (the wrapper above) meet in
/// one program. The spelling of the signature argument is load-bearing: the
/// type lambdas take an *instantiated* pair (`P (KT _)(.I In, .O Out)`), and
/// `.out` moves from the parameter to the result — the host fills
/// `struct<.n Int, .I In>`, which is what `compute.A` builds.
///
/// The `?a` in the answer is not a defect: it is the documented limit of
/// `plrun`'s result type — the element class of a value read back from a
/// buffer stays undecided (`docs/notes/compute-kernel-struct.md` §"Runtime /
/// codegen").
#[test]
fn a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper() {
    let out = run(r#"
--- compute = import "compute.lichen" ---
g = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}
kg = compute.parallel g "cpu"
inbuf = compute.plrun kg (3,)
In  = struct<.a _>
Out = struct<.z _>
Par = compute.P (compute.KT _)(.I In, .O Out)
Sig = compute.S (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read [k.in.a, i]
  compute.write [k.out.z, i, v * 2]
}
k = compute.parallel_sig f "cpu" Sig
out = compute.plrun k ((compute.A In)(.n 3, .I In(.a inbuf)))
compute.read [out, 1]
"#);
    assert_eq!(
        out, "22: ?a",
        "the struct-parameter kernel produced: {out:?}"
    );
}

#[test]
fn a_write_inside_a_conditional_is_refused() {
    // The every-ordinal-written invariant: output ordinal `k` must be written
    // on *every* index.  A write behind a condition would be written on only
    // one path, so the index function is refused — never quietly reduced to the
    // outputs it happens to write unconditionally.
    //
    // What refuses it here is the emitter's existing inline-call limit, one
    // layer before the emitter's own conditional-write guard: a same-module
    // call (`compute.write`) inside an `if` branch is not reduced, so the
    // branch still holds an `Apply`.  The refusal therefore names *that* cause,
    // and it is still a refusal — what this pins is that the program does not
    // run and does not silently produce one output buffer.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i], (if i <= 1 then compute.write [n, i, 1] else compute.write [n, i, 2]))
}
k = compute.parallel f "cpu"
outs = compute.plrun k (3,)
compute.read [outs(0), 2]
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "a conditional write is one refusal: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("compute.parallel") && message.contains("not yet supported"),
        "the refusal must name its own cause: {message:?}"
    );
}

#[test]
fn an_output_position_that_is_not_a_write_is_refused() {
    // The output count comes from the codomain's arity, so every position must
    // be a `compute.write`; a position that is a plain value is refused by its
    // position, not by a generic "unsupported kernel" message.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
f = cfg => {
  n = cfg(0)
  i = compute.range n
  (compute.write [n, i, i], i)
}
k = compute.parallel f "cpu"
outs = compute.plrun k (3,)
compute.read [outs(0), 2]
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one bad position is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("output 1") && message.contains("compute.write"),
        "the refusal must name the position that is not a write: {message:?}"
    );
}

/// A decided non-buffer in a buffer position is refused, not answered `parameterized`.
///
/// There is no way to make a buffer out of a program value, so the natural thing
/// to reach for is a plain array. It used to be accepted and to produce nothing:
/// the read stayed a lazy cell nothing forced, the launch answered
/// `parameterized`, and the program still printed a type — `array<?a, ?b>` — so it
/// read like a program that computed something.
///
/// The `Parameterized` fallback is not wrong in general: it is what makes a
/// kernel's own read deferrable. A program array is **decided**, so it is not the
/// case that fallback exists for, and a diagnostic is the honest answer.
#[test]
fn a_program_value_where_a_buffer_belongs_is_refused() {
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
data = [3, 1, 4, 1, 5, 9, 2, 6]
f = cfg => {
  n = cfg(0)
  i = compute.range n
  v = compute.read [cfg(1)(0), i]
  compute.write [n, i, v + 1]
}
k = compute.parallel f "cpu"
compute.collect (compute.plrun k (8, (data,)))
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one bad position is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("cfg(1)") && message.contains("position 0") && message.contains("array"),
        "the refusal must name the position and what it holds: {message:?}"
    );
    assert!(
        message.contains("no way to make one out of a program value"),
        "and it must say *why*, since the fix is not obvious from the fact: {message:?}"
    );
}

/// A float parameter domain is **permitted at every position the domain walk
/// reaches** — §4.4's first decision, and the two compound positions phase 0
/// closed deliberately are the interesting half of it.
///
/// The two halves are different decisions and both are pinned here.  A *scalar*
/// position is handed a value, so those kernels run.  A *compound* position is
/// one local of the element's class, so an aggregate has nothing to be lowered
/// into and a function has no scalar encoding at all: the `jit` succeeds and the
/// **argument** is what is refused, by name
/// (`docs/notes/floating-point.md` §4.2, §4.4).
#[test]
fn a_float_domain_is_permitted_at_every_position_the_walk_reaches() {
    // The parameter itself: a real float kernel, compiled, run and read back.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x : Float => x + 1.0)
compute.launch k 1.5
"#);
    assert_eq!(
        out, "2.5: Float",
        "a float-domain jit+launch produced: {out:?}"
    );

    // A tuple element: a mixed domain, where the leaf the body reads keeps its
    // own class and the fragment is lowered in the *body's*.
    let out = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Float> => p(0))
compute.launch k (1, 1.5)
"#);
    assert_eq!(out, "1: Int", "a float tuple element produced: {out:?}");

    // An array element inside a tuple: the `jit` is permitted, and the aggregate
    // argument is what the ABI cannot place — a compound position is one local,
    // so its argument is one value rather than its three elements.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, array<Float, 3>> => p(0))
compute.launch k (1, [1.5, 2.5, 3.5])
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("compute.kernel_launch")
            && messages[0].contains("takes 2 arguments")
            && messages[0].contains("supplied 4"),
        "the aggregate's three elements are three locals a two-local domain does not have: \
         {messages:?}"
    );

    // A function codomain: the walk reaches it, so the `jit` is permitted, and a
    // function argument has no scalar encoding to be lowered into.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int -> Float> => p(0))
compute.launch k (1, y => 1.0)
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    assert!(
        messages[0].contains("argument element 1") && messages[0].contains("a function"),
        "the refusal must name the position and what it holds: {messages:?}"
    );
}

/// A float launch argument is refused **by class**, not by the "no value"
/// catch-all and not by a pin on the kernel's side.
///
/// `call` gates its argument against a *fresh* domain cell, so `1.5` reaches the
/// run against an `Int` parameter and the run is the only place that can see
/// both classes.  `launch` reads the kernel's signature, so the checker refuses
/// it first — the `call` form is the one that reaches this message at all
/// (`docs/notes/floating-point.md` §4.2).
#[test]
fn a_float_argument_to_an_int_parameter_is_refused_by_class() {
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x => x + 1)
compute.call k 1.5
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one refusal is one diagnostic: {messages:?}"
    );
    assert_eq!(
        messages[0],
        "compute.kernel_launch: argument 0 is Float, but the kernel's parameter there is Int: \
         Int and Float do not convert",
        "the refusal must name the position and both classes"
    );
}

/// The seam: a real lichen program, dispatching to a real device.
///
/// Everything below this line is tested somewhere else and none of it together.
/// `compute.rs` proves the language side against the CPU backend; the GPU crate's
/// own tests prove a [`KernelFragment`] runs correctly on a device; the routing
/// test proves a `"gpu"` run reaches an installed backend. What none of them
/// touch is the seam itself — a kernel compiled from a lichen **function**, named
/// `"gpu"`, dispatched, and read back — and a break anywhere along it would leave
/// every other test green.
///
/// The program is a chain, deliberately. `k1` writes `[10, 11, 12]`, `k2` reads
/// that buffer and doubles it, so the second `plrun` consumes the first one's
/// result — which on a `"gpu"` run means the intermediate is handed to the next
/// kernel as a device id rather than being brought home in between. The CPU twin
/// of this program is `parallel_read_input_buffer` above; the two must agree, and
/// the value is chosen so that a `plrun` which silently fell back to the CPU
/// would still produce the right answer — what differs is *which* backend ran it,
/// so the backend's own name is reported alongside the value.
#[test]
fn a_gpu_program_chains_two_kernels_on_a_device() {
    // The installed-backend slot is process-global, so this test owns it for its
    // whole body: a parallel test in this binary must not observe a device that
    // another test has just uninstalled.
    let _installed = GPU_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(reason) = lichen_compute_gpu::install_default() {
        eprintln!("no device to run on, so the seam is not covered here: {reason}");
        return;
    }

    let source = r#"
--- compute = import "compute.lichen" ---
f1 = cfg => {
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}
k1 = compute.parallel f1 "gpu"
inbuf = compute.plrun k1 (3,)
f2 = cfg => {
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  compute.write [n, i, a + a]
}
k2 = compute.parallel f2 "gpu"
out = compute.plrun k2 (3, (inbuf,))
(compute.read [out, 0], compute.read [out, 1], compute.read [out, 2], compute.collect out)
"#;
    let out = run(source);
    // Uninstalling drops the context, so every device buffer it was holding goes
    // back at the same moment — which is the point of doing it here rather than
    // leaving it to process exit.
    lichen_compute_gpu::uninstall();

    // The collected array's element type decides (`Int`): the deferred
    // unification the write's element class goes through now commits the type
    // value instead of merging silently (docs/notes/defer-pending-type-forms.md).
    assert_eq!(
        out, "(20, 22, 24, [20, 22, 24]): <?a, ?b, ?c, array<Int, ?d>>",
        "a two-kernel \"gpu\" chain produced"
    );
    assert_eq!(
        lichen_compute_gpu::installed_backend_name(),
        None,
        "the backend was uninstalled, so a later test cannot inherit a device"
    );
}

/// Serialises the tests that install a backend, because the slot is
/// process-global and this binary runs its tests in parallel.
static GPU_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The placeholder each program's backend name is written as, so **one source**
/// is what both backends are given. Substituting the name is the only difference
/// between the two runs.
const BACKEND: &str = "BACKEND";

/// Elements each run covers: one whole workgroup and five more.
///
/// **Not a multiple of [`LOCAL_SIZE_X`], and that is the point of the number.**
/// A dispatch is `ceil(count / LOCAL_SIZE_X)` workgroups, so a count that
/// divides the workgroup sends **no surplus lane** — and a wrong element width
/// does not fail there, it corrupts the neighbour. A wrong width only becomes a
/// wrong *answer* once a lane reads or writes past the end of what was bound.
const ELEMENT_COUNT: usize = lichen_compute_gpu::LOCAL_SIZE_X as usize + 5;

/// The two backends' answers to one program, or `None` when there is no device
/// to run the second one on.
///
/// The oracle is the **other real backend**: the wasm one in `lichen-compute` and
/// this crate's SPIR-V emitter implement the same IR and were written apart, so
/// nothing else in the tree can say whether they agree. What an element occupies
/// is [`ScalarClass::byte_width`], a promise only a comparison of the two
/// answers keeps (`docs/notes/floating-point.md` §5.1) — which is why the
/// comparison is over *values* and not over `installed_backend_name()`, a weaker
/// claim that a silently-fallen-back run would also satisfy.
///
/// The CPU run goes first and always happens, whether or not a device opens: it
/// is the answer being compared, not half of a comparison. It is run **before**
/// the install on purpose, so a `"cpu"` run cannot be a `"gpu"` run's leftovers
/// (`crates/lichen-language/examples/crossbackend.rs` is the probe for that).
///
/// The installed-backend slot is process-global, so the whole body holds the
/// same mutex the device test above holds — a parallel test in this binary must
/// not observe a device another test has just uninstalled.
fn answer_from_each_backend(source: &str) -> Option<(String, String)> {
    let _installed = GPU_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cpu = run(&source.replace(BACKEND, "cpu"));
    if let Err(reason) = lichen_compute_gpu::install_default() {
        eprintln!(
            "no device to run the second backend on, so the two backends are not compared \
                    here: {reason}"
        );
        return None;
    }
    let gpu = run(&source.replace(BACKEND, "gpu"));
    // Uninstalling drops the context, so every device buffer it was holding goes
    // back at the same moment — the reason the device test above does it rather
    // than leaving it to process exit.
    lichen_compute_gpu::uninstall();
    Some((cpu, gpu))
}

/// A float buffer, through two kernels, computed by both backends.
///
/// `k1` writes a `Float` element; `k2` reads that buffer and writes it doubled.
/// This is the chain the integer case below runs and the chain a `"gpu"` run
/// keeps on the device: the intermediate is handed to the second kernel as a
/// resident id rather than coming home in between.
///
/// **The `0.0` is load-bearing, not decoration.** A parallel fragment's class is
/// decided from the value a write fills before the body is emitted, and a body
/// with no concrete `Float` operand is lowered in `Int` — so `a + a` alone
/// declares an `Int` fragment, and the run refuses the `Float` input buffer by
/// name (`docs/notes/floating-point.md` §5.1, point 3). `0.0 + a + a` anchors the
/// class and computes the same number.
///
/// **Every element here carries the same float**, and that is a property of these
/// two bodies rather than of the language. A float that *varies* with the index
/// arrives through `int2float` — the index being the only per-lane value an `Int`
/// operation can produce, and a body that mixes the two classes in one operation
/// is still refused (`docs/notes/floating-point.md` §5.1). The varying case is
/// `a_varying_float_element_is_seeded_from_the_index`, through the same two
/// backends.
#[test]
fn a_float_fragment_agrees_across_the_two_backends() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
f1 = cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, 1.5]
}}
k1 = compute.parallel f1 "{BACKEND}"
inbuf = compute.plrun k1 ({ELEMENT_COUNT},)
f2 = cfg => {{
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  compute.write [n, i, 0.0 + a + a]
}}
k2 = compute.parallel f2 "{BACKEND}"
out = compute.plrun k2 ({ELEMENT_COUNT}, (inbuf,))
(compute.read [out, 0], compute.read [out, {last}], compute.collect out)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some((cpu, gpu)) = answer_from_each_backend(&source) else {
        return;
    };
    println!("float  cpu: {cpu}\nfloat  gpu: {gpu}");
    assert_eq!(
        gpu, cpu,
        "the two backends answered one float fragment differently"
    );
}

/// The same two-kernel chain over `Int`, so the comparison is not only over a
/// class whose every element is the same number.
///
/// `k1` writes `i + 10` and `k2` writes `a + a`, so the answer is
/// `2 * (i + 10)` and **every element differs from its neighbour**. A width
/// mistake therefore shows up at the first index it touches, rather than needing
/// the tail to expose it.
#[test]
fn an_integer_fragment_agrees_across_the_two_backends() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
f1 = cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, i + 10]
}}
k1 = compute.parallel f1 "{BACKEND}"
inbuf = compute.plrun k1 ({ELEMENT_COUNT},)
f2 = cfg => {{
  n = cfg(0)
  i = compute.range n
  a = compute.read [cfg(1)(0), i]
  compute.write [n, i, a + a]
}}
k2 = compute.parallel f2 "{BACKEND}"
out = compute.plrun k2 ({ELEMENT_COUNT}, (inbuf,))
(compute.read [out, 0], compute.read [out, {last}], compute.collect out)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some((cpu, gpu)) = answer_from_each_backend(&source) else {
        return;
    };
    println!("int    cpu: {cpu}\nint    gpu: {gpu}");
    assert_eq!(
        gpu, cpu,
        "the two backends answered one integer fragment differently"
    );
}

/// A `Float` buffer whose elements **differ from their neighbour**, each one
/// seeded from the invocation index — the case
/// `docs/notes/floating-point.md` §5.1 recorded as not writable.
///
/// `int2float i` is the crossing that writes it.  The index is the language's
/// `Int`, a `Float` fragment carries it in an `f32` holding the exact integer,
/// and the two backends answer one crossing two different ways from the same IR:
/// the wasm one emits **nothing** where the value it holds is already a float,
/// and SPIR-V emits `OpConvertUToF` because its index is a 32-bit integer.  That
/// is precisely the pair of answers a comparison of the two backends keeps, which
/// is why this runs over a whole workgroup plus five rather than over five
/// elements — a wrong element width corrupts a neighbour, and only a buffer where
/// every element differs shows it at the first index it touches.
#[test]
fn a_varying_float_element_is_seeded_from_the_index() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
f = cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, int2float i + 0.5]
}}
k = compute.parallel f "{BACKEND}"
out = compute.plrun k ({ELEMENT_COUNT},)
(compute.read [out, 0], compute.read [out, 1], compute.read [out, {last}], compute.collect out)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some((cpu, gpu)) = answer_from_each_backend(&source) else {
        return;
    };
    println!("varying float  cpu: {cpu}\nvarying float  gpu: {gpu}");
    assert!(
        cpu.starts_with("(0.5, 1.5, 68.5, [0.5, 1.5, 2.5"),
        "every element is its index plus a half, so none of them repeats: {cpu:?}"
    );
    assert_eq!(
        gpu, cpu,
        "the two backends answered one index-seeded float fragment differently"
    );
}

/// A `jit` kernel crossing the two classes, in both directions and twice over.
///
/// **The kernel's own domain is what makes these the hard cases**: a `jit`
/// function's locals keep the classes the author annotated, so `int2float x` over
/// an `Int` parameter is a real crossing of a value the body never computed, and
/// `float2int` of a `Float` one is the truncation toward zero the interpreter
/// promises for the same number.  The last two rows are a literal folded into the
/// body's own class and a crossing in each direction inside one expression.
#[test]
fn a_jit_kernel_crosses_the_two_classes_both_ways() {
    let cases = [
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Int => int2float x)
compute.launch k 5
"#,
            "5.0: Float",
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => float2int x)
compute.launch k 5.7
"#,
            "5: Int",
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => int2float (x > 1.0))
compute.launch k 5.0
"#,
            "1.0: Float",
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Int => x + float2int 3.7)
compute.launch k 5
"#,
            "8: Int",
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => int2float (float2int (x + 0.5)))
compute.launch k 3.0
"#,
            "3.0: Float",
        ),
    ];
    for (source, expected) in cases {
        let out = run(source);
        assert_eq!(out, expected, "the kernel program produced: {out:?}");
    }
}

/// A body may compute in one class and cross to the other, and the crossing is
/// spelled — `int2float`/`float2int` are the only way between the two.
///
/// **This is what per-value classes bought**: a fragment is no longer one
/// representation for its whole body, so an `Int` parameter may be added as an
/// integer and only then crossed (`int2float (x + 1)`), where the earlier
/// fragment-wide rule refused it. The price is recorded in
/// `docs/notes/floating-point.md`: in a float fragment `Int` data is 32-bit on
/// the GPU and 64-bit on the CPU, so the two targets diverge past 2³².
///
/// The parallel half is the same decision inside an **integer** fragment: its
/// buffers are integers, and the body crosses up to `Float` to compute and back
/// down to store. That is the case both element types have to be declared for in
/// one SPIR-V module (`crates/lichen-compute-gpu/src/spirv.rs`).
#[test]
fn a_body_may_compute_in_one_class_and_cross() {
    let out = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Int => int2float (x + 1))
compute.launch k 5
"#);
    assert_eq!(out, "6.0: Float", "the integer add stays an integer's");

    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
f = cfg => {{
  n = cfg(0)
  i = compute.range n
  compute.write [n, i, float2int (int2float i + 0.5)]
}}
k = compute.parallel f "{BACKEND}"
out = compute.plrun k ({ELEMENT_COUNT},)
(compute.read [out, 0], compute.read [out, 1], compute.read [out, {last}], compute.collect out)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some((cpu, gpu)) = answer_from_each_backend(&source) else {
        return;
    };
    println!("crossing  cpu: {cpu}\ncrossing  gpu: {gpu}");
    assert!(
        cpu.starts_with(&format!("(0, 1, {}, [0, 1, 2,", ELEMENT_COUNT - 1)),
        "the truncation of the index plus a half is the index, so every element \
         differs from its neighbour: {cpu:?}"
    );
    assert_eq!(
        gpu, cpu,
        "the two backends answered one class crossing differently"
    );
}

//! `jit` a function to a kernel, `launch` it, and check the kernel and
//! codomain types. See lichen-compute.md.

use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_lowlevel::{Module, NodeId};

mod common;

/// Compile and run `source`, returning the module, the root value and the root
/// type node.
fn run(source: &str) -> (Module<LangProgram>, LangValue, NodeId) {
    common::run(source)
}

/// Compile and run `source`, returning the rendered `value: type` output.
fn render(source: &str) -> String {
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
    // The backend is a string and the parse is strict: a typo names what is accepted.
    let diags = fail(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + i))
}
k = compute.parallel f "Gpu"
out = (compute.plrun k ((compute.A In)(.n 4, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 0))
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
    // The static gate catches the shape; the runtime parse owns which strings count.
    let diags = fail(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + i))
}
k = compute.parallel f 4
out = (compute.plrun k ((compute.A In)(.n 4, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 0))
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
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => x + 1)
compute.launch k 5
"#);
    assert_eq!(
        common::usize_of(&value),
        6,
        "jit+launch produced the launch result"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_multi_op_signature() {
    // A body of several scalar operations: `x + 1 + 2`.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => x + 1 + 2)
compute.launch k 5
"#);
    assert_eq!(
        common::usize_of(&value),
        8,
        "multi-op jit+launch produced 8"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
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
    // A tuple domain compiles to a wasm `(i64, i64) -> i64` and launches a 2-tuple.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
compute.launch k (5, 3)
"#);
    assert_eq!(
        common::usize_of(&value),
        8,
        "tuple-domain jit+launch produced 8"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_multi_arg_ternary() {
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int, Int> => p(0) + p(1) + p(2))
compute.launch k (5, 3, 2)
"#);
    assert_eq!(
        common::usize_of(&value),
        10,
        "ternary jit+launch produced 10"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_multi_arg_sub() {
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => p(0) - p(1))
compute.launch k (10, 3)
"#);
    assert_eq!(common::usize_of(&value), 7, "tuple subtraction produced 7");
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn launch_rejects_wrong_arity() {
    // The `launch` gate unifies the argument against the domain, so a scalar fails.
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
    // Non-function values are graph-shared, so the JIT lowers the read to a const.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
a = 42
k = compute.jit (x => x + a)
compute.launch k 1
"#);
    assert_eq!(
        common::usize_of(&value),
        43,
        "closure-over-constant produced 43"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_multi_arg_all_ops() {
    // A tuple-domain body mixing `+`, `-`, `<=` and a constant.
    // (5 + 3) - (5 <= 3) = 8 - 0 = 8.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <Int, Int> => (p(0) + p(1)) - (p(0) <= p(1)))
compute.launch k (5, 3)
"#);
    assert_eq!(common::usize_of(&value), 8, "mixed-op tuple produced 8");
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_lowers_the_arithmetic_comparison_and_bitwise_operators() {
    // One kernel per operator, so each is on the lowering path rather than hidden
    // behind another's result.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k1 = compute.jit (x => ((x * 3) % 7) + (x / 2) + ((x < 5) & (x > 1)))
k2 = compute.jit (p : <Int, Int> => p(0) > p(1))
k3 = compute.jit (p : <Int, Int> => p(0) >= p(1))
k4 = compute.jit (p : <Int, Int> => p(0) != p(1))
k5 = compute.jit (p : <Int, Int> => (p(0) < p(1)) | (p(0) == p(1)))
k6 = compute.jit (p : <Int, Int> => (p(0) > p(1)) ^ (p(0) == p(1)))
(compute.launch k1 4, compute.launch k2 (5, 3), compute.launch k3 (3, 3), compute.launch k4 (5, 3), compute.launch k5 (5, 5), compute.launch k6 (5, 5))
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(
        common::usize_of(&elements[0]),
        8,
        "the arithmetic group's product-remainder-division-comparison result"
    );
    assert_eq!(
        common::usize_of(&elements[1]),
        1,
        "the `>` comparison yields 1 for 5 > 3"
    );
    assert_eq!(
        common::usize_of(&elements[2]),
        1,
        "the `>=` comparison yields 1"
    );
    assert_eq!(
        common::usize_of(&elements[3]),
        1,
        "the `!=` comparison yields 1"
    );
    assert_eq!(common::usize_of(&elements[4]), 1, "the `|` pair yields 1");
    assert_eq!(common::usize_of(&elements[5]), 1, "the `^` pair yields 1");
}

#[test]
fn jit_conditional_then() {
    // `if x <= 3 then 10 else 20` lowers to `[20, 10][x <= 3]` — a 2-element
    // array index the JIT lowers to a wasm `select`.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 2
"#);
    assert_eq!(
        common::usize_of(&value),
        10,
        "conditional (then) produced 10"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_conditional_else() {
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x => if x <= 3 then 10 else 20)
compute.launch k 5
"#);
    assert_eq!(
        common::usize_of(&value),
        20,
        "conditional (else) produced 20"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_nested_tuple_domain() {
    // A nested tuple domain flattens to three wasm locals read at offsets 0, 1, 2.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (p : <<Int, Int>, Int> => p(0)(0) + p(0)(1) + p(1))
compute.launch k ((2, 3), 4)
"#);
    assert_eq!(
        common::usize_of(&value),
        9,
        "nested tuple jit+launch produced 9"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );
}

#[test]
fn jit_cross_kernel_call() {
    // Style 2: k1's body calls k0, so both are in one module:   launch k1 6 = k0(6) = 7.
    let (_module, value, _root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k0 = compute.jit ((y : Int) => y + 1)
k1 = compute.jit ((x : Int) => k0 x)
compute.launch k1 6
"#);
    assert_eq!(common::usize_of(&value), 7, "cross-kernel call produced 7");
}

/// The third shape a cross-kernel argument can take: an operator applied inside the
/// argument.
///
/// # Invariant
/// A routed operator's identity is read out of its **frozen callee's body**, so `k0 (x +
/// 1)` is emitted; it was refused by name while the class channel was the only route
/// (`docs/notes/operator-polymorphism.md` §7.1, cost 1).
#[test]
fn jit_an_operator_inside_a_cross_kernel_argument_is_emitted() {
    let (_module, value, _root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k0 = compute.jit ((y : Int) => y + 1)
k1 = compute.jit ((x : Int) => k0 (x + 1))
compute.launch k1 5
"#);
    assert_eq!(common::usize_of(&value), 7, "k0 (x + 1) with x = 5");
}

#[test]
fn jit_cross_kernel_subexpr() {
    // A call result as a sub-expression; the JIT looks through the peel and emits the
    // call:   launch k1 5 = k0(5) + 1 = 7.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (y : Int => y + 1)
k1 = compute.jit (x : Int => k0 (x) + 1)
compute.launch k1 5
"#);
    assert_eq!(common::usize_of(&value), 7, "subexpr produced 7");
}

#[test]
fn jit_inline_lichen_function() {
    // The deep pass reduces the same-module call, so the JIT traces the reduced
    // graph.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
helper = y => y + 2
k = compute.jit (x => helper x + 1)
compute.launch k 5
"#);
    assert_eq!(common::usize_of(&value), 8, "inline produced 8");
}

#[test]
fn jit_cross_kernel_wrapper() {
    // The wrapper is a two-step native, so its argument arrives as an undecided
    // cell.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit ((y : Int) => y + 1)
k1 = compute.jit ((x : Int) => compute.launch k0 x)
compute.launch k1 6
"#);
    assert_eq!(common::usize_of(&value), 7, "wrapper produced 7");
}

#[test]
fn jit_cross_kernel_tuple_argument() {
    // A bare kernel apply states no signature, so the caller's `x` is annotated.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) + p(1))
k1 = compute.jit (x : Int => k0 (x, 1))
compute.launch k1 5
"#);
    assert_eq!(
        common::usize_of(&value),
        6,
        "tuple-argument cross-kernel call produced 6"
    );
}

#[test]
fn jit_cross_kernel_passes_the_parameter_through() {
    // A whole-parameter read passes through as the callee's own locals.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) - p(1))
k1 = compute.jit (q : <Int, Int> => k0 q)
compute.launch k1 (9, 4)
"#);
    assert_eq!(
        common::usize_of(&value),
        5,
        "parameter pass-through produced 5"
    );
}

#[test]
fn jit_cross_kernel_passes_a_sub_tuple_through() {
    // A sub-tuple read's leaves are contiguous, so the callee's arguments are the
    // locals from the read's start.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <<Int, Int>, Int> => p(0)(0) - p(0)(1) + p(1))
k1 = compute.jit (r : <Int, <<Int, Int>, Int>> => k0 r(1))
compute.launch k1 (100, ((9, 4), 5))
"#);
    assert_eq!(
        common::usize_of(&value),
        10,
        "sub-tuple pass-through produced 10"
    );
}

#[test]
fn jit_cross_kernel_tuple_argument_through_the_wrapper() {
    // Style 3, a tuple argument reached through the cell's equality class:
    //   launch k1 5 = k0(5, 1) = 6.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (p : <Int, Int> => p(0) + p(1))
k1 = compute.jit (x : Int => compute.launch k0 (x, 1))
compute.launch k1 5
"#);
    assert_eq!(
        common::usize_of(&value),
        6,
        "wrapper tuple-argument launch produced 6"
    );
}

#[test]
fn jit_inline_nested_function() {
    // The deep pass reduces a nested inline call through to the leaf arithmetic.
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
a = y => y + 1
b = y => a y + 1
k = compute.jit (x => b x + 1)
compute.launch k 5
"#);
    assert_eq!(common::usize_of(&value), 8, "nested inline produced 8");
}

#[test]
fn a_kernel_value_and_type_render_by_name() {
    // The signature fields hold *type values*: each field's type is `Type` and its
    // value is the type itself.
    let out = render(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (y : Int => y + y)
k
"#,
    );
    assert_eq!(
        out, "(raw Kernel, Int, Int): struct<.native raw[?a, ?b], .I Type, .O Type>",
        "kernel value/type: {out:?}"
    );
}

#[test]
fn jit_tuple_codomain_returns_several_values() {
    // The mirror of the multi-arity domain: one wasm stack slot per leaf.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0), p(1)))
compute.launch k (5, 3)
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 5, "the identity leaf");
    assert_eq!(common::usize_of(&elements[1]), 3, "the summed leaf");
}

#[test]
fn jit_tuple_codomain_computes_each_leaf() {
    // Each leaf is its own scalar body, so a leaf read from the wrong stack slot shows.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0), p(0) + p(1)))
compute.launch k (5, 3)
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 5, "the identity leaf");
    assert_eq!(common::usize_of(&elements[1]), 8, "the summed leaf");
}

#[test]
fn jit_tuple_codomain_elements_are_indexable() {
    // The returned tuple is an ordinary array value; a read addresses a leaf.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0) + 10, p(1) + 100))
r = compute.launch k (2, 3)
(r(0), r(1))
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 12, "the first indexed leaf");
    assert_eq!(
        common::usize_of(&elements[1]),
        103,
        "the second indexed leaf"
    );
}

#[test]
fn jit_three_value_codomain_returns_three_values() {
    // A distinct wasm signature from the two-value one, keyed on the arity *pair*.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x : Int => (x, x + 1, x + 2))
compute.launch k 7
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 7, "the first leaf");
    assert_eq!(common::usize_of(&elements[1]), 8, "the second leaf");
    assert_eq!(common::usize_of(&elements[2]), 9, "the third leaf");
}

#[test]
fn jit_tuple_codomain_launches_through_the_cross_kernel_wrapper() {
    // `compute.call` reaches the same path as `launch`, and both state the result
    // as `k.O`.
    let (module, value, root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => (p(0) - p(1), p(0) + p(1)))
compute.call k (10, 4)
"#);
    assert_eq!(
        common::usize_array(&module, &value),
        vec![6, 14],
        "compute.call on a tuple-codomain kernel produced the two leaves"
    );
    assert!(
        !common::type_is_undecided(&module, root_ty),
        "the typed `call` form states the result, so the kernel's codomain is \
         what the run is read against"
    );
}

#[test]
fn jit_refuses_a_cross_kernel_call_to_a_multi_value_kernel() {
    // The wasm `call` pushes one value per result, so a multi-value callee is
    // refused rather than truncated.
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
fn a_tuple_domain_kernel_type_renders_as_a_function() {
    // A tuple-domain kernel's signature is `[<Int, Int>, Int]`, carried in `.I`.
    let out = render(
        r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Int> => p(0) + p(1))
k
"#,
    );
    assert_eq!(
        out, "(raw Kernel, <Int, Int>, Int): struct<.native raw[?a, ?b], .I TypeTuple, .O Type>",
        "tuple-domain kernel value/type: {out:?}"
    );
}

#[test]
fn wrapper_functions_render_with_named_type_variables() {
    // The wrapper's cells are undecided at module level, so they render as `?a`/`?b`;
    // only an applied result resolves.
    assert_eq!(
        render(
            r#"
--- compute = import "compute.lichen" ---
compute.jit
"#
        ),
        "Function: raw[?a, raw[?b, ?c]] -> raw[?d, raw[?e, ?f]] -> struct<.native raw[?g, ?h], .I raw[?b, ?c], .O raw[?e, ?f]>",
        "jit wrapper value/type"
    );
    // Both cells are the `.I`/`.O` field read's pair, which still marks when
    // undecided (raw-rendering-mark.md §2).
    assert_eq!(
        render(
            r#"
--- compute = import "compute.lichen" ---
compute.launch
"#
        ),
        "Function: ?a -> raw[?b, ?c] -> raw[?d, ?e]",
        "launch wrapper value/type"
    );
}

#[test]
fn parallel_range_write_is_map() {
    // A parallel run's result is bound with the codomain type, so the host reads a
    // field by name.
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + i))
}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In)(.n 4, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 2))
"#);
    assert_eq!(
        common::usize_of(&value),
        4,
        "parallel range/write map produced 4"
    );
    // `compute.read` states its result as `x.from.element`, so a written-down
    // element type reads back decided.
    assert!(
        !common::type_is_undecided(&module, root_ty),
        "a `Buf`'s `.element` states what the read produces"
    );
}

#[test]
fn parallel_read_input_buffer() {
    // A second kernel reads the first's buffer through its parameter's `.in.b`.
    let (_module, value, _root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "cpu"
inbuf = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "cpu"
out = (compute.plrun k2 ((compute.A In2)(.n 3, .I In2(.b inbuf.z))) : Out2)
compute.read ((compute.Read _)(.from out.w, .at 1))
"#);
    assert_eq!(
        common::usize_of(&value),
        22,
        "parallel read/write produced 22"
    );
}

#[test]
fn parallel_write_only_collects_whole_buffer() {
    // `compute.collect out.z` materialises the whole output buffer into an array.
    let (module, value, _root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
compute.collect out.z
"#);
    assert_eq!(
        common::usize_array(&module, &value),
        vec![1, 2, 3],
        "parallel collect produced the whole buffer"
    );
}

#[test]
fn a_refused_plrun_count_says_why() {
    // The count is bounded, and a count past the bound is refused by name.
    let messages = fail(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + i))
}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In)(.n 2000000, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 2))
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
fn a_runtime_scalar_reaches_the_body_beside_the_extent() {
    // A scalar leaf arrives as a real argument beside the extent and the index.
    // See compute-runtime-scalars.md.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
In0  = struct<.a Int>
Out0 = struct<.z (compute.Buf _)>
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
g = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In0)(.n 3, .I In0(.a 0))) : Out0)
In  = struct<.b (compute.Buf _)>
Out = struct<.w (compute.Buf _)>
Par = struct<.n Int, .alpha Float, .in In, .out Out>
Host = struct<.n Int, .alpha Float, .I In>
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value v + float2int k.alpha))
}
k = compute.parallel f "cpu"
out = (compute.plrun k (Host(.n 3, .alpha 2.0, .I In(.b inbuf.z))) : Out)
(compute.read ((compute.Read _)(.from out.w, .at 0)), compute.read ((compute.Read _)(.from out.w, .at 1)), compute.read ((compute.Read _)(.from out.w, .at 2)))
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(
        common::usize_of(&elements[0]),
        12,
        "the first bumped element"
    );
    assert_eq!(
        common::usize_of(&elements[1]),
        13,
        "the second bumped element"
    );
    assert_eq!(
        common::usize_of(&elements[2]),
        14,
        "the third bumped element"
    );
}

#[test]
fn a_scalar_leaf_of_the_wrong_class_is_refused_by_name() {
    // A leaf arrives as the value *its own field* declared, so a wrong class is
    // refused by name.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
In0  = struct<.a Int>
Out0 = struct<.z (compute.Buf _)>
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
g = (k : Par0) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In0)(.n 3, .I In0(.a 0))) : Out0)
In  = struct<.b (compute.Buf _)>
Out = struct<.w (compute.Buf _)>
Par = struct<.n Int, .alpha Float, .in In, .out Out>
Host = struct<.n Int, .alpha _, .I In>
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value v + float2int k.alpha))
}
k = compute.parallel f "cpu"
out = (compute.plrun k (Host(.n 3, .alpha 2, .I In(.b inbuf.z))) : Out)
compute.read ((compute.Read _)(.from out.w, .at 0))
"#,
    );
    assert!(
        messages
            .iter()
            .any(|message| message.contains("scalar leaf is Float")
                && message.contains("passes an Int")),
        "the refusal must name the leaf's class and what was passed: {messages:?}"
    );
}

#[test]
fn a_refused_call_argument_element_says_why() {
    // The element is refused at run time, and is **nested** so the message must
    // name a path into it.
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
    // A bare string type-checks (the domain is a fresh cell) and reaches the run
    // as `undecided`.
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
    // `CallOp` lets the count through, so this refusal is the only account.
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
    // The codomain is a **tuple of writes**, so one pass emits every output buffer.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i)), compute.write ((compute.Write _)(.to k.out.y, .at i, .value i + i)))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from outs.x, .at 2)), compute.read ((compute.Read _)(.from outs.y, .at 2)))
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 2, "the first output's read");
    assert_eq!(
        common::usize_of(&elements[1]),
        4,
        "the second output's read"
    );
}

#[test]
fn parallel_multi_output_collects_each_output() {
    // `collect` materialises one output buffer whole out of the single pass.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "cpu"
inbuf = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _), .x (compute.Buf _), .y (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  (compute.write ((compute.Write _)(.to k.out.w, .at i, .value a)), compute.write ((compute.Write _)(.to k.out.x, .at i, .value a + a)), compute.write ((compute.Write _)(.to k.out.y, .at i, .value i)))
}
k2 = compute.parallel f2 "cpu"
outs = (compute.plrun k2 ((compute.A In2)(.n 3, .I In2(.b inbuf.z))) : Out2)
compute.collect outs.x
"#);
    assert_eq!(
        common::usize_array(&module, &value),
        vec![20, 22, 24],
        "collecting the second output produced the doubled buffer"
    );
}

#[test]
fn a_multi_output_parallel_run_is_identical_sequential_and_parallel() {
    // A parallel run **partitions** the index range over workers, and `write`
    // rebases the index by the worker's base.
    let (small_module, small, _) = run(r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i + 3)), compute.write ((compute.Write _)(.to k.out.y, .at i, .value i + i)))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 4, .I In(.a 0))) : Out)
(compute.collect outs.x, compute.collect outs.y)
"#);
    let small_pair = common::array_values(&small_module, &small);
    assert_eq!(
        common::usize_array(&small_module, &small_pair[0]),
        vec![3, 4, 5, 6],
        "the sequential run's first output"
    );
    assert_eq!(
        common::usize_array(&small_module, &small_pair[1]),
        vec![0, 2, 4, 6],
        "the sequential run's second output"
    );
    let (big_module, big, _) = run(r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i + 3)), compute.write ((compute.Write _)(.to k.out.y, .at i, .value i + i)))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 4096, .I In(.a 0))) : Out)
(compute.collect outs.x, compute.collect outs.y)
"#);
    let big_pair = common::array_values(&big_module, &big);
    let big_first = common::usize_array(&big_module, &big_pair[0]);
    let big_second = common::usize_array(&big_module, &big_pair[1]);
    assert_eq!(
        &big_first[..8],
        &[3, 4, 5, 6, 7, 8, 9, 10],
        "the parallel run's first elements must be the sequential run's"
    );
    assert_eq!(
        big_second.last(),
        Some(&8190),
        "the parallel run's last element must be the same arithmetic"
    );
}

#[test]
fn a_parallel_run_over_the_threshold_covers_every_index() {
    // A worker boundary is where a rebased `write` would go wrong, so this reads
    // three indices of **both** buffers.
    let (module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i + 3)), compute.write ((compute.Write _)(.to k.out.y, .at i, .value i + i)))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 4096, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from outs.x, .at 0)), compute.read ((compute.Read _)(.from outs.x, .at 2048)), compute.read ((compute.Read _)(.from outs.x, .at 4095)), compute.read ((compute.Read _)(.from outs.y, .at 0)), compute.read ((compute.Read _)(.from outs.y, .at 2048)), compute.read ((compute.Read _)(.from outs.y, .at 4095)))
"#);
    let elements = common::array_values(&module, &value);
    assert_eq!(
        common::usize_of(&elements[0]),
        3,
        "first output, first index"
    );
    assert_eq!(
        common::usize_of(&elements[1]),
        2051,
        "first output, middle index"
    );
    assert_eq!(
        common::usize_of(&elements[2]),
        4098,
        "first output, last index"
    );
    assert_eq!(
        common::usize_of(&elements[3]),
        0,
        "second output, first index"
    );
    assert_eq!(
        common::usize_of(&elements[4]),
        4096,
        "second output, middle index"
    );
    assert_eq!(
        common::usize_of(&elements[5]),
        8190,
        "second output, last index"
    );
}

/// The named struct parameter end to end; the answer's `?a` is `plrun`'s
/// documented result-type limit.
#[test]
fn a_struct_parameter_kernel_runs_through_the_signature_carrying_wrapper() {
    let (_module, value, _root_ty) = run(r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
g = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
kg = compute.parallel g "cpu"
inbuf = (compute.plrun kg ((compute.A In)(.n 3, .I In(.a 0))) : Out)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f = (k : Par2) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value v * 2))
}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In2)(.n 3, .I In2(.b inbuf.z))) : Out2)
compute.read ((compute.Read _)(.from out.w, .at 1))
"#);
    assert_eq!(
        common::usize_of(&value),
        22,
        "the struct-parameter kernel produced 22"
    );
}

#[test]
fn a_write_inside_a_conditional_is_refused() {
    // Every output ordinal must be written on *every* index; a
    // conditional write is refused, never quietly reduced.

    // The guard fires after the arm lowers: a `Select` would emit
    // both arms on every lane. (compute-buffer-wrapper.md)
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i)), (if i <= 1 then compute.write ((compute.Write _)(.to k.out.y, .at i, .value 1)) else compute.write ((compute.Write _)(.to k.out.y, .at i, .value 2))))
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from outs.x, .at 2))
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "a conditional write is one refusal: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("compute.parallel") && message.contains("inside a conditional's arm"),
        "the refusal must name its own cause: {message:?}"
    );
}

#[test]
fn an_output_position_that_is_not_a_write_is_refused() {
    // The output count comes from the codomain's arity, so a non-write position is
    // refused by its position.
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.x (compute.Buf _), .y (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  (compute.write ((compute.Write _)(.to k.out.x, .at i, .value i)), i)
}
k = compute.parallel f "cpu"
outs = (compute.plrun k ((compute.A In)(.n 3, .I In(.a 0))) : Out)
compute.read ((compute.Read _)(.from outs.x, .at 2))
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

/// A decided non-buffer in a buffer position is refused: there is no way to make
/// a buffer out of a program value.
#[test]
fn a_program_value_where_a_buffer_belongs_is_refused() {
    let messages = fail(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a (compute.Buf _)>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  v = compute.read ((compute.Read _)(.from k.in.a, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value v + 1))
}
data = [3, 1, 4, 1, 5, 9, 2, 6]
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In)(.n 8, .I In(.a data))) : Out)
compute.collect out.z
"#,
    );
    assert_eq!(
        messages.len(),
        1,
        "one bad position is one diagnostic: {messages:?}"
    );
    let message = &messages[0];
    assert!(
        message.contains("input buffer")
            && message.contains("position 0")
            && message.contains("holds a number"),
        "the refusal must name the position and what it holds: {message:?}"
    );
    assert!(
        message.contains("no way to make one out of a program value"),
        "and it must say *why*, since the fix is not obvious from the fact: {message:?}"
    );
}

/// A compound position is one local, so its argument is refused at launch.
/// See floating-point.md §4.4.
#[test]
fn a_float_domain_is_permitted_at_every_position_the_walk_reaches() {
    // The parameter itself: a real float kernel, compiled, run and read back.
    let (module, value, root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (x : Float => x + 1.0)
compute.launch k 1.5
"#);
    assert_eq!(
        common::float_of(&value),
        2.5,
        "a float-domain jit+launch produced 2.5"
    );
    assert!(
        common::type_is_float(&module, root_ty),
        "the launch result is typed Float"
    );

    // A mixed domain: the leaf keeps its own class and the fragment is lowered in
    // the body's.
    let (module, value, root_ty) = run(r#"
--- compute = import "compute.lichen" ---
k = compute.jit (p : <Int, Float> => p(0))
compute.launch k (1, 1.5)
"#);
    assert_eq!(
        common::usize_of(&value),
        1,
        "a float tuple element produced 1"
    );
    assert!(
        common::type_is_int(&module, root_ty),
        "the launch result is typed Int"
    );

    // The `jit` is permitted; the aggregate argument is what the ABI cannot place.
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

    // A function codomain is permitted; its argument has no scalar encoding.
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

/// A float launch argument is refused **by class**: only the run sees both.
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

/// The seam: a lichen function compiled as `"gpu"`, dispatched and read back.
/// Every other test proves one side of it.
#[test]
fn a_gpu_program_chains_two_kernels_on_a_device() {
    // The installed-backend slot is process-global, so this test owns it for its
    // whole body.
    let _installed = GPU_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Err(reason) = lichen_compute_gpu::install_default() {
        eprintln!("no device to run on, so the seam is not covered here: {reason}");
        return;
    }

    let source = r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}
k1 = compute.parallel f1 "gpu"
inbuf = (compute.plrun k1 ((compute.A In1)(.n 3, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}
k2 = compute.parallel f2 "gpu"
out = (compute.plrun k2 ((compute.A In2)(.n 3, .I In2(.b inbuf.z))) : Out2)
(compute.read ((compute.Read _)(.from out.w, .at 0)), compute.read ((compute.Read _)(.from out.w, .at 1)), compute.read ((compute.Read _)(.from out.w, .at 2)), compute.collect out.w)
"#;
    let (module, value, _) = run(source);
    // Uninstalling drops the context, so its device buffers go back at once.
    lichen_compute_gpu::uninstall();

    // The element type is **undecided**: the `Int` this used to print rode on an
    // array literal's element cell.
    let elements = common::array_values(&module, &value);
    assert_eq!(common::usize_of(&elements[0]), 20, "the first read");
    assert_eq!(common::usize_of(&elements[1]), 22, "the second read");
    assert_eq!(common::usize_of(&elements[2]), 24, "the third read");
    assert_eq!(
        common::usize_array(&module, &elements[3]),
        vec![20, 22, 24],
        "the collected whole buffer"
    );
    assert_eq!(
        lichen_compute_gpu::installed_backend_name(),
        None,
        "the backend was uninstalled, so a later test cannot inherit a device"
    );
}

/// Serialises the tests that install a backend; the slot is process-global.
static GPU_SLOT: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The placeholder the backend name is written as, so **one source** serves
/// both backends.
const BACKEND: &str = "BACKEND";

/// **Not a multiple of [`LOCAL_SIZE_X`]:** a wrong element width corrupts the
/// neighbour rather than failing.
const ELEMENT_COUNT: usize = lichen_compute_gpu::LOCAL_SIZE_X as usize + 5;

/// The two backends' answers, or `None` with no device; both runs hold the
/// process-global slot.
fn answer_from_each_backend(
    source: &str,
) -> Option<(
    (Module<LangProgram>, LangValue),
    (Module<LangProgram>, LangValue),
)> {
    let _installed = GPU_SLOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (cpu_module, cpu, _) = run(&source.replace(BACKEND, "cpu"));
    if let Err(reason) = lichen_compute_gpu::install_default() {
        eprintln!(
            "no device to run the second backend on, so the two backends are not compared \
                    here: {reason}"
        );
        return None;
    }
    let (gpu_module, gpu, _) = run(&source.replace(BACKEND, "gpu"));
    // Uninstalling drops the context, so its device buffers go back at once.
    lichen_compute_gpu::uninstall();
    Some(((cpu_module, cpu), (gpu_module, gpu)))
}

/// **The `0.0` anchors the class**: a body with no concrete `Float` operand is
/// lowered as an `Int` fragment.
#[test]
fn a_float_fragment_agrees_across_the_two_backends() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value 1.5))
}}
k1 = compute.parallel f1 "{BACKEND}"
inbuf = (compute.plrun k1 ((compute.A In1)(.n {ELEMENT_COUNT}, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {{
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value 0.0 + a + a))
}}
k2 = compute.parallel f2 "{BACKEND}"
out = (compute.plrun k2 ((compute.A In2)(.n {ELEMENT_COUNT}, .I In2(.b inbuf.z))) : Out2)
(compute.read ((compute.Read _)(.from out.w, .at 0)), compute.read ((compute.Read _)(.from out.w, .at {last})), compute.collect out.w)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered one float fragment differently"
    );
}

/// The same chain over `Int`, where **every element differs from its
/// neighbour**, so a width mistake shows at once.
#[test]
fn an_integer_fragment_agrees_across_the_two_backends() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}}
k1 = compute.parallel f1 "{BACKEND}"
inbuf = (compute.plrun k1 ((compute.A In1)(.n {ELEMENT_COUNT}, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {{
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + a))
}}
k2 = compute.parallel f2 "{BACKEND}"
out = (compute.plrun k2 ((compute.A In2)(.n {ELEMENT_COUNT}, .I In2(.b inbuf.z))) : Out2)
(compute.read ((compute.Read _)(.from out.w, .at 0)), compute.read ((compute.Read _)(.from out.w, .at {last})), compute.collect out.w)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered one integer fragment differently"
    );
}

/// The two backends answer `int2float` differently from the same IR: wasm emits
/// nothing, SPIR-V emits `OpConvertUToF`.
#[test]
fn a_varying_float_element_is_seeded_from_the_index() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value int2float i + 0.5))
}}
k = compute.parallel f "{BACKEND}"
out = (compute.plrun k ((compute.A In)(.n {ELEMENT_COUNT}, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from out.z, .at 0)), compute.read ((compute.Read _)(.from out.z, .at 1)), compute.read ((compute.Read _)(.from out.z, .at {last})), compute.collect out.z)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    let elements = common::array_values(&cpu_module, &cpu);
    assert_eq!(common::float_of(&elements[0]), 0.5, "the first element");
    assert_eq!(common::float_of(&elements[1]), 1.5, "the second element");
    // The element at `last` is that index **plus a half**, like every other one:
    // the body writes `int2float i + 0.5`.
    assert_eq!(
        common::float_of(&elements[2]),
        (ELEMENT_COUNT - 1) as f32 + 0.5,
        "the last element"
    );
    assert_eq!(
        &common::float_array(&cpu_module, &elements[3])[..3],
        &[0.5, 1.5, 2.5],
        "every collected element is its index plus a half"
    );
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered one index-seeded float fragment differently"
    );
}

/// The kernel's own domain makes these the hard cases: its locals keep the
/// classes the author annotated.
enum CrossedScalar {
    Int(usize),
    Float(f32),
}

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
            CrossedScalar::Float(5.0),
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => float2int x)
compute.launch k 5.7
"#,
            CrossedScalar::Int(5),
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => int2float (x > 1.0))
compute.launch k 5.0
"#,
            CrossedScalar::Float(1.0),
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Int => x + float2int 3.7)
compute.launch k 5
"#,
            CrossedScalar::Int(8),
        ),
        (
            r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Float => int2float (float2int (x + 0.5)))
compute.launch k 3.0
"#,
            CrossedScalar::Float(3.0),
        ),
    ];
    for (source, expected) in cases {
        let (module, value, root_ty) = run(source);
        match expected {
            CrossedScalar::Int(n) => {
                assert_eq!(
                    common::usize_of(&value),
                    n,
                    "the kernel program produced the crossed Int"
                );
                assert!(
                    common::type_is_int(&module, root_ty),
                    "the crossed Int is typed Int"
                );
            }
            CrossedScalar::Float(f) => {
                assert_eq!(
                    common::float_of(&value),
                    f,
                    "the kernel program produced the crossed Float"
                );
                assert!(
                    common::type_is_float(&module, root_ty),
                    "the crossed Float is typed Float"
                );
            }
        }
    }
}

/// The price of per-value classes: `Int` data is 32-bit on the GPU and 64-bit
/// on the CPU.
#[test]
fn a_body_may_compute_in_one_class_and_cross() {
    let (module, value, root_ty) = run(r#"
---
  compute = import "compute.lichen"
---
k = compute.jit (x : Int => int2float (x + 1))
compute.launch k 5
"#);
    assert_eq!(
        common::float_of(&value),
        6.0,
        "the integer add stays an integer's"
    );
    assert!(
        common::type_is_float(&module, root_ty),
        "the result is typed Float"
    );

    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value float2int (int2float i + 0.5)))
}}
k = compute.parallel f "{BACKEND}"
out = (compute.plrun k ((compute.A In)(.n {ELEMENT_COUNT}, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from out.z, .at 0)), compute.read ((compute.Read _)(.from out.z, .at 1)), compute.read ((compute.Read _)(.from out.z, .at {last})), compute.collect out.z)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    let elements = common::array_values(&cpu_module, &cpu);
    assert_eq!(common::usize_of(&elements[0]), 0, "the first element");
    assert_eq!(common::usize_of(&elements[1]), 1, "the second element");
    assert_eq!(
        common::usize_of(&elements[2]),
        ELEMENT_COUNT - 1,
        "the last element is the truncation of the index plus a half"
    );
    assert_eq!(
        &common::usize_array(&cpu_module, &elements[3])[..3],
        &[0, 1, 2],
        "every collected element differs from its neighbour"
    );
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered one class crossing differently"
    );
}

/// A mixed-class fragment through both backends; see `docs/notes/floating-point.md` §5.1.
#[test]
fn a_fragment_whose_buffers_are_of_two_classes_agrees_across_the_two_backends() {
    // The second output keeps the input `Int`: every input position takes the first output's class.
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
In1  = struct<.a Int>
Out1 = struct<.z (compute.Buf _)>
Par1 = compute.P (compute.KT _)(.I In1, .O Out1)
f1 = (k : Par1) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 10))
}}
k1 = compute.parallel f1 "{BACKEND}"
inbuf = (compute.plrun k1 ((compute.A In1)(.n {ELEMENT_COUNT}, .I In1(.a 0))) : Out1)
In2  = struct<.b (compute.Buf _)>
Out2 = struct<.w (compute.Buf _), .v (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
f2 = (k : Par2) => {{
  i = compute.range k.n
  a = compute.read ((compute.Read _)(.from k.in.b, .at i))
  (compute.write ((compute.Write _)(.to k.out.w, .at i, .value a + 1)), compute.write ((compute.Write _)(.to k.out.v, .at i, .value int2float a + 0.5)))
}}
k2 = compute.parallel f2 "{BACKEND}"
out = (compute.plrun k2 ((compute.A In2)(.n {ELEMENT_COUNT}, .I In2(.b inbuf.z))) : Out2)
(compute.collect out.w, compute.collect out.v)
"#,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    let cpu_buffers = common::array_values(&cpu_module, &cpu);
    let gpu_buffers = common::array_values(&gpu_module, &gpu);

    // The input was `i + 10`, so this output is `i + 11`.
    let integers = common::usize_array(&cpu_module, &cpu_buffers[0]);
    assert_eq!(integers.len(), ELEMENT_COUNT, "the integer output's length");
    assert_eq!(integers[0], 11, "the first integer element");
    assert_eq!(
        integers[ELEMENT_COUNT - 1],
        ELEMENT_COUNT + 10,
        "the last integer element, past the workgroup multiple"
    );
    assert_eq!(
        integers,
        common::usize_array(&gpu_module, &gpu_buffers[0]),
        "the two backends answered the integer output differently"
    );

    // Read as the typed value: outputs are tagged with their ordinal's
    // class, so this one reads back `Float`.
    let floats = common::float_array(&cpu_module, &cpu_buffers[1]);
    assert_eq!(floats.len(), ELEMENT_COUNT, "the float output's length");
    assert_eq!(floats[0], 10.5, "the first float element");
    assert_eq!(
        floats[ELEMENT_COUNT - 1],
        ELEMENT_COUNT as f32 + 9.5,
        "the last float element, past the workgroup multiple"
    );
    assert_eq!(
        floats,
        common::float_array(&gpu_module, &gpu_buffers[1]),
        "the two backends answered the float output differently"
    );
}

/// A **cross-kernel call inside a parallel body**, through both backends.
///
/// # Invariant
/// The callee is a separate kernel, so neither backend resolves the call from
/// the calling fragment: both need the callee's set and position, derived once
/// as the shared `LaunchSet` — comparing the two backends' answers is what
/// asserts it. Only the index is a value a parallel body can name, so `k0` is
/// called with it.
#[test]
fn a_cross_kernel_call_agrees_across_the_two_backends() {
    let source = format!(
        r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit ((y : Int) => y + 1)
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (compute.launch k0 i)))
}}
k = compute.parallel f "{BACKEND}"
out = (compute.plrun k ((compute.A In)(.n {ELEMENT_COUNT}, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from out.z, .at 0)), compute.read ((compute.Read _)(.from out.z, .at 1)), compute.read ((compute.Read _)(.from out.z, .at {last})), compute.collect out.z)
"#,
        last = ELEMENT_COUNT - 1,
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    let elements = common::array_values(&cpu_module, &cpu);
    assert_eq!(common::usize_of(&elements[0]), 1, "k0(0)");
    assert_eq!(common::usize_of(&elements[1]), 2, "k0(1)");
    assert_eq!(
        common::usize_of(&elements[2]),
        ELEMENT_COUNT,
        "k0 of the last index, past the workgroup multiple"
    );
    assert_eq!(
        common::usize_array(&cpu_module, &elements[3]),
        (1..=ELEMENT_COUNT).collect::<Vec<usize>>(),
        "every element is its own index plus one"
    );
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered a cross-kernel call differently"
    );
}

/// A `@loop` inside a kernel, lowered as a **nest**, on both backends.
///
/// # Invariant
/// The carried state is a run-time value, so the checker cannot expand the
/// recursion: the kernel reader builds the nest — the header's `params` are
/// the state, the base test branches out, the exit hands the value on
/// (`docs/notes/loop-conversion.md` §8.6).
#[test]
fn a_kernel_loop_nest_carries_a_runtime_state() {
    let source = format!(
        r#"
---
  compute = import "compute.lichen"
---
In0  = struct<.a Int>
Out0 = struct<.z (compute.Buf _)>
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
seed = (k : Par0) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i - i))
}}
kseed = compute.parallel seed "{BACKEND}"
buf = (compute.plrun kseed ((compute.A In0)(.n 1, .I In0(.a 0))) : Out0)
In  = struct<.b (compute.Buf _)>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  @loop pass = s => if s(0) == 0 then s(1) else pass (s(0), s(1))
  i = compute.range 1
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value pass (compute.read ((compute.Read _)(.from k.in.b, .at 0)), 7)))
}}
p = compute.parallel f "{BACKEND}"
out = (compute.plrun p ((compute.A In)(.n 1, .I In(.b buf.z))) : Out)
compute.read ((compute.Read _)(.from out.z, .at 0))
"#
    );
    let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source) else {
        return;
    };
    assert_eq!(
        common::usize_of(&cpu),
        7,
        "the nest left by its base test with the carried value"
    );
    assert!(
        common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
        "the two backends answered one loop nest differently"
    );
}

/// A dynamic reduction on both backends at three lengths: the triangle numbers.
///
/// # Invariant
/// Lane `i`'s trip count is `data[i]`, so the last lane's count is the buffer's
/// length and the answer is `length(length + 1)/2`. **Both arm orders are run**:
/// a loop's exit condition is the spelling's own, so the base-first leg guards a
/// header that leaves from its `if_true` arm
/// (`docs/notes/loop-conversion.md` §8.6 item 6).
#[test]
fn a_kernel_loop_reduces_a_runtime_buffer_length() {
    let recur = "sum_to (s(0) - 1, s(1) + compute.read ((compute.Read _)(.from k.in.b, \
                 .at s(0) - 1)))";
    // Both spellings of one reduction: the base-first leg is the emitter's guard.
    let legs = [
        (
            "continue arm first",
            format!("if s(0) != 0 then {recur} else s(1)"),
            &[4_usize, 7, 600][..],
        ),
        (
            "base arm first",
            format!("if s(0) == 0 then s(1) else {recur}"),
            &[4_usize, 7][..],
        ),
    ];
    for (order, arms, lengths) in legs {
        for &length in lengths {
            let source = format!(
                r#"
---
  compute = import "compute.lichen"
---
In0  = struct<.a Int>
Out0 = struct<.z (compute.Buf _)>
Par0 = compute.P (compute.KT _)(.I In0, .O Out0)
seed = (k : Par0) => {{
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + 1))
}}
kseed = compute.parallel seed "{BACKEND}"
data = (compute.plrun kseed ((compute.A In0)(.n {length}, .I In0(.a 0))) : Out0)
In  = struct<.b (compute.Buf _)>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  @loop sum_to = s => {arms}
  i = compute.range k.n
  count = compute.read ((compute.Read _)(.from k.in.b, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value sum_to (count, 0)))
}}
p = compute.parallel f "{BACKEND}"
out = (compute.plrun p ((compute.A In)(.n {length}, .I In(.b data.z))) : Out)
compute.read ((compute.Read _)(.from out.z, .at {last}))
"#,
                length = length,
                last = length - 1,
            );
            let Some(((cpu_module, cpu), (gpu_module, gpu))) = answer_from_each_backend(&source)
            else {
                return;
            };
            let expected = length * (length + 1) / 2;
            assert_eq!(
                common::usize_of(&cpu),
                expected,
                "the cpu backend's reduction over {} element(s), {}",
                length,
                order
            );
            assert_eq!(
                common::usize_of(&gpu),
                expected,
                "the gpu backend's reduction over {} element(s), {}",
                length,
                order
            );
            assert!(
                common::values_eq((&cpu_module, &cpu), (&gpu_module, &gpu)),
                "the two backends answered one reduction over {} element(s) differently, {}",
                length,
                order
            );
        }
    }
}

/// A body reading a **scalar leaf of its own `.in` struct** (`compute-buffer-wrapper.md`).
#[test]
fn a_body_reads_a_scalar_leaf_of_its_input_struct() {
    // The `"gpu"` dispatch refuses a runtime scalar, so this shape is cpu-only.
    let (module, value, _root) = run(&format!(
        r#"
--- compute = import "compute.lichen" ---
In  = struct<.a Int>
Out = struct<.z (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  a = k.in.a
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value a + i))
}}
k = compute.parallel f "cpu"
out = (compute.plrun k ((compute.A In)(.n {ELEMENT_COUNT}, .I In(.a 7))) : Out)
compute.collect out.z
"#,
    ));
    assert_eq!(
        common::usize_array(&module, &value),
        (0..ELEMENT_COUNT)
            .map(|offset| 7 + offset)
            .collect::<Vec<usize>>(),
        "every lane took the input's scalar leaf, not the index"
    );
}

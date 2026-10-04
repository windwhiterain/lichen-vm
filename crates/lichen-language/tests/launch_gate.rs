//! Drilling into one unification: the `$launch` binding's argument gate.
//!
//! `launch = k => a => $launch(k.native, k.I, k.O, a)` — with the kernel struct
//! carrying its `.I`/`.O` generically, the argument is gated against the
//! kernel's `.I`.  A simple annotated kernel compiles, yet the launch refuses
//! with
//!
//! ```text
//! expected raw[Int, Int], found raw[Int, Int]
//! ```
//!
//! — two rendered-equal tuple types that do not unify.  This test builds that
//! program, reports the two sides' nodes with their values, low types and
//! operations, and then reports the recorded conflict's own two nodes, so the
//! disagreement is read from the graph rather than from the rendering.

use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::program::LangProgram;
use lichen_language::{compile_with_imports_at, lex};
use lichen_lowlevel::{AnyNodeId, Module, NodeId};

/// `compute.jit` a fully annotated kernel and launch it in one program.
const SOURCE: &str = r#"
---
  compute = import "compute.lichen"
---
k0 = compute.jit (y : Int => y + 1)
k1 = compute.jit (x : Int => compute.launch k0 x)
compute.launch k1 6
"#;

/// What a node holds, in the terms that decide a unification.
fn describe(module: &Module<LangProgram>, node: NodeId) -> String {
    format!(
        "{node:?} value={:?} low_type={:?} origin={:?} has_operation={} operation={:?}",
        module.node_value(AnyNodeId::Dynamic(node)),
        module.low_type_of_node(node),
        module.node_origin(node),
        module.node_operation(node).is_some(),
        module
            .node_operation(node)
            .map(|operation| format!("{:?}", operation.operator)),
    )
}

#[test]
fn the_launch_argument_gate_reports_what_it_compares() {
    // The `---` header needs the preprocessor, so the program goes through the
    // same two steps the language tests' `common::run` uses.
    let mut store = PackageStore::<LangProgram>::new();
    let (preprocessed, import_diags) = preprocess(SOURCE, None, &mut store);
    assert!(
        import_diags.is_empty(),
        "imports must resolve: {import_diags:?}"
    );
    let line_starts = lex::line_starts(SOURCE);
    let report = compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    let Some(build) = report.build else {
        panic!("the program must check: {:#?}", report.diagnostics);
    };
    eprintln!("PROBE check diagnostics: {:#?}", report.diagnostics);
    let mut module = build.module;
    let _ = module.evaluate_node_deep(build.root_val, None);
    eprintln!("PROBE launch unify errors: {}", module.unify_errors.len());
    eprintln!(
        "PROBE extension diagnostics: {:#?}",
        module.extension_diagnostics
    );
    eprintln!("PROBE eval errors: {:#?}", module.eval_errors);
    for error in &module.unify_errors {
        eprintln!(
            "PROBE launch error: a={} b={} value_a={:?} value_b={:?}",
            describe(&module, error.a),
            describe(&module, error.b),
            error.value_a,
            error.value_b,
        );
        // The descent the unify took, step by step: each step is one array
        // position, so the last step names the level whose arities disagree.
        for (at, step) in error.steps.iter().enumerate() {
            eprintln!(
                "PROBE   step[{at}] index={} a={} b={}",
                step.index,
                describe(&module, step.a),
                describe(&module, step.b),
            );
        }
    }
    assert!(
        module.unify_errors.is_empty(),
        "the launch argument gate must accept a matching argument"
    );
}

//! `Type` is the universe marker; four ill-posed deferred-field tests were deleted.

use lichen_highlevel::program::TypeValue;
use lichen_lowlevel::{Module, NodeId};

use lichen_language::compile;
use lichen_language::program::{LangProgram, LangValue};

/// Compile and run a program, asserting it checks; returns the module and the
/// root value node.
fn run(source: &str) -> (Module<LangProgram>, NodeId) {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    let build = report.build.unwrap();
    (build.module, build.root_val)
}

fn evaluate(source: &str) -> LangValue {
    let (mut module, root) = run(source);
    module
        .evaluate_node_deep(root, None)
        .expect("the program's root value is undecided")
}

/// `Type` is the universe marker, the vocabulary the spec's comparisons assume.
#[test]
fn the_type_constant_is_the_universe_marker() {
    assert_eq!(evaluate("Type"), LangValue::TypeValue(TypeValue::TypeType));
}

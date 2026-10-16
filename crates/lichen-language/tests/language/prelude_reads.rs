//! What a compiled program's own pass answers, with and without the prelude. See
//! docs/notes/lowlevel-vm.md.

use lichen_language::compile;
use lichen_language::package::PackageStore;
use lichen_language::program::{LangProgram, LangValue};
use lichen_language::run::{PassReport, evaluate_raw_reporting};
use lichen_lowlevel::{AnyNodeId, EvalError, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("language graphs are dynamic"),
    }
}

/// The read failures the module recorded, rendered.
fn read_failures(module: &Module<LangProgram>) -> Vec<String> {
    module
        .eval_errors
        .iter()
        .filter_map(|error| match error {
            EvalError::IndexTarget { target } => {
                let node = dyn_node(*target);
                Some(format!(
                    "IndexTarget {node:?} value={:?}",
                    module.node_value(*target)
                ))
            }
            _ => None,
        })
        .collect()
}

/// A program compiled without imports: the checker's own pass, which the CLI adds
/// the prelude to.
fn own_pass(source: &str) -> (Module<LangProgram>, LangValue) {
    let report = compile(source);
    assert!(report.ok(), "{source:?} checks: {:?}", report.diagnostics);
    let build = report.build.unwrap();
    let mut module = build.module;
    let root = build.root_val;
    let value = module
        .evaluate_node_deep(root, None)
        .expect("the root has a value");
    (module, value)
}

#[test]
fn an_operation_over_an_applied_parameter_reads_nothing_it_should_not() {
    // `x + 0` as the applied body: no read of the parameter may be recorded.
    let (module, value) = own_pass("f = n => (x => x + 0) n; f 3");
    assert!(
        read_failures(&module).is_empty(),
        "no read fails: {:?}",
        read_failures(&module)
    );
    assert_eq!(value.as_enum(), Some(LowValue::USize(3)));
}

#[test]
fn the_cli_path_accepts_a_compiled_body() {
    // The prelude's operators index their operand array, so this is the shape that
    // reaches a library read.
    for source in ["f = n => (x => x) n; f 3", "f = n => (x => 1) n; f 3"] {
        let mut store = PackageStore::<LangProgram>::new();
        let result =
            evaluate_raw_reporting::<LangProgram>(source, None, &mut store, PassReport::Silent);
        assert!(result.is_ok(), "{source:?} runs: {:?}", result.err());
    }
}

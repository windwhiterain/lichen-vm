use std::collections::HashSet;
mod project;

use lichen_core::diagnostic_kind::EqualityError;
use lichen_core::plugin::DiagnosticKind as _;
use lichen_core::plugin::Operator as _;
use lichen_core::plugin::Value as _;
use lichen_core::runtime::diagnostic::Diagnostic;
use lichen_core::value::Array;
use project::DiagnosticKind;
use project::Operator;
use project::Project;
use project::Value;

use lichen_core::runtime::{Module, equation::LocalEquation, operation::Operation, solve::Solver};

#[test]
fn main() {
    let mut module = Module::<Project>::new();
    let n0 = module.add_literal(Value::from_int(1));
    let n1 = module.add_literal(Value::from_int(2));
    let n2 = Array::node(&mut module, [n0, n1]);
    let n3 = module.add_operation(Operation {
        operand: n2,
        operator: Operator::sum(),
    });
    let n4 = module.add_auto();
    module.add_equation(LocalEquation {
        nodes: Box::new([n3, n4]),
    });
    let n5 = Array::node(&mut module, [n4]);
    let n6 = module.add_literal(Value::from_int(4));
    let n7 = Array::node(&mut module, [n6]);
    module.add_equation(LocalEquation {
        nodes: Box::new([n5, n7]),
    });

    let mut solver = Solver::new(&mut module);
    solver.solve();
    let diagnostics = HashSet::from_iter(module.diagnostics.into_iter());
    assert!(
        diagnostics
            .intersection(&EqualityError::from_nodes(&[n3, n4, n6]))
            .next()
            .is_some()
    );
}

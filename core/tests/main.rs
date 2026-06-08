use std::cell::UnsafeCell;
mod project;

use lichen_core::plugin::Operator as _;
use lichen_core::plugin::Value as _;
use lichen_core::runtime::solve::LocalModuleId;
use lichen_core::runtime::value::Evaluation;
use project::Operator;
use project::Project;
use project::Value;

use lichen_core::runtime::{
    Module, equation::LocalEquation, operation::Operation, solve::Solver, value::new_array,
};

#[test]
fn main() {
    let mut module = Module::<Project>::new();
    let a = module.add_literal(Value::from_int(1));
    let b = module.add_literal(Value::from_int(2));
    let array = new_array(&mut module, [a, b].into_iter());
    let c = module.add_literal(Value::from_array(array));
    let d = module.add_operation(Operation {
        operand: c,
        operator: Operator::sum(),
    });
    let e = module.add_auto();
    module.add_equation(LocalEquation {
        nodes: Box::new([d, e]),
    });
    module.add_node(e);
    let mut solver = Solver::new(&mut module);
    solver.solve();
    assert_eq!(
        module.evaluation(&e),
        &Evaluation::Value(Value::from_int(3))
    );
}

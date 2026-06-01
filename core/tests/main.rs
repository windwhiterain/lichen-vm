use std::cell::UnsafeCell;
mod project;

use lichen_core::plugin::Operator as _;
use lichen_core::plugin::Value as _;
use project::Project;
use project::Operator;
use project::Value;

use lichen_core::{
    runtime::{Module, equation::Equation, operation::Operation, solve::Solver, value::new_array},
};

#[test]
fn main() {
    let module_cell = UnsafeCell::new(Module::<Project>::new());
    let module = unsafe { module_cell.get().as_mut_unchecked() };
    let a = module.add_literal(Value::from_int(1));
    let b = module.add_literal(Value::from_int(2));
    let array = new_array(module, [a, b].into_iter());
    let c = module.add_literal(Value::from_array(array));
    let d = module.add_operation(Operation {
        operand: c,
        operator: Operator::sum(),
    });
    let e = module.add_auto();
    module.add_equation(Equation {
        nodes: Box::new([d, e]),
    });
    Solver::<Project>::solve_equations(module.equations.iter());
    let module = unsafe { module_cell.get().as_mut_unchecked() };
    println!("{module:#?}");
    panic!();
}

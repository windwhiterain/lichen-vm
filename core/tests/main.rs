use std::cell::UnsafeCell;

use lichen_core::{
    module::{Module, equation::Equation, operation::Operation, value::Value},
    solver::Solver,
};

#[test]
fn main() {
    let module_ceil = UnsafeCell::new(Module::new());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    let a = module.add_operation(Operation::None, Value::Int(2));
    let b = module.add_operation(Operation::None, Value::AUTO);
    let c = module.add_operation(Operation::None, Value::AUTO);
    module.add_equation(Equation {
        properties: Box::new([b, c]),
    });
    module.add_equation(Equation {
        properties: Box::new([a, b]),
    });
    Solver::solve_equations(module.equations.iter());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    println!("{module:#?}");
    panic!();
}

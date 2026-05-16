use std::cell::UnsafeCell;

use lichen_core::{
    plugin_define::{Operator as _, Value as _},
    runtime::{Module, equation::Equation, operation::Operation, solve::Solver, value::new_array},
};

mod project {
    ::lichen_core::project! {
        lichen_core,
    }
}

use project::OperatorImpl as Operator;
use project::ProjectImpl as Project;
use project::ValueImpl as Value;

#[test]
fn main() {
    let module_ceil = UnsafeCell::new(Module::<Project>::new());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
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
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    println!("{module:#?}");
    panic!();
}

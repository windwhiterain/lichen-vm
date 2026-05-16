use std::cell::UnsafeCell;

use lichen_core::{
    plugin_define::{Value as _}, runtime::{Module, equation::Equation, operation::Operator as _, solve::Solver}, value::Evaluation,
};



mod project{
    ::lichen_core::project!{
        lichen_core,
    }
}

use project::ProjectImpl as Project;
use project::OperatorImpl as Operation;
use project::ValueImpl as Value;

#[test]
fn main() {
    let module_ceil = UnsafeCell::new(Module::<Project>::new());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    let a = module.add_literal(Evaluation::Value(Value::from_int(2)));
    let b = module.add_literal(Evaluation::AUTO);
    let c = module.add_literal(Evaluation::AUTO);
    module.add_equation(Equation {
        operation_ids: Box::new([b, c]),
    });
    module.add_equation(Equation {
        operation_ids: Box::new([a, b]),
    });
    Solver::<Project>::solve_equations(module.equations.iter());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    println!("{module:#?}");
    panic!();
}
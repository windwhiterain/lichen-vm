use std::cell::UnsafeCell;

use lichen_core::{
    plugin_define::{Auto, Value}, runtime::{Module, equation::Equation, operation::Operation, solver::Solver}
};

use crate::project::ValueImpl;



mod project{
    ::lichen_core::project!{
        lichen_core,
    }
}


#[test]
fn main() {
    let module_ceil = UnsafeCell::new(Module::<ValueImpl>::new());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    let a = module.add_operation(Operation::None, ValueImpl::from_int(2));
    let b = module.add_operation(Operation::None, ValueImpl::from_auto(Auto::new()));
    let c = module.add_operation(Operation::None, ValueImpl::from_auto(Auto::new()));
    module.add_equation(Equation {
        properties: Box::new([b, c]),
    });
    module.add_equation(Equation {
        properties: Box::new([a, b]),
    });
    Solver::<ValueImpl>::solve_equations(module.equations.iter());
    let module = unsafe { module_ceil.get().as_mut().unwrap() };
    println!("{module:#?}");
    panic!();
}
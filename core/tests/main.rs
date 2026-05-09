#![feature(unsafe_cell_access)]
use std::cell::UnsafeCell;

use lichen_core::{
    module::{Module, equation::Equation, expr::Expr, value::Value},
    solver::Solver,
};

#[test]
fn main() {
    let module_ceil = UnsafeCell::new(Module::new(Default::default()));
    let module = unsafe { module_ceil.as_mut_unchecked() };
    let a = module.add_expr(Expr::Literal, &[Value::Int(2)]);
    let b = module.add_expr(Expr::Literal, &[Value::AUTO]);
    let c = module.add_expr(Expr::Literal, &[Value::AUTO]);
    module.add_equation(Equation {
        properties: Box::new([Module::value_property(b), Module::value_property(c)]),
    });
    module.add_equation(Equation {
        properties: Box::new([Module::value_property(a), Module::value_property(b)]),
    });
    Solver::solve_equations(module.equations.iter());
    let module = unsafe { module_ceil.as_ref_unchecked() };
    println!("{module:#?}");
    panic!();
}

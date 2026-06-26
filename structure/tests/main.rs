use lichen_core::{
    ast::{Ast as _, AstImpl},
    plugin::{Ast as _, Value as _},
    runtime::{Module, operation::Operation, solve::Solver},
    value::{Array, StringId},
};
use lichen_structure::{
    plugin::{Ast as _, Operator as _, Value as _},
    value::NamedArray,
};

use crate::project::{Ast, Operator, Project, Value};

mod project;

#[test]
fn main() {
    let module = Module::<Project>::new();
    let mut ast = Ast {
        impl_: AstImpl::new(module),
    };
    let unit = ast.module_mut().add_literal(Value::from_unit());
    let string0 = StringId(0);
    let string1 = StringId(1);
    let named_structures = NamedArray::new(ast.module_mut(), [(string0, unit), (string1, unit)]);
    let named_structures = ast
        .module_mut()
        .add_literal(Value::from_named_array(named_structures));
    let int0 = ast.module_mut().add_literal(Value::from_int(0));
    let int1 = ast.module_mut().add_literal(Value::from_int(1));
    let named_values = NamedArray::new(ast.module_mut(), [(string0, int0), (string1, int1)]);
    let named_values = ast
        .module_mut()
        .add_literal(Value::from_named_array(named_values));
    let e0 = ast.add_auto();
    let v0 = ast.value(&e0);
    let s0 = ast.structure(&e0);
    let construct_params = Array::new(ast.module_mut(), [s0, named_values]);
    let construct_params = ast
        .module_mut()
        .add_literal(Value::from_array(construct_params));
    ast.module_mut().operation_mut(&v0).replace(Operation {
        operand: construct_params,
        operator: Operator::construct(),
    });
    ast.module_mut().operation_mut(&s0).replace(Operation {
        operand: named_structures,
        operator: Operator::compose(),
    });

    let e1 = ast.add_literal_structure(Some(Value::from_string(string0)), Some(Value::from_unit()));
    let e2 = ast.add_member(&e0, &e1);
    ast.add_entry(&e2);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();
    let v2 = ast.value(&e2);
    assert_eq!(ast.module().assert_value(&v2), &Value::from_int(0));
    let s2 = ast.structure(&e2);
    assert_eq!(ast.module().assert_value(&s2), &Value::from_unit());
    let e3 = ast.add_literal_structure(Some(Value::from_string(string1)), Some(Value::from_unit()));
    let e4 = ast.add_member(&e0, &e3);
    ast.add_entry(&e4);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();
    let v4 = ast.value(&e4);
    assert_eq!(ast.module().assert_value(&v4), &Value::from_int(1));
    let s4 = ast.structure(&e4);
    assert_eq!(ast.module().assert_value(&s4), &Value::from_unit());
}

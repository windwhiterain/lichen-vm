use lichen_core::{
    ast::{Ast as _, AstImpl},
    plugin::{Ast as _, DiagnosticKind as _, Operator as _, Value as _},
    runtime::{Module, evaluation::Evaluation, operation::Operation, solve::Solver},
    value::{Array, StringId, Table},
};
use lichen_structure::{
    plugin::{Ast as _, DiagnosticKind as _, Operator as _, Value as _},
    value::{NamedArray, Structure},
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
    let named_array = NamedArray::new(ast.module_mut(), [(StringId(0), unit), (StringId(1), unit)]);
    let named_array = ast
        .module_mut()
        .add_literal(Value::from_named_array(named_array));
    let int1 = ast.module_mut().add_literal(Value::from_int(1));
    let int2 = ast.module_mut().add_literal(Value::from_int(2));
    let array = Array::new(ast.module_mut(), [int1, int2]);
    let e0 = ast.add_literal_structure(Some(Value::from_array(array)), None);
    let s0 = ast.structure(&e0);
    ast.module_mut().operation_mut(&s0).replace(Operation {
        operand: named_array,
        operator: Operator::construct(),
    });
    let e1 = ast.add_literal_structure(
        Some(Value::from_string(StringId(0))),
        Some(Value::from_unit()),
    );
    let e2 = ast.add_member(&e0, &e1);
    ast.add_entry(&e2);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();
    let v2 = ast.value(&e2);
    assert_eq!(
        ast.module().evaluation(&v2),
        &Evaluation::Value(Value::from_int(1))
    );
    let s2 = ast.structure(&e2);
    assert_eq!(
        ast.module().evaluation(&s2),
        &Evaluation::Value(Value::from_unit())
    );

    let e3 = ast.add_literal_structure(
        Some(Value::from_string(StringId(1))),
        Some(Value::from_unit()),
    );
    let e4 = ast.add_member(&e0, &e3);
    ast.add_entry(&e4);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();
    let v4 = ast.value(&e4);
    assert_eq!(
        ast.module().evaluation(&v4),
        &Evaluation::Value(Value::from_int(2))
    );
    let s4 = ast.structure(&e4);
    assert_eq!(
        ast.module().evaluation(&s4),
        &Evaluation::Value(Value::from_unit())
    );
}

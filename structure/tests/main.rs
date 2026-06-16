use lichen_core::{
    ast::Ast as _,
    ast::AstImpl,
    plugin::{Ast as _, DiagnosticKind as _, Operator as _, Value as _},
    runtime::{Module, evaluation::Evaluation, solve::Solver},
    value::{Array, StringId, Table},
};
use lichen_structure::{
    plugin::{Ast as _, DiagnosticKind as _, Operator as _, Value as _},
    value::Structure,
};

use crate::project::{Ast, Project, Value};

mod project;

#[test]
fn main() {
    let module = Module::<Project>::new();
    let mut ast = Ast {
        impl_: AstImpl::new(module),
    };
    let unit = ast.module_mut().add_literal(Value::from_unit());
    let structure = Structure {
        table: Table::new(ast.module_mut(), [StringId(0), StringId(1)]),
        components: Array::new(ast.module_mut(), [unit, unit]),
    };
    let int1 = ast.module_mut().add_literal(Value::from_int(1));
    let int2 = ast.module_mut().add_literal(Value::from_int(2));
    let array = Array::new(ast.module_mut(), [int1, int2]);
    let e0 = ast.add_literal_structure(
        Some(Value::from_array(array)),
        Some(Value::from_structure(structure)),
    );
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
}

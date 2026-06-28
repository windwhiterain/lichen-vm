use lichen_core::{
    ast::{Ast as _, AstImpl},
    plugin::{Ast as _, Value as _},
    runtime::{Module, solve::Solver},
    value::{Array, StringId, Table},
};
use lichen_structure::{
    plugin::{Ast as _, Value as _},
    value::{NamedArray, Structure},
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
    let string0 = StringId(0);
    let string1 = StringId(1);
    let named_structures = NamedArray::new(ast.module_mut(), [(string0, unit), (string1, unit)]);
    let e0_named_structures = ast.add_literal_core(Some(Value::from_named_array(named_structures)));
    let e1_composed_structure = ast.add_compose(&e0_named_structures);
    let int0 = ast.module_mut().add_literal(Value::from_int(0));
    let int1 = ast.module_mut().add_literal(Value::from_int(1));
    let named_values = NamedArray::new(ast.module_mut(), [(string0, int0), (string1, int1)]);
    let e2_named_values = ast.add_literal_core(Some(Value::from_named_array(named_values)));
    let e3_instance = ast.add_construct(&e1_composed_structure, &e2_named_values);
    let e4_name0 =
        ast.add_literal_structure(Some(Value::from_string(string0)), Some(Value::from_unit()));
    let e5_member0 = ast.add_member(&e3_instance, &e4_name0);
    let e6_name1 =
        ast.add_literal_structure(Some(Value::from_string(string1)), Some(Value::from_unit()));
    let e7_member1 = ast.add_member(&e3_instance, &e6_name1);

    ast.add_entry(&e5_member0);
    ast.add_entry(&e7_member1);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();

    let v1_composed_structure = ast.value(&e1_composed_structure);
    let v1_target_composed_structure = Value::from_structure(Structure {
        table: Table::new(ast.module_mut(), [string0, string1]),
        components: Array::new(ast.module_mut(), [unit, unit]),
    });
    assert_eq!(
        ast.module().assert_value(&v1_composed_structure),
        &v1_target_composed_structure
    );
    let s3_instance = ast.structure(&e3_instance);
    assert_eq!(
        ast.module().assert_value(&s3_instance),
        &v1_target_composed_structure
    );
    let v5 = ast.value(&e5_member0);
    assert_eq!(ast.module().assert_value(&v5), &Value::from_int(0));
    let s5 = ast.structure(&e5_member0);
    assert_eq!(ast.module().assert_value(&s5), &Value::from_unit());
    let v7 = ast.value(&e7_member1);
    assert_eq!(ast.module().assert_value(&v7), &Value::from_int(1));
    let s7 = ast.structure(&e7_member1);
    assert_eq!(ast.module().assert_value(&s7), &Value::from_unit());
}

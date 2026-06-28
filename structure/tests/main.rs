use lichen_core::{
    ast::{Ast as _, AstImpl},
    plugin::{Ast as _, Value as _},
    runtime::{Module, solve::Solver},
    value::{Array, StringId, Table},
};
use lichen_structure::{
    plugin::{Ast as _, Value as _},
    value::{NameSet, Structure},
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
    let name0 = ast.add_literal_core(Some(Value::from_string(string0)));
    let name1 = ast.add_literal_core(Some(Value::from_string(string1)));
    let name_set = NameSet::new(ast.module_mut(), [string0, string1]);
    let name_set = ast.add_literal_core(Some(Value::from_name_set(name_set)));
    let structures = Array::new(ast.module_mut(), [unit, unit]);
    let structures = ast.add_literal_core(Some(Value::from_array(structures)));
    let composed_structure = ast.add_compose(&name_set, &structures);
    let int0 = Value::from_int(0);
    let int1 = Value::from_int(1);
    let member0 = ast.module_mut().add_literal(int0);
    let member1 = ast.module_mut().add_literal(int1);
    let members = Array::new(ast.module_mut(), [member0, member1]);
    let members = ast.add_literal_core(Some(Value::from_array(members)));
    let instance = ast.add_construct(&composed_structure, &name_set, &members);
    let member0 = ast.add_member(&instance, &name0);
    let member1 = ast.add_member(&instance, &name1);

    ast.add_entry(&member0);
    ast.add_entry(&member1);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();

    let composed_structure_value = ast.value(&composed_structure);
    let composed_structure_value_target = Value::from_structure(Structure {
        table: Table::new(ast.module_mut(), [string0, string1]),
        components: Array::new(ast.module_mut(), [unit, unit]),
    });
    assert_eq!(
        ast.module().assert_value(&composed_structure_value),
        &composed_structure_value_target
    );
    let instance_structure = ast.structure(&instance);
    assert_eq!(
        ast.module().assert_value(&instance_structure),
        &composed_structure_value_target
    );
    let member0_value = ast.value(&member0);
    assert_eq!(
        ast.module().assert_value(&member0_value),
        &Value::from_int(0)
    );
    let member0_structure = ast.structure(&member0);
    assert_eq!(
        ast.module().assert_value(&member0_structure),
        &Value::from_unit()
    );
    let member1_value = ast.value(&member1);
    assert_eq!(
        ast.module().assert_value(&member1_value),
        &Value::from_int(1)
    );
    let member1_structure = ast.structure(&member1);
    assert_eq!(
        ast.module().assert_value(&member1_structure),
        &Value::from_unit()
    );
}

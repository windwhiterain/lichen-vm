mod project;

use lichen_core::ast::Ast as _;
use lichen_core::ast::AstImpl;
use lichen_core::plugin::Ast as _;
use lichen_core::plugin::Value as _;
use lichen_core::runtime::Module;
use lichen_core::runtime::evaluation::Evaluation;
use lichen_core::runtime::solve::Solver;
use project::Ast;
use project::Project;
use project::Value;

#[test]
fn main() {
    let module = Module::<Project>::new();
    let mut ast = Ast {
        impl_: AstImpl::new(module),
    };
    let v0 = ast.module_mut().add_literal(Value::int(1));
    let v1 = ast.module_mut().add_literal(Value::int(2));
    let e0 = ast.add_literal_core(Some(&v0));
    let e1 = ast.add_literal_core(Some(&v1));
    let e2 = ast.add_tuple(&[e0, e1]);
    let e3 = ast.add_sum(&e2);
    ast.add_entry(&e3);
    let mut solver = Solver::new(ast.module_mut());
    solver.solve();
    let v3 = ast.get_value(&e3);
    assert_eq!(
        ast.module().evaluation(&v3),
        &Evaluation::Value(Value::int(3))
    )
}

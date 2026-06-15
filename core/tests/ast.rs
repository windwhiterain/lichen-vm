mod project;

use lichen_core::Ast as _;
use lichen_core::AstImpl;
use lichen_core::plugin::Ast as _;
use lichen_core::plugin::DiagnosticKind as _;
use lichen_core::plugin::Operator as _;
use lichen_core::plugin::Value as _;
use lichen_core::runtime::Module;
use lichen_core::runtime::diagnostic::Diagnostic;
use lichen_core::runtime::diagnostic::EqualityError;
use lichen_core::runtime::solve::Solver;
use lichen_core::runtime::value::Array;
use lichen_core::runtime::value::Evaluation;
use project::Ast;
use project::DiagnosticKind;
use project::Operator;
use project::Project;
use project::Value;

#[test]
fn main() {
    let module = Module::<Project>::new();
    let mut ast = Ast {
        impl_: AstImpl::new(module),
    };
    let e0 = ast.add_literal_core(Some(Value::from_int(1)));
    let e1 = ast.add_literal_core(Some(Value::from_int(2)));
    let e2 = Array::expr::<Project>(&mut ast, [e0, e1]);
    let e3 = ast.add_sum(&e2);
    ast.add_entry(&e3);
    let mut solver = Solver::new(&mut ast.impl_mut().module);
    solver.solve();
    let v3 = ast.value(&e3);
    assert_eq!(
        ast.impl_().module.evaluation(&v3),
        &Evaluation::Value(Value::from_int(3))
    )
}

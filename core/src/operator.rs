use crate::{
    diagnostic_kind::IndexOutOfBounds,
    plugin::{DiagnosticKind as _, Project, Value as _, principal_traits::Operator},
    runtime::{
        diagnostic::Diagnostic,
        operation,
        solve::{AnyNodeId, LocalNodeId, Solver},
    },
    value::{Array, Int, StringId, Table},
};

#[macro_export]
macro_rules! operands {
    ($solver:ident, $operand:ident, $node:ident, $project:ident,[$($variant: ident,)*]) => {{
        let Some(operands) = $operand.array() else {
            panic!("expected array, found: {:#?}", $operand);
        };
        if operands.0.len() != operands!(@count $(,$variant)*) {
            panic!("expeced length: {}, found: {}", operands!(@count $(,$variant)*), operands.0.len());
        }
        let mut operands = operands.0.iter();
        ($({
            let operand = operands.next().unwrap();
            let operand = $solver.solve_node(&$crate::runtime::solve::AnyNodeId::Local(operand.solver_local($node.module())),Some(&$crate::runtime::solve::AnyNodeId::Local(*$node)))?;
            let Some(operand) = $project::Value::$variant(&operand) else {
                panic!("expected variant: {}, found: {:#?}", stringify!($variant),operand);
            };
            *operand
        },)*)
    }};
    (@count) => (0);
    (@count, $variant0: ident $(, $variant1: ident)*) => (1 + operands!(@count $(, $variant1)*));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sum;

impl<P: Project> Operator<P> for Sum {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let Some(operands) = operand.array() else {
            panic!()
        };
        let mut ret = Some(0);
        for operand in operands.0.iter().copied() {
            let Some(value) = solver.solve_node(
                &AnyNodeId::Local(operand.solver_local(node.module())),
                Some(&AnyNodeId::Local(*node)),
            ) else {
                ret = None;
                continue;
            };
            if let Some(ret) = &mut ret {
                *ret += value.int().unwrap();
            }
        }
        ret.map(|x| operation::Some::Value(P::Value::from_int(x)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Index;

impl Index {
    pub fn run<P: Project>(
        _solver: &mut Solver<P>,
        _node: &LocalNodeId,
        array: Array,
        index: Int,
    ) -> Option<operation::Option<P>> {
        let reference_node = array.0.get(index as usize).copied();
        reference_node.map(|x| Some(operation::Some::Ref(x)))
    }
}

impl<P: Project> Operator<P> for Index {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let (array, index) = operands!(solver, operand, node, P, [array, int,]);
        if let Some(ret) = Index::run(solver, node, array, index) {
            ret
        } else {
            solver
                .module_mut(&node.module())
                .diagnostics
                .push(Diagnostic {
                    kind: P::DiagnosticKind::from_index_out_of_bounds(IndexOutOfBounds {
                        index,
                        len: array.0.len(),
                    }),
                    node: node.local(),
                });
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Find;

impl Find {
    pub fn run<P: Project>(table: Table, name: StringId) -> operation::Option<P> {
        let index = *table.0.get(&name)?;
        Some(operation::Some::Value(P::Value::from_int(index as i64)))
    }
}

impl<P: Project> Operator<P> for Find {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let (table, name) = operands!(solver, operand, node, P, [table, string,]);
        let index = *table.0.get(&name)?;
        Some(operation::Some::Value(P::Value::from_int(index as i64)))
    }
}

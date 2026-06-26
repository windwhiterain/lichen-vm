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
    ($solver:ident, $operand:ident, $node:ident, [$($variant_enum: ty=>$variant: ident,)*]) => {{
        let Some(operands) = $operand.array() else {
            return None;
        };
        if operands.0.len() != operands!(@count $(,$variant)*) {
            return None;
        }
        let mut operands = operands.0.iter();
        ($({
            let operand = operands.next().unwrap();
            let operand = $solver.solve_node(&$crate::runtime::solve::AnyNodeId::Local(operand.solver_local($node.module())),Some(&$crate::runtime::solve::AnyNodeId::Local(*$node)))?;
            let Some(operand) = <$variant_enum>::$variant(&operand) else {
                return None;
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
            return None;
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
                *ret += value.int()?;
            }
        }
        ret.map(|x| operation::Some::Value(P::Value::from_int(x)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Index;

impl Index {
    pub fn run<P: Project>(
        solver: &mut Solver<P>,
        node: &LocalNodeId,
        array: Array,
        index: Int,
    ) -> operation::Option<P> {
        if index >= array.0.len() as i64 || index < 0 {
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
            return None;
        }
        let reference_node = *array.0.get(index as usize);
        Some(operation::Some::Ref(reference_node))
    }
}

impl<P: Project> Operator<P> for Index {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let (array, index) = operands!(solver, operand, node, [P::Value=>array, P::Value=>int,]);
        Index::run(solver, node, array, index)
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
        let (table, name) = operands!(solver, operand, node, [P::Value=>table, P::Value=>string,]);
        let index = *table.0.get(&name)?;
        Some(operation::Some::Value(P::Value::from_int(index as i64)))
    }
}

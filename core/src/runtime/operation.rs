use crate::{
    plugin::{Project, Value, principal_traits::Operator as PrincipalOperator},
    runtime::{
        NodeIdLocal,
        solve::{AnyNodeId, LocalNodeId, Solver},
    },
};

use std::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Project> {
    pub operand: NodeIdLocal,
    pub operator: P::Operator,
}

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

impl<P: Project> PrincipalOperator<P> for Sum {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> Option<P::Value> {
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
        ret.map(|x| P::Value::from_int(x))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Index;

impl<P: Project> PrincipalOperator<P> for Index {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> Option<P::Value> {
        let (array, index) = operands!(solver, operand, node, [P::Value=>array, P::Value=>int,]);
        if index >= array.0.len() as i64 || index < 0 {
            return None;
        }
        let reference_node = *array.0.get(index as usize);
        solver.apply_equation(node.module(), &[node.local(), reference_node]);
        solver.solve_node(
            &AnyNodeId::Local(reference_node.solver_local(node.module())),
            Some(&AnyNodeId::Local(*node)),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Find;

impl<P: Project> PrincipalOperator<P> for Find {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> Option<P::Value> {
        let (table, name) = operands!(solver, operand, node, [P::Value=>table, P::Value=>string,]);
        let index = *table.0.get(&name)?;
        Some(P::Value::from_int(index as i64))
    }
}

use crate::{
    plugin::Project,
    plugin::Value,
    plugin::principal_traits::Operator as PrincipalOperator,
    runtime::{NodeId, solve::Solver},
};

use std::fmt::Debug;

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Project> {
    pub operand: NodeId<P>,
    pub operator: P::Operator,
}

macro_rules! operands {
    ($operand:ident, $node:ident, $($variant: path,)*) => {{
        let Some(operands) = $operand.array() else {
            return None;
        };
        if operands.len() != operands!(@count $(,$variant)*) {
            return None;
        }
        let mut operands = operands.iter();
        ($({
            let operand = *operands.next().unwrap();
            let operand = Solver::solve_node(operand.project(),Some($node))?;
            let Some(operand) = $variant(operand) else {
                return None;
            };
            operand
        },)*)
    }};
    (@count) => (0);
    (@count, $variant0: path $(, $variant1: path)*) => (1 + operands!(@count $(, $variant1)*));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sum;

impl<P: Project<Value: Value>> PrincipalOperator<P> for Sum {
    fn run(&self, operand: P::Value, node: crate::runtime::NodeId<P>) -> Option<P::Value> {
        let Some(operands) = operand.array() else {
            return None;
        };
        let mut ret = Some(0);
        for operand in operands.iter().copied() {
            let Some(value) = Solver::solve_node(operand.project(), Some(node)) else {
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

impl<P: Project<Value: Value>> PrincipalOperator<P> for Index {
    fn run(&self, operand: P::Value, node: NodeId<P>) -> Option<P::Value> {
        let (array, index) = operands!(operand, node, P::Value::array, P::Value::int,);
        if index >= array.len() as i64 || index < 0 {
            return None;
        }
        let reference_node = array.get(index as usize).project();
        Solver::solve_equation(&[node, reference_node]);
        Solver::solve_node(reference_node, Some(node))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Find;

impl<P: Project<Value: Value>> PrincipalOperator<P> for Find {
    fn run(&self, operand: P::Value, node: NodeId<P>) -> Option<P::Value> {
        let (table, name) = operands!(operand, node, P::Value::table, P::Value::string,);
        let index = *table.get(name)?;
        Some(P::Value::from_int(index as i64))
    }
}

use crate::plugin::Project;
use crate::plugin::Value;
use lichen_core::operator::{Find, Index};
use lichen_core::{
    operands,
    plugin::{Value as _, principal_traits::Operator},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offset;

impl<P: Project> Operator<P> for Offset
where
    P::Value: Value,
{
    fn run(
        &self,
        solver: &mut lichen_core::runtime::solve::Solver<P>,
        value: &<P as lichen_core::plugin::Project>::Value,
        node: &lichen_core::runtime::solve::LocalNodeId,
    ) -> Option<<P as lichen_core::plugin::Project>::Value> {
        let (structure, name) =
            operands!(solver, value, node, [P::Value=>structure,P::Value=>string,]);
        Find::run::<P>(structure.table, name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component;

impl<P: Project> Operator<P> for Component
where
    P::Value: Value,
{
    fn run(
        &self,
        solver: &mut lichen_core::runtime::solve::Solver<P>,
        value: &<P as lichen_core::plugin::Project>::Value,
        node: &lichen_core::runtime::solve::LocalNodeId,
    ) -> Option<<P as lichen_core::plugin::Project>::Value> {
        let (structure, index) =
            operands!(solver, value, node, [P::Value=>structure,P::Value=>int,]);
        Index::run(solver, node, structure.components, index)
    }
}

use crate::diagnostic_kind::MemberNameRepetition;
use crate::plugin::DiagnosticKind;
use crate::plugin::Project;
use crate::plugin::Value;
use crate::value::Structure;
use lichen_core::operator::{Find, Index};
use lichen_core::runtime::diagnostic::Diagnostic;
use lichen_core::runtime::operation;
use lichen_core::value::Table;
use lichen_core::{
    operands,
    plugin::{Value as _, principal_traits::Operator},
    value::Array,
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
    ) -> operation::Option<P> {
        let (structure, name) =
            operands!(solver, value, node, [P::Value=>structure,P::Value=>string,]);
        Find::run::<P>(structure.table, name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compose;

impl<P: Project> Operator<P> for Compose
where
    P::Value: Value,
    P::DiagnosticKind: DiagnosticKind<P>,
{
    fn run(
        &self,
        solver: &mut lichen_core::runtime::solve::Solver<P>,
        value: &<P as lichen_core::plugin::Project>::Value,
        node: &lichen_core::runtime::solve::LocalNodeId,
    ) -> operation::Option<P> {
        let named_array = value.named_array()?;
        let module = solver.module_mut(&node.module());
        let mut table = Table::uninit(module, named_array.0.len());
        for (i, (name, _)) in named_array.0.iter().enumerate() {
            if let Some(exists_name_index) = table.0.insert(i, *name, i) {
                module.diagnostics.push(Diagnostic {
                    kind: P::DiagnosticKind::from_member_name_repetition(MemberNameRepetition {
                        table,
                        index_1: i,
                        index_2: exists_name_index,
                    }),
                    node: node.local(),
                });
                return None;
            }
        }
        let components = Array::new(module, named_array.0.iter().map(|(_, node)| *node));
        Some(operation::Some::Value(P::Value::from_structure(
            Structure { table, components },
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Construct;

impl<P: Project> Operator<P> for Construct
where
    P::Value: Value,
{
    fn run(
        &self,
        solver: &mut lichen_core::runtime::solve::Solver<P>,
        value: &<P as lichen_core::plugin::Project>::Value,
        node: &lichen_core::runtime::solve::LocalNodeId,
    ) -> operation::Option<P> {
        let (named_array, structure) =
            operands!(solver, value, node, [P::Value=>named_array, P::Value=>structure,]);
        let module = solver.module_mut(&node.module());
        let mut array = Array::uninit(module, structure.table.0.len());
        for (name, element) in named_array.0.iter() {
            let Some(offset) = structure.table.0.get(name) else {
                return None;
            };
            *array.0.get_mut(*offset) = *element;
        }
        Some(operation::Some::Value(P::Value::from_array(array)))
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
    ) -> operation::Option<P> {
        let (structure, index) =
            operands!(solver, value, node, [P::Value=>structure,P::Value=>int,]);
        Index::run(solver, node, structure.components, index)
    }
}

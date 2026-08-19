use crate::diagnostic_kind::MemberNameMissing;
use crate::diagnostic_kind::MemberNameRepetition;
use crate::plugin::DiagnosticKind;
use crate::plugin::Project;
use crate::plugin::Value;
use crate::value::Offsets;
use lichen_core::runtime::diagnostic::Diagnostic;
use lichen_core::runtime::operation;
use lichen_core::value::Table;
use lichen_core::{
    operands,
    plugin::{Value as _, principal_traits::Operator},
    value::Tuple,
};

/// # Input
/// - name set
/// - structures
/// # Output
/// composed structure
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
        operand: &<P as lichen_core::plugin::Project>::Value,
        node: &lichen_core::runtime::solve::LocalNodeId,
    ) -> operation::Option<P> {
        let (names, structures) = operands!(
            solver,
            operand,
            node,
            [P::Value::as_name_set, P::Value::as_tuple,]
        );
        assert_eq!(names.0.len(), structures.0.len());
        let length = names.0.len();
        let module = solver.module_mut(&node.module());
        let mut table = Table::uninit(module, length);
        for (i, name) in names.0.iter().enumerate() {
            if let Some(exists_name_index) = table.0.insert(i, *name, i) {
                module.diagnostics.push(Diagnostic {
                    kind: P::DiagnosticKind::member_name_repetition(MemberNameRepetition {
                        indices: (i, exists_name_index),
                    }),
                    node: node.local(),
                });
                return None;
            }
        }
        Some(operation::Some::Value(P::Value::table(table)))
    }
}

/// # Input
/// - structure
/// - name set
/// # Output
/// offsets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match;

impl<P: Project> Operator<P> for Match
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
        let (structure, names) = operands!(
            solver,
            value,
            node,
            [P::Value::as_table, P::Value::as_name_set,]
        );
        let module = solver.module_mut(&node.module());
        let mut offsets = Offsets::uninit(module, structure.0.len());
        let mut init_mask = vec![None; offsets.0.len()];
        for (i, name) in names.0.iter().enumerate() {
            let Some(offset) = structure.0.get(name).copied() else {
                return None;
            };
            let mask = &mut init_mask[offset];
            if let Some(exists_index) = mask {
                module.diagnostics.push(Diagnostic {
                    kind: P::DiagnosticKind::member_name_repetition(MemberNameRepetition {
                        indices: (i, *exists_index),
                    }),
                    node: node.local(),
                });
            } else {
                *mask = Some(i);
                *offsets.0.get_mut(i).unwrap() = offset;
            }
        }
        for (i, mask) in init_mask.iter().enumerate() {
            if mask.is_none() {
                module.diagnostics.push(Diagnostic {
                    kind: P::DiagnosticKind::member_name_missing(MemberNameMissing { index: i }),
                    node: node.local(),
                });
            }
        }
        Some(operation::Some::Value(P::Value::offsets(offsets)))
    }
}

/// # Input
/// - offsets
/// - items
/// # Output
/// offseted items
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform;

impl<P: Project> Operator<P> for Transform
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
        let (layout, items) = operands!(
            solver,
            value,
            node,
            [P::Value::as_offsets, P::Value::as_tuple,]
        );
        let module = solver.module_mut(&node.module());
        let mut transformeds = Tuple::uninit(module, items.0.len());
        for (i, item) in items.0.iter().enumerate() {
            transformeds
                .0
                .get_uninit(*layout.0.get(i).unwrap())
                .unwrap()
                .write(*item);
        }
        Some(operation::Some::Value(P::Value::tuple(transformeds)))
    }
}

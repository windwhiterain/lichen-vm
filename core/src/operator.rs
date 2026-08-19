use crate::{
    diagnostic_kind::IndexOutOfBounds,
    operator::Index::Dynamic,
    plugin::{DiagnosticKind as _, Project, Value as _, principal_traits::Operator},
    runtime::{
        NodeIdLocal,
        diagnostic::Diagnostic,
        operation,
        solve::{AnyNodeId, LocalModuleId, LocalNodeId, Solver},
    },
    value::{Int, StringId, Table, Tuple},
};

/// # Argument
/// - `solver`: ident
/// - `operand`: ident
/// - `node`: ident
/// - `variants`: [[path, ..]]
#[macro_export]
macro_rules! operands {
    ($solver:ident, $operand:ident, $node:ident, [$($variant: path,)*]) => {{
        let Some(operands) = $operand.as_tuple() else {
            panic!("expected array, found: {:#?}", $operand);
        };
        if operands.0.len() != operands!(@count $(,$variant)*) {
            panic!("expeced length: {}, found: {}", operands!(@count $(,$variant)*), operands.0.len());
        }
        let mut operands = operands.0.iter();
        ($({
            let operand = operands.next().unwrap();
            let operand = $solver.solve_node(&$crate::runtime::solve::AnyNodeId::Local(operand.solver_local($node.module())),Some(&$crate::runtime::solve::AnyNodeId::Local(*$node)))?;
            let Some(operand) = $variant(&operand) else {
                panic!("expected variant: {}, found: {:#?}", stringify!($variant),operand);
            };
            *operand
        },)*)
    }};
    (@count) => (0);
    (@count, $variant0: path $(, $variant1: path)*) => (1 + operands!(@count $(, $variant1)*));
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
        let Some(operands) = operand.as_tuple() else {
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
                *ret += value.as_int().unwrap();
            }
        }
        ret.map(|x| operation::Some::Value(P::Value::int(x)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Index {
    Dynamic { stride: usize, offset: usize },
    Static { index: usize },
}

impl Index {
    pub fn run<P: Project>(
        &self,
        _solver: &mut Solver<P>,
        _node: &LocalNodeId,
        array: &Tuple,
        index: &Int,
    ) -> Option<operation::Option<P>> {
        let reference_node = array
            .0
            .get(match self {
                Self::Dynamic { stride, offset } => {
                    TryInto::<usize>::try_into(*index).ok()? * stride + offset
                }
                Self::Static { index } => *index,
            })
            .copied();
        reference_node.map(|x| Some(operation::Some::Ref(x)))
    }
}

impl Default for Index {
    fn default() -> Self {
        Self::Dynamic {
            stride: 1,
            offset: 0,
        }
    }
}

impl<P: Project> Operator<P> for Index {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let (array, index) = match self {
            Dynamic { .. } => {
                operands!(
                    solver,
                    operand,
                    node,
                    [P::Value::as_tuple, P::Value::as_int,]
                )
            }
            Index::Static { index } => (*operand.as_tuple().unwrap(), *index as i64),
        };
        if let Some(ret) = Index::run(self, solver, node, &array, &index) {
            ret
        } else {
            solver
                .module_mut(&node.module())
                .diagnostics
                .push(Diagnostic {
                    kind: P::DiagnosticKind::index_out_of_bounds(IndexOutOfBounds {
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
    pub fn run<P: Project>(table: Table, name: StringId) -> Option<operation::Option<P>> {
        let index = table.0.get(&name).copied();
        index.map(|index| Some(operation::Some::Value(P::Value::int(index as i64))))
    }
}

impl<P: Project> Operator<P> for Find {
    fn run(
        &self,
        solver: &mut Solver<P>,
        operand: &P::Value,
        node: &LocalNodeId,
    ) -> operation::Option<P> {
        let (table, name) = operands!(
            solver,
            operand,
            node,
            [P::Value::as_table, P::Value::as_string,]
        );
        Self::run(table, name).unwrap()
    }
}

pub fn solve_all<'a, P: Project>(
    solver: &mut Solver<P>,
    module_id: LocalModuleId,
    nodes: impl IntoIterator<Item = &'a NodeIdLocal>,
) -> bool {
    let mut solved = true;
    for name in nodes.into_iter() {
        if solver
            .solve_node(&AnyNodeId::Local(name.solver_local(module_id)), None)
            .is_none()
        {
            solved = false;
        }
    }
    solved
}

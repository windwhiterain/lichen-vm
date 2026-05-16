use std::marker::PhantomData;

use crate::{
    plugin::Project,
    plugin_define::Value,
    runtime::{NodeId, equation::Equation, operation::Operator, value::Evaluation},
};

#[derive(Debug, Default, Clone, Copy)]
pub enum SolveState {
    #[default]
    None,
    Pending {
        is_solving: bool,
        dependencies_count: usize,
    },
    Solved,
}

#[derive(Debug, Default)]
pub struct Solve<P: Project> {
    pub state: SolveState,
    pub dependents: Vec<NodeId<P>>,
}

pub struct Solver<P: Project> {
    _p: PhantomData<P>,
}
impl<P: Project> Solver<P> {
    pub fn set_node_value(node: NodeId<P>, value: P::Value) {
        *node.evaluation_mut() = Evaluation::Value(value);
        for dependent in std::mem::take(node.dependents_mut()) {
            let SolveState::Pending {
                dependencies_count, ..
            } = dependent.solve_state_mut()
            else {
                unreachable!()
            };
            *dependencies_count -= 1;
            Self::solve_node(dependent, None);
        }
    }
    pub fn solve_node(node: NodeId<P>, dependent: Option<NodeId<P>>) -> Option<P::Value> {
        let operation_value = 'operation_value: {
            if let Some(operation) = node.operation() {
                let solve_state = node.solve_state_mut();
                let is_solving = match solve_state {
                    SolveState::None => {
                        *solve_state = SolveState::Pending {
                            is_solving: true,
                            dependencies_count: 0,
                        };
                        let SolveState::Pending { is_solving, .. } = solve_state else {
                            unreachable!()
                        };
                        is_solving
                    }
                    SolveState::Pending {
                        is_solving,
                        dependencies_count,
                    } => {
                        if *is_solving || *dependencies_count > 0 {
                            break 'operation_value None;
                        }
                        is_solving
                    }
                    SolveState::Solved => break 'operation_value None,
                };
                if let Some(param) = Self::solve_node(operation.operand, Some(node)) {
                    if let Some(value) = operation.operator.run(param, operation.operand) {
                        *node.solve_state_mut() = SolveState::Solved;
                        break 'operation_value Some(value);
                    }
                }
                *is_solving = false;
                None
            } else {
                None
            }
        };
        let root = node.root();
        let evaluation = root.evaluation_mut();
        if let Evaluation::Value(value) = evaluation {
            if let Some(operation_value) = operation_value {
                if *value != operation_value {
                    panic!()
                }
                Some(operation_value)
            } else {
                Some(*value)
            }
        } else if let Evaluation::Auto { .. } = evaluation {
            if let Some(operation_value) = operation_value {
                root.set_value(operation_value);
                Some(operation_value)
            } else {
                if let Some(dependent) = dependent {
                    root.dependents_mut().push(dependent);
                    let SolveState::Pending {
                        dependencies_count, ..
                    } = dependent.solve_state_mut()
                    else {
                        unreachable!()
                    };
                    *dependencies_count += 1;
                }
                None
            }
        } else {
            unreachable!()
        }
    }
    pub fn solve_equations<'a>(equations: impl Iterator<Item = &'a Equation<P>>)
    where
        P: 'a,
        P::Value: Value,
    {
        for equation in equations {
            for operation_id in equation.nodes.iter().copied() {
                Self::solve_node(operation_id, None);
            }
            Self::solve_equation(&equation.nodes);
        }
    }
    pub fn solve_equation(nodes: &[NodeId<P>])
    where
        P::Value: Value,
    {
        let (mut max_evaluation, mut max_order, mut max_root) = (
            &mut Evaluation::AUTO.clone(),
            (0, 0),
            *nodes.first().unwrap(),
        );
        for node in nodes.iter().copied() {
            let root = node.root();
            let evaluation = root.evaluation_mut();
            let order = evaluation.evaluation_order();
            if order > max_order {
                max_evaluation = evaluation;
                max_order = order;
                max_root = root;
            }
            if order.0 == 2 {
                break;
            }
        }
        for node in nodes.iter().copied() {
            let root = node.root();
            if root == max_root {
                continue;
            }
            let evaluation = root.evaluation_mut();
            if let Evaluation::Auto { .. } = max_evaluation {
                root.set_ref(max_root);
            } else if let Evaluation::Value(max_value) = *max_evaluation {
                if let Evaluation::Value(value) = evaluation {
                    if let Some(max_array) = max_value.array()
                        && let Some(array) = value.array()
                    {
                        assert!(max_array.len() == array.len());
                        for i in 0..max_array.len() {
                            Self::solve_equation(&[
                                max_array.get(i).project(),
                                array.get(i).project(),
                            ]);
                        }
                    } else {
                        if max_value != *value {
                            panic!()
                        }
                    }
                } else if let Evaluation::Auto { .. } = evaluation {
                    root.set_value(max_value);
                } else {
                    unreachable!()
                }
            } else {
                unreachable!()
            }
        }
    }
}

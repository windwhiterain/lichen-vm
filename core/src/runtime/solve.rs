use std::marker::PhantomData;

use crate::{
    plugin::Project,
    plugin_define::Value,
    runtime::{OperationId, equation::Equation, operation::Operator},
    value::Evaluation,
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
pub struct Operation<P: Project> {
    pub state: SolveState,
    pub dependents: Vec<OperationId<P>>,
}

pub struct Solver<P: Project> {
    _p: PhantomData<P>,
}
impl<P: Project> Solver<P> {
    pub fn set_operation_value(operation_id: OperationId<P>, value: P::Value) {
        *operation_id.evaluation_mut() = Evaluation::Value(value);
        for dependent in std::mem::take(operation_id.dependents_mut()) {
            let SolveState::Pending {
                dependencies_count, ..
            } = dependent.solve_state_mut()
            else {
                unreachable!()
            };
            *dependencies_count -= 1;
            Self::solve_operation(dependent, None);
        }
    }
    pub fn solve_operation(
        operation_id: OperationId<P>,
        dependent: Option<OperationId<P>>,
    ) -> Option<P::Value> {
        let operation_value = 'operation_value: {
            if let Some(operation) = operation_id.operation() {
                let solve_state = operation_id.solve_state_mut();
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
                if let Some(param) = Self::solve_operation(operation.param, Some(operation_id)) {
                    if let Some(value) = operation.operator.run(param, operation.param) {
                        *operation_id.solve_state_mut() = SolveState::Solved;
                        break 'operation_value Some(value);
                    }
                }
                *is_solving = false;
                None
            } else {
                None
            }
        };
        let root = operation_id.root();
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
            for operation_id in equation.operation_ids.iter().copied() {
                Self::solve_operation(operation_id, None);
            }
            Self::solve_equation(&equation.operation_ids);
        }
    }
    pub fn solve_equation(operations: &[OperationId<P>])
    where
        P::Value: Value,
    {
        let (mut max_evaluation, mut max_order, mut max_operation) = (
            &mut Evaluation::AUTO.clone(),
            (0, 0),
            *operations.first().unwrap(),
        );
        for operation in operations.iter().copied() {
            let operation = operation.root();
            let evaluation = operation.evaluation_mut();
            let order = evaluation.evaluation_order();
            if order > max_order {
                max_evaluation = evaluation;
                max_order = order;
                max_operation = operation;
            }
            if order.0 == 2 {
                break;
            }
        }
        for operation in operations.iter().copied() {
            let operation = operation.root();
            if operation == max_operation {
                continue;
            }
            let evaluation = operation.evaluation_mut();
            if let Evaluation::Auto { .. } = max_evaluation {
                operation.set_ref(max_operation);
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
                    operation.set_value(max_value);
                } else {
                    unreachable!()
                }
            } else {
                unreachable!()
            }
        }
    }
}

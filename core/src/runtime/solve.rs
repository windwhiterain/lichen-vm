use std::marker::PhantomData;

use lichen_utils::{erase, erase_mut};

use crate::{
    plugin::{
        DiagnosticKind, Project,
        principal_traits::{Operator, Value},
    },
    runtime::{
        Module, ModuleId, NodeId, NodeIdLocal,
        diagnostic::{Diagnostic, EqualityError},
        equation::{Equation, LocalEquation},
        operation::Operation,
        value::Evaluation,
    },
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
pub struct Solve {
    pub state: SolveState,
    pub dependents: Vec<NodeId>,
}

pub struct Solver<'a, P: Project> {
    pub module: &'a mut Module<P>,
    pub equations: Vec<Equation>,
    pub nodes: Vec<LocalNodeId>,
}

#[derive(Debug, Clone, Copy)]
pub struct LocalModuleId;

impl LocalModuleId {
    pub fn global<P: Project>(&self, solver: &Solver<P>) -> ModuleId {
        solver.module.id()
    }
}

#[derive(Debug)]
pub enum AnyNodeId {
    Local(LocalNodeId),
    Remote(NodeId),
}

impl AnyNodeId {
    pub fn global<P: Project>(&self, solver: &Solver<P>) -> NodeId {
        match self {
            AnyNodeId::Local(local_node) => local_node.global(solver),
            AnyNodeId::Remote(node_id) => *node_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LocalNodeId {
    pub module: LocalModuleId,
    pub local: NodeIdLocal,
}

impl LocalNodeId {
    pub fn global<P: Project>(&self, solver: &Solver<P>) -> NodeId {
        NodeId {
            module: solver.module.id(),
            local: self.local,
        }
    }
    pub fn local(&self) -> NodeIdLocal {
        self.local
    }
    pub fn module(&self) -> LocalModuleId {
        self.module
    }
}

impl<'a, P: Project<DiagnosticKind: DiagnosticKind<P>>> Solver<'a, P> {
    pub fn new(module: &'a mut Module<P>) -> Self {
        let equations = std::mem::take(&mut module.equations)
            .into_iter()
            .map(|x| Equation {
                module: LocalModuleId,
                nodes: x.nodes,
            })
            .collect();
        let nodes = std::mem::take(&mut module.nodes)
            .iter()
            .map(|x| x.solver_local(LocalModuleId))
            .collect();
        Self {
            module,
            equations,
            nodes,
        }
    }
    pub fn solve(&mut self) {
        loop {
            if let Some(node) = self.nodes.pop() {
                self.solve_node(&AnyNodeId::Local(node), None);
            } else if let Some(equation) = self.equations.pop() {
                self.apply_equation(equation.module, &equation.nodes);
            } else {
                break;
            }
        }
    }
    pub fn module(&self, id: &LocalModuleId) -> &Module<P> {
        &self.module
    }
    pub fn module_mut(&mut self, id: &LocalModuleId) -> &mut Module<P> {
        &mut self.module
    }
    pub fn module_of(&self, node: &LocalNodeId) -> &Module<P> {
        self.module(&node.module())
    }
    pub fn module_mut_of(&mut self, node: &LocalNodeId) -> &mut Module<P> {
        self.module_mut(&node.module())
    }
    pub fn node(&self, node: &NodeId) -> AnyNodeId {
        if self.module.id() == node.module {
            AnyNodeId::Local(LocalNodeId {
                module: LocalModuleId,
                local: node.local,
            })
        } else {
            AnyNodeId::Remote(*node)
        }
    }
    pub fn set_value(&mut self, node: &LocalNodeId, value: P::Value) {
        let module_id = node.module;
        let module = self.module_mut(&module_id);
        for node in module.set_value(&node.local, value) {
            let module = self.module_mut(&module_id);
            for dependent in std::mem::take(&mut module.solve_mut(&node).dependents) {
                if let AnyNodeId::Local(dependent) = self.node(&dependent) {
                    let SolveState::Pending {
                        dependencies_count, ..
                    } = &mut self
                        .module_mut_of(&dependent)
                        .solve_mut(&dependent.local())
                        .state
                    else {
                        unreachable!()
                    };
                    *dependencies_count -= 1;
                } else {
                    todo!()
                }
                self.solve_node(&self.node(&dependent), None);
            }
        }
    }
    pub fn solve_node(
        &mut self,
        node: &AnyNodeId,
        dependent: Option<&AnyNodeId>,
    ) -> Option<P::Value> {
        let self_static = unsafe { erase(self) };
        match node {
            AnyNodeId::Local(node) => {
                let module = self.module_mut_of(node);
                let operation_value = 'operation_value: {
                    if let Some(operation) = *module.operation(&node.local()) {
                        let solve = unsafe { erase_mut(module).solve_mut(&node.local()) };
                        let is_solving = match &mut solve.state {
                            SolveState::None => {
                                solve.state = SolveState::Pending {
                                    is_solving: true,
                                    dependencies_count: 0,
                                };
                                let SolveState::Pending { is_solving, .. } = &mut solve.state
                                else {
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
                        if let Some(param) = self.solve_node(
                            &AnyNodeId::Local(operation.operand.solver_local(node.module())),
                            Some(&AnyNodeId::Local(*node)),
                        ) {
                            if let Some(value) = operation.operator.run(self, &param, node) {
                                solve.state = SolveState::Solved;
                                break 'operation_value Some(value);
                            }
                        }
                        *is_solving = false;
                        None
                    } else {
                        None
                    }
                };
                let module = self.module_mut_of(node);
                let root = module.root(&node.local());
                let evaluation = module.evaluation_mut(&root);
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
                        self.set_value(&mut root.solver_local(node.module()), operation_value);
                        Some(operation_value)
                    } else {
                        if let Some(dependent) = dependent {
                            module
                                .solve_mut(&root)
                                .dependents
                                .push(dependent.global(self_static));
                            if let AnyNodeId::Local(dependent) = dependent {
                                let SolveState::Pending {
                                    dependencies_count, ..
                                } = &mut self
                                    .module_mut_of(dependent)
                                    .solve_mut(&dependent.local)
                                    .state
                                else {
                                    unreachable!()
                                };
                                *dependencies_count += 1;
                            }
                        }
                        None
                    }
                } else {
                    unreachable!()
                }
            }
            AnyNodeId::Remote { .. } => todo!(),
        }
    }
    pub fn apply_equation(&mut self, module_id: LocalModuleId, nodes: &[NodeIdLocal]) {
        for node in nodes.iter().copied() {
            self.solve_node(&&AnyNodeId::Local(node.solver_local(module_id)), None);
        }
        let module = self.module_mut(&module_id);
        let (mut max_evaluation, mut max_order, mut max_root) =
            (&Evaluation::AUTO.clone(), (0, 0), *nodes.first().unwrap());
        for node in nodes.iter().copied() {
            let root = module.root(&node);
            let evaluation = unsafe { erase(module.evaluation(&node)) };
            let order = module.evaluation_order(&node);
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
            let module = self.module_mut(&module_id);
            let root = module.root(&node);
            if root == max_root {
                continue;
            }
            let evaluation = module.evaluation_mut(&root);
            if let Evaluation::Auto { .. } = max_evaluation {
                module.set_ref(&root, &max_root);
            } else if let Evaluation::Value(max_value) = *max_evaluation {
                if let Evaluation::Value(value) = *evaluation {
                    if max_value != value {
                        module.diagnostics.push(Diagnostic {
                            kind: P::DiagnosticKind::from_equality_error(EqualityError {
                                expected: max_root,
                            }),
                            node: root,
                        });
                    }
                    max_value.for_field_pairs(&value, |i, j| {
                        self.apply_equation(module_id, &[*i, *j]);
                    });
                } else if let Evaluation::Auto { .. } = evaluation {
                    self.set_value(&root.solver_local(module_id), max_value);
                } else {
                    unreachable!()
                }
            } else {
                unreachable!()
            }
        }
    }
}

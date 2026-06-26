use std::{fmt::Debug, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::{
    plugin::Project,
    runtime::{
        diagnostic::Diagnostic,
        equation::LocalEquation,
        evaluation::Evaluation,
        operation::Operation,
        solve::{LocalModuleId, LocalNodeId, Solve},
    },
};

pub mod diagnostic;
pub mod equation;
pub mod evaluation;
pub mod operation;
pub mod solve;

pub struct Module<P: Project> {
    pub arena: Arena,
    pub operations: StableVec<Option<Operation<P>>>,
    pub evaluations: StableVec<Evaluation<P>>,
    pub solves: Vec<Solve>,
    pub entries: Vec<NodeIdLocal>,
    pub equations: Vec<LocalEquation>,
    pub diagnostics: Vec<Diagnostic<P>>,
}

struct DebugVec<'a, T: Debug>(&'a Vec<T>);

impl<T: Debug> Debug for DebugVec<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.0.iter().enumerate()).finish()
    }
}

impl<P: Project> Debug for Module<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module")
            .field("operations", &self.operations)
            .field("evaluations", &self.evaluations)
            .field("solves", &DebugVec(&self.solves))
            .field("entries", &DebugVec(&self.entries))
            .field("equations", &DebugVec(&self.equations))
            .field("diagnostics", &DebugVec(&self.diagnostics))
            .finish()
    }
}
pub struct Ptr<T>(NonNull<T>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId {
    pub module: ModuleId,
    pub local: NodeIdLocal,
}
impl NodeId {
    pub fn local(&self) -> NodeIdLocal {
        self.local
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeIdLocal(pub usize);
impl NodeIdLocal {
    pub fn global(&self, module: ModuleId) -> NodeId {
        NodeId {
            module,
            local: *self,
        }
    }
    pub fn solver_local(&self, module: LocalModuleId) -> LocalNodeId {
        LocalNodeId {
            module,
            local: *self,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleId(pub *const ());

impl<P: Project> Module<P> {
    pub fn new() -> Self {
        Self {
            arena: Default::default(),
            operations: Default::default(),
            evaluations: Default::default(),
            solves: Default::default(),
            equations: Default::default(),
            entries: Default::default(),
            diagnostics: Default::default(),
        }
    }
    pub fn id(&self) -> ModuleId {
        ModuleId::from_ref(self)
    }

    fn add_node_raw(
        &mut self,
        operation: Option<Operation<P>>,
        value: Evaluation<P>,
    ) -> NodeIdLocal {
        let (local, _) = self.operations.push(operation);
        self.evaluations.push(value);
        self.solves.push(Default::default());
        NodeIdLocal(local)
    }
    pub fn add_operation(&mut self, operation: Operation<P>) -> NodeIdLocal {
        self.add_node_raw(Some(operation), Evaluation::AUTO)
    }
    pub fn add_literal(&mut self, value: P::Value) -> NodeIdLocal {
        self.add_node_raw(None, Evaluation::Value(value))
    }
    pub fn add_auto(&mut self) -> NodeIdLocal {
        self.add_node_raw(None, Evaluation::AUTO)
    }
    pub fn add_equation(&mut self, equation: LocalEquation) {
        self.equations.push(equation);
    }
    pub fn add_entry(&mut self, node: NodeIdLocal) {
        self.entries.push(node);
    }
}

impl<P: Project> Module<P> {
    pub fn operation(&self, node: &NodeIdLocal) -> &Option<Operation<P>> {
        self.operations.get(node.0)
    }
    pub fn evaluation(&self, node: &NodeIdLocal) -> &Evaluation<P> {
        self.evaluations.get(node.0)
    }
    pub fn solve(&self, node: &NodeIdLocal) -> &Solve {
        self.solves.get(node.0).unwrap()
    }
    pub fn operation_mut(&mut self, node: &NodeIdLocal) -> &mut Option<Operation<P>> {
        self.operations.get_mut(node.0)
    }
    pub fn evaluation_mut(&mut self, node: &NodeIdLocal) -> &mut Evaluation<P> {
        self.evaluations.get_mut(node.0)
    }
    pub fn solve_mut(&mut self, node: &NodeIdLocal) -> &mut Solve {
        self.solves.get_mut(node.0).unwrap()
    }

    pub fn assert_value(&self, node: &NodeIdLocal) -> &P::Value {
        let Evaluation::Value(value) = self.evaluation(&self.debug_root(node)) else {
            panic!()
        };
        value
    }
}

impl<T> Ptr<T> {
    pub fn from_ref(r: &T) -> Self {
        Self(NonNull::from_ref(r))
    }
    pub fn as_ref<'a>(self) -> &'a T {
        unsafe { self.0.as_ref() }
    }
}

impl ModuleId {
    pub fn from_ref<P: Project>(r: &Module<P>) -> Self {
        Self(r as *const Module<P> as *const ())
    }
    pub fn local(&self) -> LocalModuleId {
        LocalModuleId
    }
}

impl<T: Debug> Debug for Ptr<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}
impl<T> Clone for Ptr<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> Copy for Ptr<T> {}
impl<T> PartialEq for Ptr<T> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<T> Eq for Ptr<T> {}

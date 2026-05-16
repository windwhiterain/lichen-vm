use std::{fmt::Debug, marker::PhantomData, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::{
    plugin::Project,
    runtime::{equation::Equation, operation::Operation, solve::Solve, value::Evaluation},
};

pub mod equation;
pub mod operation;
pub mod solve;
pub mod switch;

pub mod value;

#[derive(Debug)]
pub struct Module<P: Project> {
    pub arena: Arena,
    pub operations: StableVec<Option<Operation<P>>>,
    pub evaluations: StableVec<Evaluation<P>>,
    pub solves: Vec<Solve<P>>,
    pub equations: StableVec<Equation<P>>,
}
pub struct Ptr<T>(NonNull<T>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeIdRaw {
    pub module: ModuleId,
    pub local: NodeIdLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeId<P: Project> {
    pub module: ModuleId,
    pub local: NodeIdLocal,
    _p: PhantomData<P>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeIdLocal(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleId(pub *mut ());
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub usize);

impl<P: Project> Module<P> {
    pub fn new() -> Self {
        Self {
            arena: Default::default(),
            operations: Default::default(),
            evaluations: Default::default(),
            solves: Default::default(),
            equations: Default::default(),
        }
    }
    fn add_node_raw(&mut self, operation: Option<Operation<P>>, value: Evaluation<P>) -> NodeId<P> {
        let (local, _) = self.operations.push(operation);
        self.evaluations.push(value);
        self.solves.push(Default::default());
        NodeId::new(ModuleId::from_ref(self), NodeIdLocal(local))
    }
    pub fn add_operation(&mut self, operation: Operation<P>) -> NodeId<P> {
        self.add_node_raw(Some(operation), Evaluation::AUTO)
    }
    pub fn add_literal(&mut self, value: P::Value) -> NodeId<P> {
        self.add_node_raw(None, Evaluation::Value(value))
    }
    pub fn add_auto(&mut self) -> NodeId<P> {
        self.add_node_raw(None, Evaluation::AUTO)
    }
    pub fn add_equation(&mut self, equation: Equation<P>) {
        self.equations.push(equation);
    }
}

impl NodeIdRaw {
    pub fn project<P: Project>(self) -> NodeId<P> {
        NodeId::new(self.module, self.local)
    }
}
impl<P: Project> NodeId<P> {
    pub fn new(module: ModuleId, local: NodeIdLocal) -> Self {
        Self {
            module,
            local,
            _p: Default::default(),
        }
    }
    pub fn raw(self) -> NodeIdRaw {
        NodeIdRaw {
            module: self.module,
            local: self.local,
        }
    }
    pub fn operation(self) -> Option<Operation<P>> {
        let module = self.module.as_ref::<P>();
        *module.operations.get(self.local.0)
    }
    pub fn evaluation(self) -> Evaluation<P> {
        let module = self.module.as_ref::<P>();
        *module.evaluations.get(self.local.0)
    }
    pub fn evaluation_mut<'a>(self) -> &'a mut Evaluation<P> {
        let module = self.module.as_mut::<P>();
        module.evaluations.get_mut(self.local.0)
    }
    pub fn solve_state_mut<'a>(self) -> &'a mut solve::SolveState
    where
        P: 'a,
    {
        let module = self.module.as_mut::<P>();
        &mut module.solves.get_mut(self.local.0).unwrap().state
    }
    pub fn dependents_mut<'a>(self) -> &'a mut Vec<NodeId<P>>
    where
        P: 'a,
    {
        let module = self.module.as_mut::<P>();
        &mut module.solves.get_mut(self.local.0).unwrap().dependents
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
        Self(r as *const Module<P> as *const () as *mut ())
    }
    pub fn as_ref<'a, P: Project>(self) -> &'a Module<P> {
        unsafe { &*(self.0 as *mut Module<P>) }
    }
    pub fn as_mut<'a, P: Project>(self) -> &'a mut Module<P> {
        unsafe { &mut *(self.0 as *mut Module<P>) }
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

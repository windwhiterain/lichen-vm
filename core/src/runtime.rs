use std::{fmt::Debug, marker::PhantomData, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::{
    plugin::Project,
    runtime::{equation::Equation, operation::Operation},
    value::Evaluation,
};

pub mod equation;
pub mod operation;
pub mod solve;
pub mod switch;

#[derive(Debug)]
pub struct Module<P: Project> {
    pub arena: Arena,
    pub operations: StableVec<Option<Operation<P>>>,
    pub evaluations: StableVec<Evaluation<P>>,
    pub solve_operations: Vec<solve::Operation<P>>,
    pub equations: StableVec<Equation<P>>,
}
#[derive(Debug, Default)]
pub struct Solving<P: Project> {
    pub is_solving: bool,
    pub dependents: Vec<OperationId<P>>,
    pub dependencies_count: usize,
}
pub struct Ptr<T>(NonNull<T>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationIdRaw {
    pub module: ModuleId,
    pub local: OperationIdLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationId<P: Project> {
    pub module: ModuleId,
    pub local: OperationIdLocal,
    _p: PhantomData<P>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationIdLocal(pub usize);
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
            solve_operations: Default::default(),
            equations: Default::default(),
        }
    }
    fn add_operation_raw(
        &mut self,
        operation: Option<Operation<P>>,
        value: Evaluation<P>,
    ) -> OperationId<P> {
        let (local, _) = self.operations.push(operation);
        self.evaluations.push(value);
        self.solve_operations.push(Default::default());
        OperationId::new(ModuleId::from_ref(self), OperationIdLocal(local))
    }
    pub fn add_operation(&mut self, operation: Operation<P>) -> OperationId<P> {
        self.add_operation_raw(Some(operation), Evaluation::AUTO)
    }
    pub fn add_literal(&mut self, value: Evaluation<P>) -> OperationId<P> {
        self.add_operation_raw(None, value)
    }
    pub fn add_equation(&mut self, equation: Equation<P>) {
        self.equations.push(equation);
    }
}

impl OperationIdRaw {
    pub fn project<P: Project>(self) -> OperationId<P> {
        OperationId::new(self.module, self.local)
    }
}
impl<P: Project> OperationId<P> {
    pub fn new(module: ModuleId, local: OperationIdLocal) -> Self {
        Self {
            module,
            local,
            _p: Default::default(),
        }
    }
    pub fn raw(self) -> OperationIdRaw {
        OperationIdRaw {
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
        &mut module.solve_operations.get_mut(self.local.0).unwrap().state
    }
    pub fn dependents_mut<'a>(self) -> &'a mut Vec<OperationId<P>>
    where
        P: 'a,
    {
        let module = self.module.as_mut::<P>();
        &mut module
            .solve_operations
            .get_mut(self.local.0)
            .unwrap()
            .dependents
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

use std::{fmt::Debug, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::runtime::{equation::Equation, operation::Operation};

pub mod equation;
pub mod operation;
pub mod solver;
pub mod switch;
#[derive(Debug)]
pub struct Module<V> {
    pub arena: Arena,
    pub operations: StableVec<Operation>,
    pub values: StableVec<V>,
    pub equations: StableVec<Equation>,
}
pub struct Ptr<T>(NonNull<T>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationId {
    pub module: ModuleId,
    pub local: OperationIdLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationIdLocal(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleId(pub *mut ());
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub usize);

impl<V: Copy> Module<V> {
    pub fn new() -> Self {
        Self {
            arena: Default::default(),
            operations: Default::default(),
            values: Default::default(),
            equations: Default::default(),
        }
    }
    pub fn add_operation(&mut self, operation: Operation, value: V) -> OperationId {
        let (local, _) = self.operations.push(operation);
        self.values.push(value);
        OperationId {
            module: ModuleId::from_ref(self),
            local: OperationIdLocal(local),
        }
    }
    pub fn add_equation(&mut self, equation: Equation) {
        self.equations.push(equation);
    }
    pub fn value<'a>(operation_id: OperationId) -> V {
        let module = operation_id.module.as_ref();
        *module.values.get(operation_id.local.0)
    }
    pub fn value_mut<'a>(operation_id: OperationId) -> &'a mut V {
        let module = operation_id.module.as_mut();
        module.values.get_mut(operation_id.local.0)
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
    pub fn from_ref<V>(r: &Module<V>) -> Self {
        Self(r as *const Module<V> as *const () as *mut ())
    }
    pub fn as_ref<'a, V>(self) -> &'a Module<V> {
        unsafe { &*(self.0 as *mut Module<V>) }
    }
    pub fn as_mut<'a, V>(self) -> &'a mut Module<V> {
        unsafe { &mut *(self.0 as *mut Module<V>) }
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

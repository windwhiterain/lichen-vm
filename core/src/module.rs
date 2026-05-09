use std::{collections::HashMap, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::module::{equation::Equation, expr::Expr, switch::Switch, value::Value};

pub mod equation;
pub mod expr;
pub mod switch;
pub mod value;
#[derive(Debug)]
pub struct Module {
    pub arena: Arena,
    pub exprs: StableVec<Expr>,
    pub properties: StableVec<Value>,
    pub property_table: HashMap<StringId, usize>,
    pub equations: StableVec<Equation>,
    pub switches: StableVec<Switch>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PtrRaw<T>(T);
pub type Ptr<T> = PtrRaw<NonNull<T>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprId {
    pub module: ModuleId,
    pub local: ExprIdLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleId(pub NonNull<Module>);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExprIdLocal(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyId {
    pub module: ModuleId,
    pub local: PropertyIdLocal,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropertyIdLocal(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchId(pub usize);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StringId(pub usize);

impl Module {
    pub fn new(property_table: HashMap<StringId, usize>) -> Self {
        Self {
            arena: Default::default(),
            exprs: Default::default(),
            properties: Default::default(),
            property_table,
            equations: Default::default(),
            switches: Default::default(),
        }
    }
    pub fn expr_property_count(&self)-> usize{
        self.property_table.len() + 1
    }
    pub fn add_expr(&mut self,expr:Expr,property_values:&[Value])->ExprId{
        debug_assert!(property_values.len() == self.expr_property_count());
        let (local, _) = self.exprs.push(expr);
        for property_value in property_values.iter().copied(){
            self.properties.push(property_value);
        }
        ExprId { module: ModuleId::from_ref(self), local: ExprIdLocal(local) }
    }
    pub fn add_equation(&mut self,equation: Equation){
        self.equations.push(equation);
    }
    pub fn value_property(expr_id: ExprId) -> PropertyId {
        let module = expr_id.module.as_ref();
        PropertyId {
            module: expr_id.module,
            local: PropertyIdLocal(expr_id.local.0 * module.expr_property_count()),
        }
    }
    pub fn property(expr_id: ExprId, name: StringId) -> PropertyId {
        let module = expr_id.module.as_ref();
        PropertyId {
            module: expr_id.module,
            local: PropertyIdLocal(
                expr_id.local.0 * module.expr_property_count()
                    + module.property_table[&name]
                    + 1,
            ),
        }
    }
    pub fn property_value<'a>(property: PropertyId) -> Value {
        let module = property.module.as_ref();
        *module.properties.get(property.local.0)
    }
    pub fn property_value_mut<'a>(property: PropertyId) -> &'a mut Value {
        let module = property.module.as_mut();
        module.properties.get_mut(property.local.0)
    }
}

impl PropertyId {
    pub const DUMMY: Self = Self {
        module: ModuleId(NonNull::dangling()),
        local: PropertyIdLocal(usize::MAX),
    };
}

impl<T> Ptr<T> {
    pub fn from_ref(r: &T)->Self{
        Self(NonNull::from_ref(r))
    }
    pub fn as_ref<'a>(self) -> &'a T {
        unsafe { self.0.as_ref() }
    }
}

impl ModuleId{
    pub fn from_ref(r: &Module)->Self{
        Self(NonNull::from_ref(r))
    }
    pub fn as_ref<'a>(self) -> &'a Module{
        unsafe { self.0.as_ref() }
    }
    pub fn as_mut<'a>(mut self) -> &'a mut Module{
        unsafe { self.0.as_mut() }
    }
}

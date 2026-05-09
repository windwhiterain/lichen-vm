use std::{collections::HashMap, ptr::NonNull};

use lichen_utils::{arena::Arena, stable_vec::StableVec};

use crate::module::{equation::Equation, expr::Expr, switch::Switch, value::Value};

pub mod equation;
pub mod expr;
pub mod switch;
pub mod value;
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
pub type Ptr<T> = PtrRaw<*const T>;
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
    pub fn value_property(expr_id: ExprId) -> PropertyId {
        let module = unsafe { expr_id.module.0.as_ref() };
        PropertyId {
            module: expr_id.module,
            local: PropertyIdLocal(expr_id.local.0 * (module.property_table.len() + 1)),
        }
    }
    pub fn property(expr_id: ExprId, name: StringId) -> PropertyId {
        let module = unsafe { expr_id.module.0.as_ref() };
        PropertyId {
            module: expr_id.module,
            local: PropertyIdLocal(
                expr_id.local.0 * (module.property_table.len() + 1)
                    + module.property_table[&name]
                    + 1,
            ),
        }
    }
    pub fn property_value<'a>(property: PropertyId) -> Value {
        let module = unsafe { property.module.0.as_ref() };
        *module.properties.get(property.local.0)
    }
    pub fn property_value_mut<'a>(mut property: PropertyId) -> &'a mut Value {
        let module = unsafe { property.module.0.as_mut() };
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
    pub fn as_ref<'a>(self) -> &'a T {
        unsafe { self.0.as_ref_unchecked() }
    }
}

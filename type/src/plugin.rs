// This file is @generated. Do not edit by hand.
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![allow(unused_mut)]

pub mod principal_traits {
    pub trait Ast<P: crate::plugin::Project> {
        const PROPERTIES_COUNT: usize;
        fn impl_(&self) -> &::lichen_core::ast::AstImpl<P>;
        fn impl_mut(&mut self) -> &mut ::lichen_core::ast::AstImpl<P>;
    }
}
pub trait Project:
    std::fmt::Debug + Default + Copy + Eq + std::hash::Hash + 'static + ::lichen_core::plugin::Project
{
}
pub trait Value: ::lichen_core::plugin::Value {
    fn int_type(&self) -> bool;
    fn from_int_type() -> Self;
    fn string_type(&self) -> bool;
    fn from_string_type() -> Self;
}
pub trait DiagnosticKind<P: crate::plugin::Project>:
    ::lichen_core::plugin::DiagnosticKind<P>
{
}
pub trait Operator<P: crate::plugin::Project>: ::lichen_core::plugin::Operator<P> {}
pub trait Ast<P: crate::plugin::Project>:
    ::lichen_core::ast::Ast<P> + ::lichen_core::plugin::Ast<P>
{
    fn r#type(&self, expr: &::lichen_core::ast::ExprId) -> ::lichen_core::runtime::NodeIdLocal;
    fn add_literal_type(
        &mut self,
        value: Option<P::Value>,
        r#type: Option<P::Value>,
    ) -> ::lichen_core::ast::ExprId;
}
pub mod expr {}

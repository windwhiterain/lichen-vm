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
pub trait Operator<P: crate::plugin::Project>: ::lichen_core::plugin::Operator<P> {
    fn offset() -> Self;
    fn component() -> Self;
    fn compose() -> Self;
    fn construct() -> Self;
}
pub trait DiagnosticKind<P: crate::plugin::Project>:
    ::lichen_core::plugin::DiagnosticKind<P>
{
    fn member_name_repetition(&self) -> Option<&crate::diagnostic_kind::MemberNameRepetition>;
    fn from_member_name_repetition(data: crate::diagnostic_kind::MemberNameRepetition) -> Self;
    fn member_name_missing(&self) -> Option<&crate::diagnostic_kind::MemberNameMissing>;
    fn from_member_name_missing(data: crate::diagnostic_kind::MemberNameMissing) -> Self;
}
pub trait Value: ::lichen_core::plugin::Value {
    fn named_array(&self) -> Option<&crate::value::NamedArray>;
    fn from_named_array(data: crate::value::NamedArray) -> Self;
    fn name_set(&self) -> Option<&crate::value::NameSet>;
    fn from_name_set(data: crate::value::NameSet) -> Self;
    fn structure(&self) -> Option<&crate::value::Structure>;
    fn from_structure(data: crate::value::Structure) -> Self;
}
pub trait Ast<P: crate::plugin::Project>:
    ::lichen_core::ast::Ast<P> + ::lichen_core::plugin::Ast<P>
{
    fn structure(&self, expr: &::lichen_core::ast::ExprId) -> ::lichen_core::runtime::NodeIdLocal;
    fn add_literal_structure(
        &mut self,
        value: Option<P::Value>,
        structure: Option<P::Value>,
    ) -> ::lichen_core::ast::ExprId;
    fn add_member(
        &mut self,
        instance: &::lichen_core::ast::ExprId,
        name: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId;
    fn add_compose(
        &mut self,
        components: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId;
    fn add_construct(
        &mut self,
        structure: &::lichen_core::ast::ExprId,
        members: &::lichen_core::ast::ExprId,
    ) -> ::lichen_core::ast::ExprId;
}
pub mod expr {
    pub trait member<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &::lichen_core::ast::ExprId,
            instance: &::lichen_core::ast::ExprId,
            name: &::lichen_core::ast::ExprId,
        );
    }
    pub trait compose<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &::lichen_core::ast::ExprId,
            components: &::lichen_core::ast::ExprId,
        );
    }
    pub trait construct<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &::lichen_core::ast::ExprId,
            structure: &::lichen_core::ast::ExprId,
            members: &::lichen_core::ast::ExprId,
        );
    }
}

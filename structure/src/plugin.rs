pub mod principal_traits {}
pub trait Project:
    std::fmt::Debug + Default + Copy + Eq + std::hash::Hash + 'static + ::lichen_core::plugin::Project
{
}
pub trait Operator<P: crate::plugin::Project>:
    ::lichen_core::plugin::principal_traits::Operator<P> + ::lichen_core::plugin::Operator<P>
{
}
pub trait DiagnosticKind<P: crate::plugin::Project>:
    ::lichen_core::plugin::principal_traits::DiagnosticKind<P>
    + ::lichen_core::plugin::DiagnosticKind<P>
{
}
pub trait Value:
    ::lichen_core::plugin::principal_traits::Value + ::lichen_core::plugin::Value
{
    fn named_array(&self) -> Option<&crate::NamedArray>;
    fn from_named_array(data: crate::NamedArray) -> Self;
    fn name_set(&self) -> Option<&crate::NameSet>;
    fn from_name_set(data: crate::NameSet) -> Self;
    fn structure(&self) -> Option<&crate::Structure>;
    fn from_structure(data: crate::Structure) -> Self;
}
pub trait Ast<P: crate::plugin::Project>:
    ::lichen_core::Ast<P> + ::lichen_core::plugin::Ast<P>
{
    fn structure(&self, expr: &::lichen_core::ExprId) -> ::lichen_core::runtime::NodeIdLocal;
    fn add_literal(
        &mut self,
        structure: Option<P::Value>,
        value: Option<P::Value>,
    ) -> ::lichen_core::ExprId;
    fn add_member(
        &mut self,
        structure: &::lichen_core::ExprId,
        name: &::lichen_core::ExprId,
    ) -> ::lichen_core::ExprId;
}
pub mod expr {
    pub trait member<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &::lichen_core::ExprId,
            structure: &::lichen_core::ExprId,
            name: &::lichen_core::ExprId,
        );
    }
}

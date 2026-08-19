// This file is @generated. Do not edit by hand.
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(unused_variables)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![allow(unused_mut)]

pub mod principal_traits {
    pub trait Value: std::fmt::Debug + Copy + Eq + std::hash::Hash {
        fn fields(&self) -> impl Iterator<Item = &crate::runtime::NodeIdLocal> {
            std::iter::empty()
        }
        fn for_fields(&self, mut action: impl FnMut(&crate::runtime::NodeIdLocal)) {
            for i in self.fields() {
                {
                    action(i);
                }
            }
        }
        fn for_field_pairs(
            &self,
            other: &Self,
            mut action: impl FnMut(&crate::runtime::NodeIdLocal, &crate::runtime::NodeIdLocal),
        ) {
            for (i, j) in self.fields().zip(other.fields()) {
                {
                    action(i, j);
                }
            }
        }
    }
    pub trait Operator<P: crate::plugin::Project>: std::fmt::Debug + Copy + Eq {
        fn run(
            &self,
            solver: &mut crate::runtime::solve::Solver<P>,
            operand: &<P as crate::plugin::Project>::Value,
            node: &crate::runtime::solve::LocalNodeId,
        ) -> crate::runtime::operation::Option<P>;
    }
    pub trait DiagnosticKind<P: crate::plugin::Project>:
        std::fmt::Debug + Eq + std::hash::Hash + Clone
    {
        fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    }
    pub trait Ast<P: crate::plugin::Project> {
        const PROPERTIES_COUNT: usize;
        fn impl_(&self) -> &crate::ast::AstImpl<P>;
        fn impl_mut(&mut self) -> &mut crate::ast::AstImpl<P>;
    }
}
pub trait Project: std::fmt::Debug + Default + Copy + Eq + std::hash::Hash + 'static {
    type Value: crate::plugin::principal_traits::Value + crate::plugin::Value;
    type Operator: crate::plugin::principal_traits::Operator<Self> + crate::plugin::Operator<Self>;
    type DiagnosticKind: crate::plugin::principal_traits::DiagnosticKind<Self>
        + crate::plugin::DiagnosticKind<Self>;
    type Ast: crate::plugin::principal_traits::Ast<Self> + crate::plugin::Ast<Self>;
}
pub trait Value {
    fn as_int(&self) -> Option<&crate::value::Int>;
    fn int(data: crate::value::Int) -> Self;
    fn as_string(&self) -> Option<&crate::value::StringId>;
    fn string(data: crate::value::StringId) -> Self;
    fn as_tuple(&self) -> Option<&crate::value::Tuple>;
    fn tuple(data: crate::value::Tuple) -> Self;
    fn as_table(&self) -> Option<&crate::value::Table>;
    fn table(data: crate::value::Table) -> Self;
    fn as_unit(&self) -> bool;
    fn unit() -> Self;
}
pub trait DiagnosticKind<P: crate::plugin::Project> {
    fn as_unequality(&self) -> Option<&crate::diagnostic_kind::Unequality<P>>;
    fn unequality(data: crate::diagnostic_kind::Unequality<P>) -> Self;
    fn as_index_out_of_bounds(&self) -> Option<&crate::diagnostic_kind::IndexOutOfBounds>;
    fn index_out_of_bounds(data: crate::diagnostic_kind::IndexOutOfBounds) -> Self;
}
pub trait Operator<P: crate::plugin::Project> {
    fn as_sum(&self) -> bool;
    fn sum() -> Self;
    fn as_index(&self) -> Option<&crate::operator::Index>;
    fn index(data: crate::operator::Index) -> Self;
    fn as_find(&self) -> bool;
    fn find() -> Self;
}
pub trait Ast<P: crate::plugin::Project>: crate::ast::Ast<P> {
    fn get_value_uninit<'a>(
        &'a self,
        expr: &'a mut crate::value::Tuple,
    ) -> &'a mut crate::runtime::NodeIdLocal;
    fn get_value(&self, expr: &crate::ast::ExprId) -> crate::runtime::NodeIdLocal;
    fn get_value_dynamic(
        &mut self,
        expr: &crate::runtime::NodeIdLocal,
    ) -> crate::runtime::NodeIdLocal;
    fn add_literal_core(
        &mut self,
        value: Option<&crate::runtime::NodeIdLocal>,
    ) -> crate::ast::ExprId;
    fn add_sum(&mut self, addends: &crate::ast::ExprId) -> crate::ast::ExprId;
    fn add_index(
        &mut self,
        array: &crate::ast::ExprId,
        index: &crate::ast::ExprId,
    ) -> crate::ast::ExprId;
    fn add_find(
        &mut self,
        table: &crate::ast::ExprId,
        name: &crate::ast::ExprId,
    ) -> crate::ast::ExprId;
    fn add_tuple<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a crate::ast::ExprId> + Copy,
    ) -> crate::ast::ExprId;
}
pub mod expr {
    pub trait sum<P: crate::plugin::Project> {
        fn build(ast: &mut P::Ast, output: &crate::ast::ExprId, addends: &crate::ast::ExprId);
    }
    pub trait index<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &crate::ast::ExprId,
            array: &crate::ast::ExprId,
            index: &crate::ast::ExprId,
        );
    }
    pub trait find<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &crate::ast::ExprId,
            table: &crate::ast::ExprId,
            name: &crate::ast::ExprId,
        );
    }
    pub trait tuple<P: crate::plugin::Project> {
        fn build<'a>(
            ast: &mut P::Ast,
            output: &crate::ast::ExprId,
            items: impl IntoIterator<Item = &'a crate::ast::ExprId> + Copy,
        );
    }
}

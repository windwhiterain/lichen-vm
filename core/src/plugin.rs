pub mod principal_traits {
    pub trait Value: std::fmt::Debug + Copy + Eq {
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
            value: &<P as crate::plugin::Project>::Value,
            node: &crate::runtime::solve::LocalNodeId,
        ) -> Option<<P as crate::plugin::Project>::Value>;
    }
    pub trait DiagnosticKind<P: crate::plugin::Project>:
        std::fmt::Debug + Eq + std::hash::Hash + Clone
    {
        fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    }
    pub trait Ast<P: crate::plugin::Project> {
        const PROPERTIES_COUNT: usize;
        fn impl_(&self) -> &crate::AstImpl<P>;
        fn impl_mut(&mut self) -> &mut crate::AstImpl<P>;
    }
}
pub trait Project: std::fmt::Debug + Default + Copy + Eq + std::hash::Hash + 'static {
    type Value: crate::plugin::principal_traits::Value + crate::plugin::Value;
    type Operator: crate::plugin::principal_traits::Operator<Self> + crate::plugin::Operator<Self>;
    type DiagnosticKind: crate::plugin::principal_traits::DiagnosticKind<Self>
        + crate::plugin::DiagnosticKind<Self>;
    type Ast: crate::plugin::principal_traits::Ast<Self> + crate::plugin::Ast<Self>;
}
pub trait Operator<P: crate::plugin::Project> {
    fn sum() -> Self;
    fn index() -> Self;
    fn find() -> Self;
}
pub trait Value {
    fn int(&self) -> Option<&crate::runtime::value::Int>;
    fn from_int(data: crate::runtime::value::Int) -> Self;
    fn string(&self) -> Option<&crate::runtime::StringId>;
    fn from_string(data: crate::runtime::StringId) -> Self;
    fn array(&self) -> Option<&crate::runtime::value::Array>;
    fn from_array(data: crate::runtime::value::Array) -> Self;
    fn table(&self) -> Option<&crate::runtime::value::Table>;
    fn from_table(data: crate::runtime::value::Table) -> Self;
    fn unit(&self) -> bool;
    fn from_unit() -> Self;
}
pub trait DiagnosticKind<P: crate::plugin::Project> {
    fn equality_error(&self) -> Option<&crate::runtime::diagnostic::EqualityError>;
    fn from_equality_error(data: crate::runtime::diagnostic::EqualityError) -> Self;
}
pub trait Ast<P: crate::plugin::Project>: crate::Ast<P> {
    fn value(&self, expr: &crate::ExprId) -> crate::runtime::NodeIdLocal;
    fn add_literal_core(&mut self, value: Option<P::Value>) -> crate::ExprId;
    fn add_sum(&mut self, addends: &crate::ExprId) -> crate::ExprId;
    fn add_index(&mut self, array: &crate::ExprId, index: &crate::ExprId) -> crate::ExprId;
    fn add_find(&mut self, table: &crate::ExprId, name: &crate::ExprId) -> crate::ExprId;
}
pub mod expr {
    pub trait sum<P: crate::plugin::Project> {
        fn build(ast: &mut P::Ast, output: &crate::ExprId, addends: &crate::ExprId);
    }
    pub trait index<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &crate::ExprId,
            array: &crate::ExprId,
            index: &crate::ExprId,
        );
    }
    pub trait find<P: crate::plugin::Project> {
        fn build(
            ast: &mut P::Ast,
            output: &crate::ExprId,
            table: &crate::ExprId,
            name: &crate::ExprId,
        );
    }
}

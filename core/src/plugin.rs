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
    pub trait Operator<Project: crate::plugin::Project>: std::fmt::Debug + Copy + Eq {
        fn run(
            &self,
            solver: &mut crate::runtime::solve::Solver<Project>,
            value: &<Project as crate::plugin::Project>::Value,
            node: &crate::runtime::solve::LocalNodeId,
        ) -> Option<<Project as crate::plugin::Project>::Value>;
    }
    pub trait DiagnosticKind<Project: crate::plugin::Project>:
        std::fmt::Debug + Eq + std::hash::Hash + Clone
    {
        fn message(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result;
    }
}
pub trait Project: std::fmt::Debug + Default + Copy + Eq + std::hash::Hash + 'static {
    type Value: self::Value;
    type Operator: self::Operator<Self>;
    type DiagnosticKind: self::DiagnosticKind<Self>;
    type Ast<'a>: Ast<'a, Self>;
}
pub trait Value: self::principal_traits::Value {
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
pub trait Operator<Project: crate::plugin::Project>:
    self::principal_traits::Operator<Project>
{
    fn sum() -> Self;
    fn index() -> Self;
    fn find() -> Self;
}
pub trait DiagnosticKind<Project: crate::plugin::Project>:
    self::principal_traits::DiagnosticKind<Project>
{
    fn equality_error(&self) -> Option<&crate::runtime::diagnostic::EqualityError>;
    fn from_equality_error(data: crate::runtime::diagnostic::EqualityError) -> Self;
}
pub trait Ast<'a, Project: crate::plugin::Project>: crate::Ast<'a, Project> {
    fn value(&self, expr: &crate::ExprId) -> crate::runtime::NodeIdLocal;
    fn add_literal(&mut self, value: Option<Project::Value>) -> crate::ExprId;
    fn add_sum(&mut self, input: &crate::ExprId) -> crate::ExprId;
    fn add_index(&mut self, input: &crate::ExprId) -> crate::ExprId;
    fn add_find(&mut self, input: &crate::ExprId) -> crate::ExprId;
}

pub mod principal_traits{
pub trait Value<Project: crate::plugin::Project
+>:std::fmt::Debug+Copy+Eq+{
}
pub trait Operator<Project: crate::plugin::Project
+>:std::fmt::Debug+Copy+Eq+{
fn run(& self,value: <Project as crate::plugin::Project
>::Value,node: crate::runtime::NodeId<Project>,)->Option<<Project as crate::plugin::Project
>::Value>;
}
pub trait DiagnosticKind<Project: crate::plugin::Project
+>:std::fmt::Debug+{
fn message(& self,f: &mut std::fmt::Formatter<'_>,)->std::fmt::Result;
}
}
pub trait Project: std::fmt::Debug + Default + Copy + Eq{
type Value<>:self::principal_traits::Value::<Self>;
type Operator<>:self::principal_traits::Operator::<Self>;
type DiagnosticKind<>:self::principal_traits::DiagnosticKind::<Self>;
}
pub trait Value<Project: crate::plugin::Project
+>:self::principal_traits::Value::<Project>{
fn int(self)->Option<crate::runtime::value::Int::<>>;
fn from_int(data: crate::runtime::value::Int::<>)->Self;
fn string(self)->Option<crate::runtime::StringId::<>>;
fn from_string(data: crate::runtime::StringId::<>)->Self;
fn array(self)->Option<crate::runtime::value::Array::<Project>>;
fn from_array(data: crate::runtime::value::Array::<Project>)->Self;
fn table(self)->Option<crate::runtime::value::Table::<>>;
fn from_table(data: crate::runtime::value::Table::<>)->Self;
fn unit(self)->bool;
fn from_unit()->Self;
}
pub trait Operator<Project: crate::plugin::Project
+>:self::principal_traits::Operator::<Project>{
fn sum()->Self;
fn index()->Self;
fn find()->Self;
}
pub trait DiagnosticKind<Project: crate::plugin::Project
+>:self::principal_traits::DiagnosticKind::<Project>{
fn equality_error(self)->Option<crate::runtime::diagnostic::EqualityError::<Project>>;
fn from_equality_error(data: crate::runtime::diagnostic::EqualityError::<Project>)->Self;
}


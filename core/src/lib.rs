pub mod property;
pub mod runtime;
pub mod plugin;

static NAME: &'static str = "lichen_core";

// crate::project! {
//     value{
//         int: crate::runtime::value::Int,
//         string: crate::runtime::StringId,
//         array: crate::runtime::value::Array,
//         table: crate::runtime::value::Table,
//     }{
//         unit: (),
//     }
//     operator{
//         sum:crate::runtime::operation::sum,
//         index:crate::runtime::operation::index,
//         find:crate::runtime::operation::find,
//     }
//     diagnostic_kind{
//         equality_error: crate::runtime::diagnostic::EqualityError,
//     }{}
// }

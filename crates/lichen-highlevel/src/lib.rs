//! The highlevel layer: a typed builder that constructs [`lichen_lowlevel::Module`]s
//! from highlevel expression trees.
//!
//! Layering (language → highlevel → lowlevel): the real language frontend
//! (not built yet) compiles source into an [`ir::ExprTable`]; the
//! [`checker::Checker`] checks it and builds the lowlevel
//! [`lichen_lowlevel::Module`] in one pass — unify is runtime behaviour, so
//! checking happens while building.
//!
//! The IR is dense and id-referenced (no `Box`, no name strings): a use of a
//! parameter is the parameter's own pre-resolved `ExprId`.

pub mod checker;
pub mod diagnostic;
pub mod ir;
pub mod program;

// The value vocabulary is itself an extension point: `#[enum_ext]` on the
// union generates a carrier macro whose body resolves `$crate::HighProgramValue`
// and `$crate::__extend_shape_HighProgramValue` at this crate's root, so both
// must be re-exported here (a downstream crate calls
// `lichen_highlevel::extend_HighProgramValue!` to splice its own variants in).
pub use program::__extend_shape_HighProgramValue;
pub use program::{HighGlobal, HighGlobalExt, HighProgram, HighProgramOperator, HighProgramValue, ValueType};

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
pub mod print;
pub mod program;

// The vocabularies are themselves extension points: a downstream composes
// its own union with `lichen_utils::enum_ext!`, listing every layer's enum
// directly — `+ LowValue as LowValue; + TypeValue as TypeValue;` plus its
// own variants.  Each layer provides a plain enum; nothing nests.
pub use program::{
    HighGlobal, HighGlobalExt, HighProgram, HighProgramOperator, HighProgramValue, TypeOperator,
    TypeValue, ValueType,
};

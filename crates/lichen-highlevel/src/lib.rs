//! The highlevel layer: a typed builder that constructs [`lichen_lowlevel::Module`]s
//! from highlevel expression trees.
//!
//! Layering (language → highlevel → lowlevel): the real language frontend
//! (not built yet) compiles source into an [`ir::IR`]; the
//! [`checker::Checker`] checks it and builds the lowlevel
//! [`lichen_lowlevel::Module`] in one pass — unify is runtime behaviour, so
//! checking happens while building.
//!
//! The IR is dense and id-referenced (no `Box`, no name strings): a use of a
//! parameter is the parameter's own pre-resolved `ExprId`.

pub mod attr;
pub mod checker;
pub mod diagnostic;
pub mod ir;
pub mod native;
pub mod plugin;
pub mod program;

// The vocabularies are themselves extension points: a downstream composes
// its own union with `lichen_utils::enum_ext!`, listing every layer's enum
// directly — `+ LowValue as LowValue; + TypeValue as TypeValue;` plus its
// own variants.  Each layer provides a plain enum; nothing nests.
pub use attr::{AttrExt, AttrSpec, NoAttr};
pub use native::{no_native_ops, NativeApply, NativeArg, NativeOp, NativeOps};
pub use plugin::NativePlugin;
pub use program::{
    Ctx, HighGlobal, HighGlobalExt, HighProgram, HighProgramLiteral, HighProgramOperator,
    HighProgramValue, IntLit, IntTypeLit, LiteralBuild, LiteralExt, ProgramImpl, TypeOperator,
    TypeValue, TypeTypeLit, ValueType,
};

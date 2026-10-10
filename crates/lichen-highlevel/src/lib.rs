//! The highlevel layer: its checker builds the lowlevel `Module` from an
//! [`ir::IR`]. See docs/notes/overview.md.

pub mod attr;
pub mod checker;
pub mod diagnostic;
pub mod ir;
pub mod native;
pub mod plugin;
pub mod program;
pub mod refinement;
pub mod set;
pub mod shape;

// Each layer's vocabulary is an extension point `enum_ext!` composes
// downstream — see docs/notes/attributes.md.
pub use attr::{AttrExt, AttrSet, AttrSpec, NoAttr};
pub use native::{NativeApply, NativeArg, NativeOp, NativeOps, no_native_ops};
pub use plugin::NativePlugin;
pub use program::{
    Ctx, FloatLit, FloatTypeLit, HighGlobal, HighGlobalExt, HighProgram, HighProgramLiteral,
    HighProgramOperator, HighProgramValue, IntLit, IntTypeLit, LiteralBuild, LiteralExt,
    ProgramImpl, TypeOperator, TypeTypeLit, TypeValue, ValueType,
};

//! The highlevel layer: a typed builder that constructs [`lichen_lowlevel::Module`]s
//! from highlevel expression trees.
//!
//! Layering (language → highlevel → lowlevel): the real language frontend
//! (not built yet) compiles source into an [`expr::ExprTable`]; the
//! [`checker::Checker`] checks it and builds the lowlevel
//! [`lichen_lowlevel::Module`] in one pass — unify is runtime behaviour, so
//! checking happens while building.
//!
//! The IR is dense and id-referenced (no `Box`, no name strings): variables
//! are pre-resolved references to binder expressions.

pub mod checker;
pub mod diag;
pub mod expr;
pub mod program;

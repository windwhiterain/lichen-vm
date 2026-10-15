//! The symbolic core of reduction fusion by repair function.
//! See docs/notes/graph-fusion.md §6.
//!
//! # Invariant
//! Every obligation this crate takes on is discharged symbolically or refused by name:
//! a region it cannot prove runs unfused rather than wrongly. The layer is symbolic —
//! it reads expression trees, not `KernelFragment` — so the mathematics can be settled
//! before the bridge to the fragment IR exists.

mod abstraction;
mod display;
mod eval;
mod expr;
mod invert;
mod normalize;
mod operators;
mod random;
mod repair;

pub use abstraction::{Abstraction, abstract_element_function};
pub use eval::{Bindings, evaluate, max_relative_disagreement};
pub use expr::{Expr, FAN_IN_INPUTS, RUNTIME_SCALAR_INPUTS, Variable, substitute_all};
pub use invert::{InvertRefusal, invert};
pub use normalize::{NORMALIZATION_BUDGET, normalize};
pub use operators::{BinaryOp, UnaryOp, cos, exp, ln, relu, sin, sqrt, square};
pub use random::Random;
pub use repair::{
    Assignment, FusedReduction, FusionRefusal, Origin, Reduction, RepairFunction, derive,
    derive_with_repair, distributivity, distributivity_sides,
};

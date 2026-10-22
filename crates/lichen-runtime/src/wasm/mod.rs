//! The wasm backend: fragment sets lowered through `waffle`.
//! See docs/notes/wasm-control-flow.md.
//!
//! # Invariant
//! A backend that runs no wasm takes this crate with `default-features = false`
//! and never resolves it, so the assembler is behind the same feature as the
//! engine that runs what it assembles.

mod assemble;
pub mod lower;
pub mod mixed;
pub mod parallel;
pub mod run;
mod spike;

pub use assemble::assemble_module;
pub use parallel::{HostWords, SEQUENTIAL_PARALLEL_ELEMENTS, run_parallel};
pub use run::{from_wasmi, run_kernel, to_wasmi, value_word, wasmi_zero};

use lichen_kernel_ir::{KernelFragment, ScalarClass};

/// # Invariant
/// A leaf's own class, not the fragment's: an `array{Float, 3}` position is one
/// `f32` local inside a body that may compute integers, and the argument encoding
/// follows the same per-leaf answer.
pub fn param_classes(fragment: &KernelFragment) -> Vec<ScalarClass> {
    fragment.param_shape.leaf_classes()
}
/// # Invariant
/// A wasm import has one signature, so one `read` could not serve both classes:
/// the pair is declared per class, and this is where the name is spelled. A
/// `BFloat16` is spelled its own name and not `f32`'s, so a module in the class
/// fails to link rather than bind the float host import to a half's payload.
pub fn buffer_import_name(class: ScalarClass, kind: &str) -> String {
    let class = match class {
        ScalarClass::Int => "i64",
        ScalarClass::Float => "f32",
        ScalarClass::BFloat16 => "bf16",
    };
    format!("{kind}_{class}")
}

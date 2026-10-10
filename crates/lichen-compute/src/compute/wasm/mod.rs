//! The wasm backend: fragment sets lowered through `waffle`.
//! See docs/notes/wasm-control-flow.md.

mod assemble;
pub(crate) mod lower;
pub(crate) mod mixed;

pub(crate) use assemble::assemble_module;

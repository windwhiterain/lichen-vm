//! A GPU backend for the lowered-kernel IR: a [`KernelFragment`] to a SPIR-V
//! compute shader, dispatched on a real Vulkan device.
//!
//! # Why this is its own crate
//!
//! It depends on [`lichen_kernel_ir`] (the IR) and on `ash` (a Vulkan loader),
//! and **nothing in the workspace depends on it**. The wasm backend in
//! `lichen-compute` does not name this crate, so a build that never dispatches
//! a kernel to a device never links a GPU loader — the same rule that put the IR
//! in a dependency-free crate in the first place.
//!
//! # What runs, and what is refused
//!
//! A fragment becomes a **compute shader** that maps one invocation to one
//! buffer index: invocation `i` handles index `i`, which is exactly the shape of
//! the CPU thread pool's index-based parallel kernel, so the lowered body is used
//! unchanged and its results must match the CPU path bit for bit.
//!
//! A fragment this backend does not handle is **refused by name**, never
//! approximated — see [`spirv::SpirvRefusal`]. A kernel that runs but computes
//! the wrong thing is worse than one that does not run.
//!
//! # The tail-lane obligation
//!
//! A dispatch covers [`spirv::LOCAL_SIZE_X`] invocations per workgroup, so the
//! last workgroup's surplus invocations compute indices the bound buffers do not
//! cover. The shader has no bounds test, and that is deliberate: it is this
//! crate's job, not the shader's, to make the dispatch exact. See
//! [`dispatch::run`].
//!
//! See `docs/notes/lichen-compute-gpu.md` for why the IR split exists and what a
//! second backend was measured to cost.

pub mod dispatch;
pub mod spirv;

pub use dispatch::{GpuContext, RunError, install, install_default, installed_backend_name};
pub use spirv::{Binding, LOCAL_SIZE_X, SpirvRefusal};

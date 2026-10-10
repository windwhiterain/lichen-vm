//! A GPU backend for the lowered-kernel IR: a fragment to a SPIR-V compute shader,
//! dispatched on a Vulkan device.
//!
//! # Invariant
//! Nothing in the workspace depends on this crate, so a build that never dispatches a
//! kernel never links a Vulkan loader. A fragment becomes a compute shader mapping one
//! invocation to one buffer index, so the lowered body is used unchanged and its results
//! must match the CPU path bit for bit; one this backend does not handle is refused by
//! name.

pub mod dispatch;
pub mod spirv;

pub use dispatch::{
    DEFAULT_SLOT_DEPTH, GpuConfig, GpuContext, RunError, install, install_default,
    installed_backend_name, uninstall,
};
pub use spirv::{Binding, LOCAL_SIZE_X, SpirvRefusal};

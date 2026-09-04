//! The `lichen-compute` native plugin: jit-compile a lichen function to a
//! wasm kernel and launch it as a numeric kernel.
//!
//! This plugin is a *compile-time composition*, not a loadable ABI.  It
//! contributes [`ComputeValue`] / [`ComputeOperator`] as sibling leaves of a
//! host program's value/operator vocabularies (composed with
//! [`lichen_utils::enum_ext!`]), and wires the native-call extension point
//! ([`NativeOp`]) over [`JitOp`] / [`LaunchOp`] and the embedded
//! [`WRAPPER_SOURCE`] the same way the reference host (`lichen-language`)
//! does.
//!
//! The whole plugin is **program-generic** so it is a **native plugin** (see
//! [`lichen_highlevel::plugin`]): it never names a concrete host `Program`,
//! its IR, its grammar, or its on-disk format.  Every entry point is bounded
//! by the same set of associated-type constraints a host satisfies whenever
//! its `enum_ext!` vocabulary composes [`LowOperator`], [`TypeOperator`], and
//! [`ComputeOperator`] (its operators) and carries [`ComputeValue`] (its
//! values).  A host composes those leaves and invokes
//! [`compute_native_ops!`] to assemble the plugin's private per-module
//! registry, so the plugin composes cleanly without a circular dependency
//! back onto a specific language crate.

pub mod compute;

pub use lichen_highlevel::native::{NativeOp, NativeOps};

pub use compute::{
    ComputeOperator, ComputePlugin, ComputeValue, JitOp, KernelId, LaunchOp, WRAPPER_SOURCE,
};

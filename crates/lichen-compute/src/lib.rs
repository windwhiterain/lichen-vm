//! The `lichen-compute` compiler plugin: jit-compile a lichen function to a
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
//! The whole plugin is **program-generic**: it never names a concrete host
//! `Program`.  Every entry point is bounded by the same set of
//! associated-type constraints a host satisfies whenever its `enum_ext!`
//! vocabulary composes [`LowOperator`], [`TypeOperator`], and
//! [`ComputeOperator`] (its operators) and carries [`ComputeValue`] (its
//! values).  A host composes those leaves and wires the plugin's native
//! registry itself, so the plugin composes cleanly without a circular
//! dependency back onto a specific language crate.

pub mod compute;

pub use compute::{
    ComputeOperator, ComputeValue, JitOp, KernelId, LaunchOp, WRAPPER_SOURCE,
};

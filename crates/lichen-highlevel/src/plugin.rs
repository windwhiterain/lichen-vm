//! The native-plugin contract.
//!
//! A **native plugin** is a crate that extends the core — [`lichen_utils`],
//! [`lichen_lowlevel`], [`lichen_highlevel`] — through the program-generic
//! extension points, and can be composed into a *fixed* host layer without
//! the host codesigning with it.  Concretely, a native plugin never names the
//! host's concrete `Program` marker, its IR, its grammar, or its on-disk
//! format — so a host can pull the crate and rebuild a compiler without
//! editing the language layer.
//!
//! It contributes, in any combination:
//! - **vocabulary leaves** — value/operator enums the host combines with
//!   [`enum_ext!`](lichen_utils::enum_ext);
//! - **native operators** — `NativeOp` impls, exposed through a
//!   plugin-provided `#[macro_export] macro_rules! <name>_native_ops` that,
//!   for a host program `$program`, expands to a `NativeOps` value (the
//!   host invokes it to build its private per-module registry);
//! - **an attribute** — an `AttrSpec` marker + `AttrExt` provider;
//! - **a `GlobalExt` component** — composed by the host with
//!   [`compose_ext!`](lichen_utils::compose_ext).
//!
//! [`NativePlugin`] is the nominal marker a plugin implements to opt into
//! this contract.  The mechanics are macro-based because *enum composition is
//! inherently a compile-time expansion*, so a plugin set is fixed at build
//! time (a package manager assembles a compiler crate that lists the chosen
//! plugins) rather than loaded at runtime.

/// The nominal marker of a native plugin — opt-in to the native-plugin
/// contract (see the [module docs](self)).
///
/// The marker carries no methods: the mechanics are macro-based (enum
/// composition is a compile-time expansion), so a conforming plugin
/// contributes its vocabulary leaves and a
/// `#[macro_export] macro_rules! <name>_native_ops` for the host to invoke,
/// alongside any `AttrExt` / `GlobalExt` it supplies.  A marker is a unit
/// struct with an explicit `impl NativePlugin for ..`.
pub trait NativePlugin {}

// `NativeOp`/`NativeOps`/`AttrExt` are referenced in the module docs above;
// the lint does not count doc-link usage, so keep them bound with allow.
#[allow(unused_imports)]
use crate::attr::AttrExt as _AttrExtDoc;
#[allow(unused_imports)]
use crate::native::{NativeOp as _NativeOpDoc, NativeOps as _NativeOpsDoc};

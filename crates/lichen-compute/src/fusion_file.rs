//! The fusion file a runner installs, re-exported from the runtime it lives in.
//!
//! # Invariant
//! A file is text a runner hands a pass, read with no language in it, so the reader and the
//! plan it answers are `lichen-runtime`'s. This module is the path `lichen-compute` callers
//! already named, so the code did not move a public path.

pub use lichen_runtime::fusion_file::{
    Action, Budget, FusionFile, FusionFileRefusal, FusionPlan, Override, Pattern, Refusals,
    Strategy, read_fusion_file,
};

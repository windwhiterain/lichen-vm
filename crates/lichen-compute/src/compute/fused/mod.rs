//! A fused kernel as a textual artifact, and the reader that checks it
//! (`docs/notes/graph-fusion.md` §5).
//!
//! # Invariant
//! The artifact is data: `compute` reads a fused kernel with no dependency on the
//! fusion crates and no callback, and the plan travels with the kernel it applies
//! to, so a machine's placement is not lost between compile and emit.

mod convert;
mod schema;

use lichen_kernel_ir::{KernelClaim, KernelFragment, Plan, PlanRefusal};

/// A fused kernel, as an artifact: the fragment, the claim it is interned with,
/// and the plan it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusedKernel {
    /// What the kernel's body compiled to.
    pub fragment: KernelFragment,
    /// The interface the kernel claims, or `None`.
    pub claim: Option<KernelClaim>,
    /// The placement the fragment was checked against.
    pub plan: Plan,
}

/// Why a fused kernel's text was refused.
///
/// # Invariant
/// One variant per way the text fails, and each names what it met: a human edits
/// this file, so a refusal that says only *that* it failed tells them nothing
/// about which field to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusedRefusal {
    /// The text is not JSON this artifact can be read from.
    ///
    /// # Invariant
    /// `path` and `at` are carried rather than folded into the sentence so a
    /// caller can point at the field; a parse error that named neither is the
    /// case this variant exists to rule out.
    Malformed {
        /// The path the offending field sits at, when the parser knew one.
        path: Option<String>,
        /// The byte offset and length the parser pointed at, in the text.
        at: Option<(usize, usize)>,
        /// What the parser said.
        message: String,
    },
    /// The text parsed, and names a value the IR has no room for.
    NotAnIrValue {
        /// The path the offending entry sits at.
        path: Option<String>,
        /// What was found there.
        found: String,
    },
    /// `KernelBody::validate` or `KernelFragment::validate` refused the text.
    FragmentRefused {
        /// The IR's own message.
        message: String,
    },
    /// `KernelFragment::check_plan` refused the plan the text carried.
    PlanRefused {
        /// The IR's own refusal.
        refusal: PlanRefusal,
    },
}

impl FusedRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for FusedRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FusedRefusal::Malformed { path, at, message } => {
                write!(formatter, "the text is not a fused kernel: {message}")?;
                if let Some(path) = path {
                    write!(formatter, " (at `{path}`)")?;
                }
                if let Some((offset, len)) = at {
                    write!(formatter, " (bytes {offset}..{})", offset + len)?;
                }
                Ok(())
            }
            FusedRefusal::NotAnIrValue { path, found } => {
                write!(
                    formatter,
                    "the text names something the kernel IR has no value for: it met {found}"
                )?;
                if let Some(path) = path {
                    write!(formatter, " (at `{path}`)")?;
                }
                Ok(())
            }
            FusedRefusal::FragmentRefused { message } => write!(
                formatter,
                "the fused kernel's fragment was refused before any backend reads it: {message}"
            ),
            FusedRefusal::PlanRefused { refusal } => write!(
                formatter,
                "the fused kernel's plan was refused against its own fragment: {refusal}"
            ),
        }
    }
}

/// Write `kernel` as pretty JSON a human can read and edit.
pub fn write_fused_kernel(kernel: &FusedKernel) -> String {
    let artifact = schema::Artifact::from(kernel);
    facet_json::to_string_pretty(&artifact).expect("a fused kernel is a plain data artifact")
}

/// Read `text` back as the kernel it names, checking it as it goes.
///
/// # Invariant
/// **Pure**: nothing is interned, registered or looked up in a global, so reading
/// the same text twice answers the same kernel in any process. The order is
/// parse, convert, the IR's body gate, the fragment's own gate, `check_plan`:
/// both gates run because each sees facts the other cannot, and a fragment the
/// IR refuses is refused here rather than handed to a backend.
pub fn read_fused_kernel(text: &str) -> Result<FusedKernel, FusedRefusal> {
    let artifact: schema::Artifact = facet_json::from_str(text).map_err(malformed)?;
    let fragment = convert::fragment(artifact.fragment)?;
    let kernel = FusedKernel {
        fragment,
        claim: artifact.claim.map(Into::into),
        plan: artifact.plan.into(),
    };
    kernel
        .fragment
        .body
        .validate()
        .and_then(|()| kernel.fragment.validate())
        .map_err(|message| FusedRefusal::FragmentRefused { message })?;
    kernel
        .fragment
        .check_plan(&kernel.plan)
        .map_err(|refusal| FusedRefusal::PlanRefused { refusal })?;
    Ok(kernel)
}

/// The refusal a `facet_json` parse error earns, with where it happened.
fn malformed(error: facet_json::DeserializeError) -> FusedRefusal {
    let at = error
        .span
        .map(|span| (span.offset as usize, span.len as usize));
    FusedRefusal::Malformed {
        path: error.path.as_ref().map(|path| path.format()),
        at,
        message: error.kind.to_string(),
    }
}

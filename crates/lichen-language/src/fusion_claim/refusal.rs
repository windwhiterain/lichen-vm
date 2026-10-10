//! Why a kernel value's interface claim could not be read.

use std::fmt;

/// The form a claim took that a reader does not carry.
///
/// # Invariant
/// One variant per shape a claim can miss, and the variant names what was met: a reader
/// that cannot see a claim says which field or which value was wrong, because the claim
/// channel is the one place a host learns what a kernel means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The value carries no label at all.
    NoLabel,
    /// The label's payload is not a record of fields.
    PayloadNotARecord,
    /// The payload has no field of that name.
    MissingField {
        /// The field the reader asked for.
        name: &'static str,
    },
    /// A payload field the claim requires is not the shape it must be.
    FieldNotText {
        /// The field that was read.
        name: &'static str,
        /// What the field holds instead.
        found: String,
    },
    /// The labelled value is not a kernel.
    NotAKernel {
        /// What the value is.
        found: String,
    },
    /// No fragment is registered under the kernel's id.
    UnknownKernel {
        /// The id the value named.
        id: usize,
    },
    /// The fragment carries no mark for a role the claim names.
    RoleNotMarked {
        /// The interface the claim names.
        interface: String,
        /// The role that has no mark.
        role: String,
    },
}

impl ClaimRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for ClaimRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClaimRefusal::NoLabel => write!(
                formatter,
                "the value carries no interface claim: attach one with `? Claim(…)`"
            ),
            ClaimRefusal::PayloadNotARecord => write!(
                formatter,
                "the claim's payload is not a record of fields, and a claim names an interface \
                 and the roles it requires"
            ),
            ClaimRefusal::MissingField { name } => {
                write!(formatter, "the claim's payload has no `{name}` field")
            }
            ClaimRefusal::FieldNotText { name, found } => write!(
                formatter,
                "the claim's `{name}` field is {found}, and the claim reads it as text"
            ),
            ClaimRefusal::NotAKernel { found } => write!(
                formatter,
                "the claim rides on {found}, and an interface claim rides on a kernel value"
            ),
            ClaimRefusal::UnknownKernel { id } => write!(
                formatter,
                "kernel {id} is not registered, so the claim's projections have no fragment"
            ),
            ClaimRefusal::RoleNotMarked { interface, role } => write!(
                formatter,
                "the kernel claims {interface} and carries no mark for its `{role}` role"
            ),
        }
    }
}

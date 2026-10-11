//! The marks an expression in a kernel body carried, and the claim a kernel
//! declares. See docs/notes/graph-fusion.md §3.

use crate::ValueId;

/// The payload prefix that makes a mark a **claim** rather than a role name.
///
/// # Invariant
/// The one lexical distinction between the two mark kinds, so a reader classifies a
/// payload by looking at its first characters instead of at the expression it rode.
pub const CLAIM_PREFIX: &str = "claim:";

/// One mark: the body value it was attached to, and the payload it carried.
///
/// # Invariant
/// The value is the *fragment's* value — the same [`ValueId`] the body defines — so a
/// reader carves the marked subexpression out of the body rather than out of the source.
/// The payload is plain text and is never interpreted here: which field names an interface
/// and which a role is the claim's language, not the IR's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelMark {
    /// The body value the marked expression computed.
    pub value: ValueId,
    /// The mark's payload, one entry per field, in the payload's own order.
    pub payload: Vec<MarkField>,
}

/// One field of a mark's payload: the name the payload gave it, and the text it holds.
///
/// # Invariant
/// A payload that is one text and nothing else has one field with no name, so a mark's
/// payload is always a list of texts and a reader that wants "the mark's own text" reads
/// the unnamed field rather than a second variant of [`KernelMark`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkField {
    /// The field's name, as the payload's type names it; `None` for a bare text.
    pub name: Option<String>,
    /// The field's value, as text.
    pub text: String,
}

impl KernelMark {
    /// The text of the payload's field `name`, or `None` when it has none.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.payload
            .iter()
            .find(|field| field.name.as_deref() == Some(name))
            .map(|field| field.text.as_str())
    }

    /// The payload's own text, for a mark whose payload is one text and nothing else.
    pub fn text(&self) -> Option<&str> {
        match self.payload.as_slice() {
            [field] if field.name.is_none() => Some(field.text.as_str()),
            _ => None,
        }
    }

    /// The role this mark names: the payload's own text.
    ///
    /// # Invariant
    /// A mark that is not a claim names its role with its own text, so one reader covers
    /// every role mark and never has to know how the body spelled it.
    pub fn role(&self) -> Option<&str> {
        self.text().filter(|text| !text.starts_with(CLAIM_PREFIX))
    }
}

/// The interface a kernel implements, and the roles that interface requires of its body.
///
/// # Invariant
/// The projections are **not** here: each is a mark on an expression inside the kernel's
/// own body, so a claim cannot diverge from what the kernel computes. A claim names what
/// the kernel must provide; the body says where each one is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelClaim {
    /// The interface's name.
    pub interface: String,
    /// The roles the interface requires, in the order the payload lists them.
    pub roles: Vec<String>,
}

impl KernelClaim {
    /// The claim a mark payload spells: `claim:Reduction(combine,identity)`.
    ///
    /// # Invariant
    /// The interface name and the roles are the two halves the payload spells, and a
    /// payload that spells neither is refused by name: a claim a host cannot read is not a
    /// claim, and the fragment then carries no instance at all.
    pub fn parse(payload: &str) -> Result<Self, ClaimRefusal> {
        let body = payload
            .strip_prefix(CLAIM_PREFIX)
            .ok_or_else(|| ClaimRefusal::NotAClaim {
                text: payload.to_string(),
            })?
            .trim();
        let opened = body
            .find('(')
            .ok_or_else(|| ClaimRefusal::UnreadableClaim {
                text: payload.to_string(),
            })?;
        let roles = &body[opened + 1..];
        let Some(roles) = roles.strip_suffix(')') else {
            return Err(ClaimRefusal::UnreadableClaim {
                text: payload.to_string(),
            });
        };
        let interface = body[..opened].trim();
        if interface.is_empty() || interface.contains(char::is_whitespace) {
            return Err(ClaimRefusal::UnreadableClaim {
                text: payload.to_string(),
            });
        }
        Ok(KernelClaim {
            interface: interface.to_string(),
            roles: roles
                .split(',')
                .map(str::trim)
                .filter(|role| !role.is_empty())
                .map(str::to_string)
                .collect(),
        })
    }

    /// Whether this claim requires the role `role`.
    pub fn requires(&self, role: &str) -> bool {
        self.roles.iter().any(|required| required == role)
    }
}

/// Why a claim could not be read, or a role could not be found.
///
/// # Invariant
/// One variant per way the channel fails, and each names what was met: the claim channel
/// is the one place a host learns what a kernel means, so a reader that cannot see a claim
/// says which part was wrong rather than reporting nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The payload is not a claim: it does not carry the claim prefix.
    NotAClaim {
        /// The payload as it was written.
        text: String,
    },
    /// The payload carries the prefix but does not spell `interface(role,…)`.
    UnreadableClaim {
        /// The payload as it was written.
        text: String,
    },
    /// The kernel claims an interface and its body carries no mark for a role.
    RoleNotMarked {
        /// The interface the claim names.
        interface: String,
        /// The role that has no mark.
        role: String,
    },
    /// The value the claim was read from is not a kernel.
    NotAKernel {
        /// What the value is.
        found: String,
    },
    /// No kernel is registered under the id the value named.
    UnknownKernel {
        /// The id the value named.
        id: usize,
    },
}

impl ClaimRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for ClaimRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimRefusal::NotAClaim { text } => write!(
                formatter,
                "`{text}` is not an interface claim: a claim reads \
                 `claim:Interface(role,…)`"
            ),
            ClaimRefusal::UnreadableClaim { text } => write!(
                formatter,
                "the claim `{text}` is not spelled `claim:Interface(role,…)`"
            ),
            ClaimRefusal::RoleNotMarked { interface, role } => write!(
                formatter,
                "the kernel claims {interface} and its body carries no mark for its `{role}` role"
            ),
            ClaimRefusal::NotAKernel { found } => write!(
                formatter,
                "the claim rides on {found}, and an interface claim rides on a kernel value"
            ),
            ClaimRefusal::UnknownKernel { id } => write!(
                formatter,
                "kernel {id} is not registered, so the claim has no compiled fragment"
            ),
        }
    }
}

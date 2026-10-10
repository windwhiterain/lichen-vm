//! The marks an expression inside a kernel body carried. See docs/notes/graph-fusion.md §3.

use crate::ValueId;

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

    /// The role this mark names: the payload's own text, or its `as` field.
    ///
    /// # Invariant
    /// One reader for both spellings of a role mark — `? "combine"` and
    /// `? Role(.of "Reduction", .as "combine")` name the same role — so a consumer asks by
    /// role and never has to know how the body spelled it.
    pub fn role(&self) -> Option<&str> {
        self.text().or_else(|| self.field("as"))
    }
}

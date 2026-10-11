//! Marks, and the interface instance a kernel's claim assembles with them.
//! See docs/notes/graph-fusion.md §3.
//!
//! # Invariant
//! A mark is **not** an attribute: it is a side table beside the expression tree, so no
//! reader that matches on expression kinds ever meets one, and a mark is never unified,
//! propagated or evaluated. What it carries is text, which is what makes that possible.

use std::collections::HashSet;

use lichen_kernel_ir::{CLAIM_PREFIX, ClaimRefusal, KernelClaim};

use crate::attr::AttrSpec;
use crate::ir::{ExprId, IR};

/// The interface instance one kernel expression declares: the claim, and where each role
/// its body provides is.
///
/// # Invariant
/// **Complete by construction.** The assembly refuses a claim that names a role its body
/// does not mark (see [`IR::assemble_interfaces`]), so an instance that exists has every
/// projection resolved to an expression inside the body the claim is about — a claim cannot
/// diverge from the code it describes, and a reader never has to search for a mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceInstance {
    /// The claim payload as the source wrote it.
    ///
    /// # Invariant
    /// The source's own text, not a re-spelling of [`Self::claim`]: the anchor the lowering
    /// stamps is what the source said, so a reader that echoes the payload echoes the source.
    pub text: String,
    /// The interface and the roles it requires.
    pub claim: KernelClaim,
    /// Each role the claim requires and the expression inside the body that is its
    /// projection, in the claim's own role order.
    pub projections: Vec<Projection>,
}

/// One role of an interface instance, resolved to the expression that is its projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    /// The role's name, as the claim spells it.
    pub role: String,
    /// The marked expression inside the kernel body.
    pub expr: ExprId,
}

impl InterfaceInstance {
    /// The expression this instance's role `role` is, or `None` when the claim asks for a
    /// role the body does not provide.
    pub fn projection(&self, role: &str) -> Option<ExprId> {
        self.projections
            .iter()
            .find(|projection| projection.role == role)
            .map(|projection| projection.expr)
    }
}

impl<A: AttrSpec, L> IR<A, L> {
    /// Record that `expr` carries `payload` as one of its marks.
    ///
    /// # Invariant
    /// Marks accumulate in source order, and a later mark on the same expression appends:
    /// `e ~ "a" ~ "b"` is two marks, never one that replaces the other. A re-record of the
    /// *same* payload is still appended, because the payload is text and the reader that
    /// counts marks is a role lookup, not a tally.
    pub fn add_mark(&mut self, expr: ExprId, payload: String) {
        self.marks.entry(expr).or_default().push(payload);
    }

    /// The marks `expr` carries, in source order; empty when it carries none.
    pub fn marks_of(&self, expr: ExprId) -> &[String] {
        self.marks.get(&expr).map(Vec::as_slice).unwrap_or_default()
    }

    /// Assemble the interface instance each claim-carrying expression declares.
    ///
    /// # Invariant
    /// The assembly is **structural**: it is table lookups over [`Self::marks`] and one walk
    /// of the claimed kernel's own body, so a missing role is a refusal *here* rather than a
    /// reader's discovery later. The assembly decides which marks are *projections*; it does
    /// not decide which marks exist, and a mark it does not name is anchored like any other
    /// ([`crate::checker::Checker`]).
    pub fn assemble_interfaces(&mut self) {
        let claimed: Vec<ExprId> = self
            .marks
            .iter()
            .filter(|(_, payloads)| {
                payloads
                    .iter()
                    .any(|payload| payload.starts_with(CLAIM_PREFIX))
            })
            .map(|(expr, _)| *expr)
            .collect();
        for expr in claimed {
            let payloads = self.marks_of(expr).to_vec();
            for text in payloads
                .iter()
                .filter(|payload| payload.starts_with(CLAIM_PREFIX))
            {
                match self.assemble_one(expr, text) {
                    Ok(instance) => {
                        self.claims.insert(expr, instance);
                    }
                    Err(refusal) => self.claim_refusals.push((expr, refusal)),
                }
            }
        }
    }

    /// Assemble one claim: the roles it requires, resolved inside the kernel body.
    fn assemble_one(&self, kernel: ExprId, text: &str) -> Result<InterfaceInstance, ClaimRefusal> {
        let claim = KernelClaim::parse(text)?;
        let roles = self.body_roles(kernel);
        let mut projections = Vec::with_capacity(claim.roles.len());
        for role in &claim.roles {
            // The refusal is the coverage check: the claim asked for a
            // role the body names none.
            let expr = roles
                .iter()
                .find(|(marked, _)| marked == role)
                .map(|(_, expr)| *expr)
                .ok_or(ClaimRefusal::RoleNotMarked {
                    interface: claim.interface.clone(),
                    role: role.clone(),
                })?;
            projections.push(Projection {
                role: role.clone(),
                expr,
            });
        }
        Ok(InterfaceInstance {
            text: text.to_string(),
            claim,
            projections,
        })
    }

    /// The role each marked expression inside `kernel`'s body names, first one wins.
    ///
    /// # Invariant
    /// The walk starts at the kernel expression's **children**: a projection is an
    /// expression *inside* the body, never the kernel expression itself, and a claim payload
    /// is not a role (it carries the claim prefix). An explicit stack, so a body deeper than
    /// the call stack assembles like any other.
    fn body_roles(&self, kernel: ExprId) -> Vec<(String, ExprId)> {
        let mut found = Vec::new();
        let mut stack = self.children(kernel);
        let mut seen: HashSet<ExprId> = HashSet::new();
        while let Some(expr) = stack.pop() {
            if !seen.insert(expr) {
                continue;
            }
            for payload in self.marks_of(expr) {
                if !payload.starts_with(CLAIM_PREFIX) {
                    found.push((payload.clone(), expr));
                }
            }
            stack.extend(self.children(expr));
        }
        found
    }
}

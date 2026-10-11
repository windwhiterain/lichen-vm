//! The plan: where a fragment's staged values live, and the check that refuses one the fragment
//! refutes. See `docs/notes/graph-fusion.md` §5.

use crate::{KernelFragment, Space, ValueId};

/// One staged value's placement, as a plan states it.
///
/// # Invariant
/// The name is the mark the body published for the value, which is the only durable way to name
/// one: a run's value numbering is reported and never hashed, so a plan that named a value by
/// number would name a different one in the next session. A **count** is deliberately absent — a
/// slice's element count is the body's, and this width is the plan's own group width
/// (`docs/notes/shared-memory-and-barriers.md` §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staging {
    /// The mark the body published for the value, as the text it carried.
    pub name: String,
    /// Where the value lives.
    pub space: Space,
    /// The group width this placement is checked against.
    pub group_width: u32,
}

/// Where a region's staged values live, as a plan states it.
///
/// # Invariant
/// A plan is a **proposal**: a fragment is checked against it rather than silently built from it,
/// so a disagreement is refused by name in either direction rather than resolved by whichever
/// side ran last. An empty plan proposes nothing and is refused by nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// One entry per value the plan stages.
    pub staging: Vec<Staging>,
}

/// Why a plan was refused.
///
/// # Invariant
/// One variant per way a proposal fails, and each names what it met: a plan is the one place a
/// machine states a placement, so a refusal says which entry was wrong and why rather than
/// reporting nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanRefusal {
    /// The plan names a value no mark published.
    UnknownStagedValue {
        /// The name the plan used.
        name: String,
    },
    /// The plan names a value two marks published.
    AmbiguousStagedValue {
        /// The name the plan used.
        name: String,
    },
    /// The value the plan names is not one the body staged.
    NotAStagedValue {
        /// The name the plan used.
        name: String,
    },
    /// The plan's group width is not a width the fragment's target has.
    StagingWidthNotAWidth {
        /// The name the plan used.
        name: String,
        /// The width the plan asked for.
        asked: u32,
        /// The lane count one workgroup runs.
        lane_width: u32,
    },
    /// The fragment's own slice states another space than the plan.
    PlanContradicted {
        /// The name the plan used.
        name: String,
        /// The space the plan asked for.
        asked: Space,
        /// The space the fragment declares.
        found: Space,
    },
}

impl KernelFragment {
    /// The value a mark named `name` was attached to.
    ///
    /// # Invariant
    /// A name two values carry names an ambiguity rather than a value, so it is refused rather
    /// than resolved to the first: which one a reader picked would be an accident of the body's
    /// walk order (`docs/notes/graph-fusion.md` §5).
    pub fn marked_value(&self, name: &str) -> Result<ValueId, PlanRefusal> {
        let mut found = self
            .marks
            .iter()
            .filter(|mark| mark.text() == Some(name))
            .map(|mark| mark.value);
        let first = found
            .next()
            .ok_or_else(|| PlanRefusal::UnknownStagedValue {
                name: name.to_string(),
            })?;
        if found.next().is_some() {
            return Err(PlanRefusal::AmbiguousStagedValue {
                name: name.to_string(),
            });
        }
        Ok(first)
    }

    /// Whether this fragment honours `plan`, or the entry that refutes it.
    ///
    /// # Invariant
    /// Checked against the fragment rather than believed: the width is the target's own lane
    /// count, the name resolves to a mark the body published, the value it names is the staging
    /// itself, and the space is the one the slice already declares — so a plan the engine did not
    /// apply is refused instead of reported as applied (`docs/notes/graph-fusion.md` §5).
    pub fn check_plan(&self, plan: &Plan) -> Result<(), PlanRefusal> {
        let lane_width = self.target.lane_width.lanes();
        for entry in &plan.staging {
            let value = self.marked_value(&entry.name)?;
            let name = entry.name.clone();
            let Some(ordinal) = self.body.staged_slice(value) else {
                return Err(PlanRefusal::NotAStagedValue { name });
            };
            let Some(slice) = self.slices.get(ordinal) else {
                return Err(PlanRefusal::NotAStagedValue { name });
            };
            if entry.group_width != lane_width {
                return Err(PlanRefusal::StagingWidthNotAWidth {
                    name,
                    asked: entry.group_width,
                    lane_width,
                });
            }
            if slice.space != entry.space {
                return Err(PlanRefusal::PlanContradicted {
                    name,
                    asked: entry.space,
                    found: slice.space,
                });
            }
        }
        Ok(())
    }
}

impl PlanRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for PlanRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanRefusal::UnknownStagedValue { name } => write!(
                formatter,
                "the plan stages `{name}` and the fragment's body published no mark of that \
                 name, so the plan names no value (`docs/notes/graph-fusion.md` §5)"
            ),
            PlanRefusal::AmbiguousStagedValue { name } => write!(
                formatter,
                "the plan stages `{name}` and two values carry that mark, so the name states an \
                 ambiguity rather than a value (`docs/notes/graph-fusion.md` §5)"
            ),
            PlanRefusal::NotAStagedValue { name } => write!(
                formatter,
                "the plan stages `{name}` and the value that mark named is not a staging: the \
                 plan states where a value lives, and only a slice write stages one \
                 (`docs/notes/graph-fusion.md` §5)"
            ),
            PlanRefusal::StagingWidthNotAWidth {
                name,
                asked,
                lane_width,
            } => write!(
                formatter,
                "the plan stages `{name}` in a group of {asked} lane(s) and one workgroup runs \
                 {lane_width}, so the width is not one this target has: a group narrower than the \
                 workgroup needs a barrier narrower than the block, which lichen does not have \
                 (`docs/notes/graph-fusion.md` §5)"
            ),
            PlanRefusal::PlanContradicted { name, asked, found } => write!(
                formatter,
                "the plan stages `{name}` in {asked:?} and the fragment's slice declares \
                 {found:?}, so the plan the engine reports as applied is not the one it applied \
                 (`docs/notes/graph-fusion.md` §5)"
            ),
        }
    }
}

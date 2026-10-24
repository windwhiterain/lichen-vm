//! The fusion file: a strategy, a budget, and the overrides a human edits.
//! See docs/notes/automatic-fusion.md.
//!
//! # Invariant
//! **The file steers and never decides.** It names a strategy, it bounds the regions the merge
//! pass attempts, and it names the regions to merge or to bar; what a merge must satisfy is
//! still the legality check's own, and no field here loosens one.

use std::fmt;

use crate::fusion_claim::SchemaHash;
use crate::placement_pack::{PackRefusal, PlacementPack, read_placement_pack};

/// The name the pack a force-merge override becomes carries in the run report.
const FORCE_MERGE_PACK: &str = "fusion-file force_merge";

/// A fusion file: what a runner installs, and what a human edits.
///
/// # Invariant
/// Every field is read into the plan, and the plan carries it into the pass; a field the file
/// omits takes the floor's own answer, so a file states what this machine wants and nothing
/// else.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionFile {
    /// The strategy the plan runs under.
    pub strategy: Strategy,
    /// The bounds the merge gate reads.
    pub budget: Budget,
    /// What the file refuses.
    pub refuse: Refusals,
    /// The patterns the file names.
    pub patterns: Vec<Pattern>,
    /// The overrides the file states.
    pub overrides: Vec<Override>,
}

/// The strategy the merge pass runs under.
///
/// # Invariant
/// One variant, and it is the family the survey chose: a consumer-side greedy pass (H2) and a
/// sibling merge (H4), the shape that improves a fan-out without duplicating the producer
/// (`docs/notes/automatic-fusion.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The greedy sibling pass.
    GreedySibling,
}

impl Strategy {
    /// The name a report prints for this strategy.
    pub fn as_str(&self) -> &'static str {
        match self {
            Strategy::GreedySibling => "greedy_sibling",
        }
    }

    /// The strategy the name spells, or `None` when it spells none.
    pub fn parse(text: &str) -> Option<Strategy> {
        match text {
            "greedy_sibling" => Some(Strategy::GreedySibling),
            _ => None,
        }
    }
}

/// The bounds a plan carries.
///
/// # Invariant
/// **A bound the file omits is no bound**: the plan states what this machine wants limited, and
/// a field it leaves out is the floor's own answer. `compute_tolerance`, `staging_bytes` and
/// `passes` are read and carried, and the merge gate spends none of them yet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// The most nodes a region may hold and still be attempted.
    pub max_region_nodes: usize,
    /// The deepest a region's own DAG may be and still be attempted.
    pub max_region_depth: usize,
    /// The recomputation the plan tolerates, as a fraction of the work it saves.
    pub compute_tolerance: f64,
    /// The staging the plan allows, in bytes.
    pub staging_bytes: usize,
    /// How many merge passes the plan asks for.
    pub passes: usize,
}

impl Budget {
    /// The budget that bounds nothing.
    pub fn unlimited() -> Budget {
        Budget {
            max_region_nodes: usize::MAX,
            max_region_depth: usize::MAX,
            compute_tolerance: 0.0,
            staging_bytes: usize::MAX,
            passes: 1,
        }
    }
}

/// What the file refuses.
///
/// # Invariant
/// The two refusals the survey names, read and carried. The engine's own refusals stand
/// whatever they say: a shared producer is refused (`NotAReductionChain`) and an interface the
/// pass cannot prove is a barrier (`InterfacesAreNotProved`), and this file cannot loosen one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusals {
    /// Whether the file refuses to duplicate a producer several consumers read.
    pub shared_producer_duplication: bool,
    /// Whether an interface the pass cannot prove bars the merge.
    pub opaque_is_barrier: bool,
}

impl Default for Refusals {
    fn default() -> Refusals {
        Refusals {
            shared_producer_duplication: false,
            opaque_is_barrier: true,
        }
    }
}

/// A pattern the file names.
///
/// # Invariant
/// Read and carried: a pattern names the nodes a human matched by hand, and nothing consults
/// it yet, so a file that states one changes no merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    /// The pattern's name.
    pub name: String,
    /// The node names the pattern holds.
    pub nodes: Vec<String>,
    /// The pattern's priority, when the file states one.
    pub priority: Option<i64>,
}

/// One override: what the file says about one region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    /// The region the override names.
    pub region: SchemaHash,
    /// What the file says about it.
    pub action: Action,
    /// Why the file says so.
    pub reason: Option<String>,
}

/// What an override says about its region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// The region's operations become one kernel.
    ForceMerge,
    /// The region is not merged.
    Barrier,
}

/// The plan a fusion file answers.
///
/// # Invariant
/// The plan carries what the file states and nothing it does not: the strategy the pass runs
/// under, the budget it gates on, the packs the force-merge overrides installed, and the
/// regions the barriers bar.
#[derive(Debug, Clone, PartialEq)]
pub struct FusionPlan {
    /// The strategy the merge pass runs under.
    pub strategy: Strategy,
    /// The bounds the merge gate reads.
    pub budget: Budget,
    /// The packs the file's force-merge overrides installed.
    pub overrides: Vec<PlacementPack>,
    /// The regions the file bars from merging.
    pub barriers: Vec<SchemaHash>,
}

/// Why a fusion file's text was refused.
///
/// # Invariant
/// One variant per way the text fails, and each names the field it met: a human edits this
/// file, so a refusal that says only *that* it failed leaves nothing to fix.
#[derive(Debug, Clone, PartialEq)]
pub enum FusionFileRefusal {
    /// The text is not JSON this artifact can be read from.
    Malformed {
        /// The path the offending field sits at, when the parser knew one.
        path: Option<String>,
        /// The byte offset and length the parser pointed at, in the text.
        at: Option<(usize, usize)>,
        /// What the parser said.
        message: String,
    },
    /// The strategy is not one the pass runs under.
    UnknownStrategy {
        /// What the field carried.
        found: String,
    },
    /// The override's region is not one `sha256:<hex>` spells.
    BadRegionHash {
        /// What the field carried.
        found: String,
    },
    /// One region is overridden twice.
    RegionIsOverriddenTwice {
        /// The region both overrides name.
        region: String,
    },
    /// The pack a force-merge override became was refused.
    PackRefused {
        /// The region the override named.
        region: String,
        /// The pack reader's own refusal.
        refusal: PackRefusal,
    },
}

impl FusionFileRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for FusionFileRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FusionFileRefusal::Malformed { path, at, message } => {
                write!(formatter, "the text is not a fusion file: {message}")?;
                if let Some(path) = path {
                    write!(formatter, " (at `{path}`)")?;
                }
                if let Some((offset, len)) = at {
                    write!(formatter, " (bytes {offset}..{})", offset + len)?;
                }
                Ok(())
            }
            FusionFileRefusal::UnknownStrategy { found } => write!(
                formatter,
                "the file names the strategy `{found}`, which is not one the pass runs under: the \
                 only one is `greedy_sibling` (`docs/notes/automatic-fusion.md`)"
            ),
            FusionFileRefusal::BadRegionHash { found } => write!(
                formatter,
                "the override names the region `{found}`, which is not a schema hash: one is \
                 `sha256:` and 64 hex digits, the shape a region report prints \
                 (`docs/notes/graph-fusion.md` §4)"
            ),
            FusionFileRefusal::RegionIsOverriddenTwice { region } => write!(
                formatter,
                "the region `{region}` is overridden twice, and one region takes one action"
            ),
            FusionFileRefusal::PackRefused { region, refusal } => write!(
                formatter,
                "the override of the region `{region}` became a pack the engine refused: {refusal}"
            ),
        }
    }
}

/// Read `text` as the fusion file it names, checking it as it goes.
///
/// # Invariant
/// **Pure**: nothing is interned and no global is consulted, so the same text reads back as the
/// same file in any process. Every refusal happens before a plan reaches a pass, and each names
/// the field it met.
pub fn read_fusion_file(text: &str) -> Result<FusionFile, FusionFileRefusal> {
    let file: artifact::File = facet_json::from_str(text).map_err(malformed)?;
    let strategy = match file.strategy.as_deref() {
        None => Strategy::GreedySibling,
        Some(found) => {
            Strategy::parse(found).ok_or_else(|| FusionFileRefusal::UnknownStrategy {
                found: found.to_string(),
            })?
        }
    };
    let budget = file
        .budget
        .as_ref()
        .map(Budget::from)
        .unwrap_or_else(Budget::unlimited);
    let refuse = file
        .refuse
        .map(Refusals::from)
        .unwrap_or_else(Refusals::default);
    let patterns = file
        .patterns
        .unwrap_or_default()
        .into_iter()
        .map(Pattern::from)
        .collect();
    let mut overrides: Vec<Override> = Vec::new();
    for entry in file.overrides.unwrap_or_default() {
        let region =
            SchemaHash::parse(&entry.region).ok_or_else(|| FusionFileRefusal::BadRegionHash {
                found: entry.region.clone(),
            })?;
        if overrides.iter().any(|held| held.region == region) {
            return Err(FusionFileRefusal::RegionIsOverriddenTwice {
                region: region.to_string(),
            });
        }
        overrides.push(Override {
            region,
            action: entry.action.into(),
            reason: entry.reason,
        });
    }
    Ok(FusionFile {
        strategy,
        budget,
        refuse,
        patterns,
        overrides,
    })
}

impl FusionFile {
    /// The plan this file answers: the packs its overrides install and the regions they bar.
    ///
    /// # Invariant
    /// **One override is one region.** A force-merge override becomes a `fuse` pack naming that
    /// region and asking for one kernel, read with the pack's own reader, so a pack this file
    /// spells is a pack the engine already knows. A barrier names the region and asks for no
    /// merge, which the pack vocabulary cannot spell, so the plan carries it beside the packs.
    pub fn plan(&self) -> Result<FusionPlan, FusionFileRefusal> {
        let mut overrides = Vec::new();
        let mut barriers = Vec::new();
        for entry in &self.overrides {
            match entry.action {
                Action::ForceMerge => overrides.push(force_merge_pack(entry)?),
                Action::Barrier => barriers.push(entry.region.clone()),
            }
        }
        Ok(FusionPlan {
            strategy: self.strategy,
            budget: self.budget,
            overrides,
            barriers,
        })
    }
}

/// The pack a force-merge override becomes: a `fuse` naming the region, asking for one kernel.
///
/// # Invariant
/// **The pack's own reader reads it**, so the override is installed as the pack the engine
/// already knows and no second spelling of a pack exists here.
fn force_merge_pack(entry: &Override) -> Result<PlacementPack, FusionFileRefusal> {
    let text = format!(
        r#"{{"schema": null, "name": "{name}", "requires": {{"class": null}}, "fuse": {{"region": "{region}", "as": "one_kernel"}}, "plan": {{"staging": []}}}}"#,
        name = FORCE_MERGE_PACK,
        region = entry.region,
    );
    read_placement_pack(&text).map_err(|refusal| FusionFileRefusal::PackRefused {
        region: entry.region.to_string(),
        refusal,
    })
}

/// The refusal a `facet_json` parse error earns, with where it happened.
fn malformed(error: facet_json::DeserializeError) -> FusionFileRefusal {
    let (path, at, message) = crate::fused::malformed_at(error);
    FusionFileRefusal::Malformed { path, at, message }
}

impl From<&artifact::Budget> for Budget {
    fn from(budget: &artifact::Budget) -> Budget {
        let unlimited = Budget::unlimited();
        Budget {
            max_region_nodes: budget
                .max_region_nodes
                .unwrap_or(unlimited.max_region_nodes),
            max_region_depth: budget
                .max_region_depth
                .unwrap_or(unlimited.max_region_depth),
            compute_tolerance: budget
                .compute_tolerance
                .unwrap_or(unlimited.compute_tolerance),
            staging_bytes: budget.staging_bytes.unwrap_or(unlimited.staging_bytes),
            passes: budget.passes.unwrap_or(unlimited.passes),
        }
    }
}

impl From<artifact::Refusals> for Refusals {
    fn from(refuse: artifact::Refusals) -> Refusals {
        let default = Refusals::default();
        Refusals {
            shared_producer_duplication: refuse
                .shared_producer_duplication
                .unwrap_or(default.shared_producer_duplication),
            opaque_is_barrier: refuse
                .opaque_is_barrier
                .unwrap_or(default.opaque_is_barrier),
        }
    }
}

impl From<artifact::Action> for Action {
    fn from(action: artifact::Action) -> Action {
        match action {
            artifact::Action::ForceMerge => Action::ForceMerge,
            artifact::Action::Barrier => Action::Barrier,
        }
    }
}

impl From<artifact::Pattern> for Pattern {
    fn from(pattern: artifact::Pattern) -> Pattern {
        Pattern {
            name: pattern.name,
            nodes: pattern.nodes,
            priority: pattern.priority,
        }
    }
}

/// The fusion file's own text, as the JSON spells it.
///
/// # Invariant
/// Every type mirrors the file the note sketches, and the fields the note's own example
/// carries are the fields this reader accepts, so the example a human copies is a file this
/// reader reads (`docs/notes/automatic-fusion.md`).
mod artifact {
    use facet::Facet;

    /// A fusion file, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct File {
        /// The strategy the plan runs under, or `null` for the floor's own.
        pub strategy: Option<String>,
        /// The bounds the merge gate reads.
        pub budget: Option<Budget>,
        /// What the file refuses.
        pub refuse: Option<Refusals>,
        /// The patterns the file names.
        pub patterns: Option<Vec<Pattern>>,
        /// The overrides the file states.
        pub overrides: Option<Vec<Override>>,
    }

    /// The bounds the merge gate reads, as the text spells them.
    #[derive(Debug, Clone, PartialEq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Budget {
        /// The most nodes a region may hold, or `null` for no bound.
        pub max_region_nodes: Option<usize>,
        /// The deepest a region's own DAG may be, or `null` for no bound.
        pub max_region_depth: Option<usize>,
        /// The recomputation the plan tolerates, or `null` for none.
        pub compute_tolerance: Option<f64>,
        /// The staging the plan allows, or `null` for none.
        pub staging_bytes: Option<usize>,
        /// How many merge passes the plan asks for, or `null` for one.
        pub passes: Option<usize>,
    }

    /// What the file refuses, as the text spells it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Refusals {
        /// Whether the file refuses to duplicate a shared producer, or `null` for the floor's own.
        pub shared_producer_duplication: Option<bool>,
        /// Whether an unproved interface bars the merge, or `null` for the floor's own.
        pub opaque_is_barrier: Option<bool>,
    }

    /// A pattern the file names, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Pattern {
        /// The pattern's name.
        pub name: String,
        /// The node names the pattern holds.
        pub nodes: Vec<String>,
        /// The pattern's priority, or `null` for none.
        pub priority: Option<i64>,
    }

    /// One override, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Override {
        /// The region the override names, as `sha256:<hex>`.
        pub region: String,
        /// What the file says about it.
        pub action: Action,
        /// Why the file says so, or `null` for no reason.
        pub reason: Option<String>,
    }

    /// What an override says about its region, as the text spells it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(u8)]
    pub enum Action {
        /// The region's operations become one kernel.
        #[facet(rename = "force_merge")]
        ForceMerge,
        /// The region is not merged.
        #[facet(rename = "barrier")]
        Barrier,
    }
}

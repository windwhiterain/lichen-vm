//! A placement pack: a runner's plan for a node, and its `fuse` for a region.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! A pack is guidance, not an implementation: it names the schema it steers, the fragment facts
//! it needs and — where it has them — where the staged values live and which region's
//! operations become one kernel. It never says what anything computes. A pack whose hash or
//! constraints do not match is **inert**, so the engine's own choice answers.

use crate::fused::malformed_at;
use lichen_kernel_ir::{KernelFragment, Plan, ScalarClass};

use crate::fusion_claim::SchemaHash;

/// A plan a runner installs for one node schema, read from the text a human wrote.
///
/// # Invariant
/// [`read_placement_pack`] is the only constructor, so every hash a pack spells is one
/// `sha256:<hex>` and every pack steers **something**: a staged placement, a named region, or
/// both. The two halves are separate — a `schema` places one node and a [`Fuse`] names one
/// region — so a pack that steers one places no node.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementPack {
    /// The node schema this pack places, or `None` when it places no node.
    schema: Option<SchemaHash>,
    /// The pack's name, for the run report.
    name: String,
    /// The fragment facts the pack needs before it may fire.
    requires: Constraints,
    /// The machine's decision that a region's operations become one kernel.
    fuse: Option<Fuse>,
    /// Where the values the fusion stages live.
    plan: Plan,
}

/// The machine's decision that a region's operations become one kernel.
///
/// # Invariant
/// A `fuse` naming no region steers no fusion: the pack is placement-only, and the merge pass
/// reads it as naming nothing, so the engine's own floor answers. The hash is the **region's**
/// schema — which of the two granularities a pack binds by is what it steers (§4) — and the
/// directive states no body, so what the region computes is still the logic's.
#[derive(Debug, Clone, PartialEq)]
pub struct Fuse {
    /// The region this pack steers, or `None` when the directive names none.
    region: Option<SchemaHash>,
    /// What the region's operations become.
    form: Fusion,
}

/// What a fused region's operations become.
///
/// # Invariant
/// One variant, and it is the only decision this interface carries: `compose` inlines a chain,
/// so asking for anything else would name a shape the engine cannot produce rather than a
/// licence it would have to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fusion {
    /// Every operation of the region becomes one kernel.
    OneKernel,
}

impl Fuse {
    /// The region this pack's `fuse` names, or `None` when it names none.
    pub fn region(&self) -> Option<&SchemaHash> {
        self.region.as_ref()
    }

    /// What the region's operations become, as a report reads it.
    pub fn form(&self) -> Fusion {
        self.form
    }
}

/// The fragment facts a pack states and the engine checks.
///
/// # Invariant
/// Stated by the pack and read off the fragment, never derived: a constraint is the one fact a
/// schema hash cannot carry, so a mismatch makes the pack inert rather than wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Constraints {
    /// The element class the node's first input buffer must be, or `None`.
    class: Option<ScalarClass>,
    /// The node positions the class constraints apply to; empty is every node.
    nodes: Vec<usize>,
    /// The input slots and the class each takes, which narrows one buffer of a node.
    classes: Vec<(usize, ScalarClass)>,
}

impl Constraints {
    /// Whether the element class in hand is one this pack's class may hold against, or which is not.
    ///
    /// # Invariant
    /// The constraint is a **shape**, not a precision: the language's `Float` says the buffer holds real
    /// numbers and says nothing about how many bits one takes, so a pack stating `BFloat16` holds against
    /// a node whose buffer states `Float` — that licence is the whole point of a pack. A real/integer
    /// crossing is the one mismatch that cannot be licensed, because nothing converts.
    fn hold_for(&self, class: Option<ScalarClass>) -> Result<(), ConstraintRefusal> {
        let Some(wanted) = self.class else {
            return Ok(());
        };
        match class {
            Some(held) if held == wanted => Ok(()),
            Some(held) if held.is_real() && wanted.is_real() => Ok(()),
            found => Err(ConstraintRefusal::ClassMismatch { wanted, found }),
        }
    }
}

impl PlacementPack {
    /// The node schema this pack places, or `None` when it places no node.
    pub fn schema(&self) -> Option<&SchemaHash> {
        self.schema.as_ref()
    }

    /// Whether this pack places the node whose schema is `hash`, staging a value for it.
    ///
    /// # Invariant
    /// Placement needs both halves: a pack that names no node places none, and a plan that stages
    /// nothing places nothing, so neither is a firing pack. This is the gate the tiling pass
    /// consults, and it is why a pack that steers only a fusion never reaches the emitter.
    pub fn places(&self, hash: &SchemaHash) -> bool {
        self.schema() == Some(hash) && self.plan.staging.first().is_some()
    }

    /// The pack's name, for the run report.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The element class this pack states for the node's first input buffer.
    ///
    /// # Invariant
    /// The pack's sentence about precision, not a fact read off the fragment: a node whose pack
    /// states one is compiled for it, which is what makes `Buf T` the payload's shape alone.
    pub fn class(&self) -> Option<ScalarClass> {
        self.requires.class
    }

    /// The node positions this pack's class constraints apply to; empty is every node.
    pub fn nodes(&self) -> &[usize] {
        &self.requires.nodes
    }

    /// The input slots this pack narrows, each with the class it takes.
    pub fn classes(&self) -> &[(usize, ScalarClass)] {
        &self.requires.classes
    }

    /// The plan this pack hands over.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The machine's decision that a region's operations become one kernel, when the pack states one.
    pub fn fuse(&self) -> Option<&Fuse> {
        self.fuse.as_ref()
    }

    /// Whether `fragment` states every fact this pack's constraints need.
    pub fn holds_for(&self, fragment: &KernelFragment) -> Result<(), ConstraintRefusal> {
        self.holds_for_class(fragment.input_classes.first().copied())
    }

    /// Whether the element class in hand is every fact this pack's constraints need.
    ///
    /// # Invariant
    /// One rule read off two sources: a node's first input buffer, or a region's first input
    /// port. A node with no input states no class, so a pack needing one does not hold there
    /// rather than being checked against something else.
    pub fn holds_for_class(&self, class: Option<ScalarClass>) -> Result<(), ConstraintRefusal> {
        self.requires.hold_for(class)
    }
}

/// Why a pack's constraints did not hold against a fragment.
///
/// # Invariant
/// A mismatch is inert rather than fatal, so this names the fact that failed rather than
/// refusing the program: the engine's own choice answers instead (`docs/notes/graph-fusion.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintRefusal {
    /// The pack needs an element class the node's first input buffer does not state.
    ClassMismatch {
        /// The class the pack asked for.
        wanted: ScalarClass,
        /// The class the fragment states, or `None` for a node with no input buffer.
        found: Option<ScalarClass>,
    },
}

impl ConstraintRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for ConstraintRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintRefusal::ClassMismatch { wanted, found } => match found {
                Some(found) => write!(
                    formatter,
                    "the pack needs a {wanted:?} element and this node's first input buffer \
                     states {found:?}"
                ),
                None => write!(
                    formatter,
                    "the pack needs a {wanted:?} element and this node's first input buffer \
                     states no element class"
                ),
            },
        }
    }
}

/// Why a pack's text was refused.
///
/// # Invariant
/// One variant per way the text fails, and each names what it met: a human writes this file, so
/// a refusal that says only *that* it failed tells them nothing about which field to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackRefusal {
    /// The text is not JSON this artifact can be read from.
    Malformed {
        /// The path the offending field sits at, when the parser knew one.
        path: Option<String>,
        /// The byte offset and length the parser pointed at, in the text.
        at: Option<(usize, usize)>,
        /// What the parser said.
        message: String,
    },
    /// The schema hash is not one `sha256:<hex>` spells.
    BadSchemaHash {
        /// What the field carried.
        found: String,
    },
    /// The `fuse` directive's region hash is not one `sha256:<hex>` spells.
    ///
    /// # Invariant
    /// A distinct arm from [`Self::BadSchemaHash`] because it is a distinct field: the node's
    /// hash places an intra-kernel staging and the region's licenses a fusion, so a report that
    /// named both by one sentence could not say which one to fix.
    BadRegionHash {
        /// What the field carried.
        found: String,
    },
    /// The pack stages nothing and names no region, so it steers nothing.
    EmptyPlan {
        /// The pack's own name.
        name: String,
    },
}

impl PackRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl std::fmt::Display for PackRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackRefusal::Malformed { path, at, message } => {
                write!(formatter, "the text is not a placement pack: {message}")?;
                if let Some(path) = path {
                    write!(formatter, " (at `{path}`)")?;
                }
                if let Some((offset, len)) = at {
                    write!(formatter, " (bytes {offset}..{})", offset + len)?;
                }
                Ok(())
            }
            PackRefusal::BadSchemaHash { found } => write!(
                formatter,
                "the pack implements `{found}`, which is not a schema hash: one is \
                 `sha256:` and 64 hex digits, the shape a report prints \
                 (`docs/notes/graph-fusion.md` §4)"
            ),
            PackRefusal::BadRegionHash { found } => write!(
                formatter,
                "the pack's `fuse` names the region `{found}`, which is not a schema hash: a \
                 region is bound by the **region's** own hash, which a region report prints \
                 (`docs/notes/graph-fusion.md` §4)"
            ),
            PackRefusal::EmptyPlan { name } => write!(
                formatter,
                "the pack `{name}` stages nothing and names no region, so it steers neither a \
                 placement nor a fusion (`docs/notes/graph-fusion.md` §5)"
            ),
        }
    }
}

/// Read `text` as the pack it names, checking it as it goes.
///
/// # Invariant
/// **Pure**: nothing is interned and no global is consulted, so the same text reads back as the
/// same pack in any process. The order is parse, node hash, region hash, plan: every refusal
/// happens before a pack reaches a pass, and a pack that steers neither a placement nor a
/// fusion is refused rather than installed inert.
pub fn read_placement_pack(text: &str) -> Result<PlacementPack, PackRefusal> {
    let pack: artifact::Pack = facet_json::from_str(text).map_err(malformed)?;
    let schema = pack
        .schema
        .map(|found| SchemaHash::parse(&found).ok_or(PackRefusal::BadSchemaHash { found }))
        .transpose()?;
    let fuse = pack
        .fuse
        .map(|directive| {
            let region = directive
                .region
                .map(|found| SchemaHash::parse(&found).ok_or(PackRefusal::BadRegionHash { found }))
                .transpose()?;
            Ok(Fuse {
                region,
                form: directive.r#as.into(),
            })
        })
        .transpose()?;
    let plan: Plan = pack.plan.into();
    let class = pack.requires.class.map(ScalarClass::from);
    let nodes = pack.requires.nodes.unwrap_or_default();
    let classes: Vec<(usize, ScalarClass)> = pack
        .requires
        .classes
        .unwrap_or_default()
        .into_iter()
        .map(|entry| (entry.at, ScalarClass::from(entry.class)))
        .collect();
    // A class constraint is not a staging: a pack that only states classes needs no plan.
    let constraint = class.is_some() || !classes.is_empty();
    if plan.staging.is_empty() && fuse.as_ref().and_then(Fuse::region).is_none() && !constraint {
        return Err(PackRefusal::EmptyPlan { name: pack.name });
    }
    Ok(PlacementPack {
        schema,
        name: pack.name,
        requires: Constraints {
            class,
            nodes,
            classes,
        },
        fuse,
        plan,
    })
}

/// The refusal a `facet_json` parse error earns, with where it happened.
fn malformed(error: facet_json::DeserializeError) -> PackRefusal {
    let (path, at, message) = malformed_at(error);
    PackRefusal::Malformed { path, at, message }
}

impl From<artifact::ScalarClass> for ScalarClass {
    fn from(class: artifact::ScalarClass) -> Self {
        match class {
            artifact::ScalarClass::Int => ScalarClass::Int,
            artifact::ScalarClass::Float => ScalarClass::Float,
            artifact::ScalarClass::BFloat16 => ScalarClass::BFloat16,
        }
    }
}

impl From<artifact::Fusion> for Fusion {
    fn from(form: artifact::Fusion) -> Self {
        match form {
            artifact::Fusion::OneKernel => Fusion::OneKernel,
        }
    }
}

/// The pack's own text, as the JSON spells it.
///
/// # Invariant
/// Every type mirrors `lichen-kernel-ir`, and the plan is the fused artifact's own spelling
/// rather than a second one: one plan text serves both artifacts
/// (`crates/lichen-runtime/src/fused/schema.rs`).
mod artifact {
    use facet::Facet;

    use crate::fused::schema::Plan;

    /// One scalar class, as a pack spells it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(u8)]
    #[facet(rename_all = "lowercase")]
    pub enum ScalarClass {
        /// The IR's `Int`.
        Int,
        /// The IR's `Float`.
        Float,
        /// The IR's `BFloat16`.
        BFloat16,
    }

    /// The fragment facts a pack states.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Constraints {
        /// The element class the node's first input buffer must be, or `null`.
        pub class: Option<ScalarClass>,
        /// The input slots and the class each takes, or `null` for the whole node.
        pub classes: Option<Vec<ClassAt>>,
        /// The node positions these class constraints apply to, or `null` for every node.
        pub nodes: Option<Vec<usize>>,
    }

    /// One input slot and the class a pack states for it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct ClassAt {
        /// The fragment's input slot, in its own order.
        pub at: usize,
        /// The class the slot takes.
        pub class: ScalarClass,
    }

    /// A placement pack, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Pack {
        /// The node schema hash this pack places, as `sha256:<hex>`, or `null` for none.
        pub schema: Option<String>,
        /// The pack's name, for the run report.
        pub name: String,
        /// The fragment facts the pack needs.
        pub requires: Constraints,
        /// The machine's decision that a region's operations become one kernel, or `null`.
        pub fuse: Option<Fuse>,
        /// Where the values the fusion stages live.
        pub plan: Plan,
    }

    /// What a fused region's operations become, as a pack spells it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(u8)]
    pub enum Fusion {
        /// Every operation of the region becomes one kernel.
        #[facet(rename = "one_kernel")]
        OneKernel,
    }

    /// The `fuse` half of a pack, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Fuse {
        /// The **region's** schema hash this pack steers, as `sha256:<hex>`, or `null`.
        pub region: Option<String>,
        /// What the region's operations become.
        #[facet(rename = "as")]
        pub r#as: Fusion,
    }
}

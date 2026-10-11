//! A placement pack: the plan a runner installs for a node's schema.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! A pack is guidance, not an implementation: it names the schema it implements, the fragment
//! facts it needs and where the staged values live, and never what a region computes. A pack
//! whose hash or constraints do not match is **inert**, so the engine's own choice answers.

use lichen_compute::compute::fused::malformed_at;
use lichen_kernel_ir::{KernelFragment, Plan, ScalarClass, Staging};

use crate::fusion_claim::SchemaHash;

/// A plan a runner installs for one node schema, read from the text a human wrote.
///
/// # Invariant
/// [`read_placement_pack`] is the only constructor, so a pack's schema is always one
/// `sha256:<hex>` spells and its plan always stages at least one value.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacementPack {
    /// The node schema this pack implements.
    schema: SchemaHash,
    /// The pack's name, for the run report.
    name: String,
    /// The fragment facts the pack needs before it may fire.
    requires: Constraints,
    /// Where the values the fusion stages live.
    plan: Plan,
}

/// The fragment facts a pack states and the engine checks.
///
/// # Invariant
/// Stated by the pack and read off the fragment, never derived: a constraint is the one fact a
/// schema hash cannot carry, so a mismatch makes the pack inert rather than wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Constraints {
    /// The element class the node's first input buffer must be, or `None`.
    class: Option<ScalarClass>,
}

impl Constraints {
    /// Whether `fragment` states this constraint, or which one does not hold.
    ///
    /// # Invariant
    /// The class is read from the fragment's own first input buffer, which is the buffer an
    /// intra-kernel tiling folds: a node with no input states no class, so a pack needing one
    /// does not hold there rather than being checked against something else.
    fn hold_for(&self, fragment: &KernelFragment) -> Result<(), ConstraintRefusal> {
        let found = fragment.input_classes.first().copied();
        match self.class {
            Some(wanted) if found != Some(wanted) => {
                Err(ConstraintRefusal::ClassMismatch { wanted, found })
            }
            _ => Ok(()),
        }
    }
}

impl PlacementPack {
    /// The node schema this pack implements.
    pub fn schema(&self) -> &SchemaHash {
        &self.schema
    }

    /// The pack's name, for the run report.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The plan this pack hands over.
    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    /// The one staged value a plan for an intra-kernel tiling states.
    ///
    /// # Invariant
    /// The reader refuses a plan that stages nothing, so this entry always exists: it is where
    /// the space, the group width and the mark's name come from.
    pub fn staging(&self) -> &Staging {
        self.plan
            .staging
            .first()
            .expect("a pack that was read stages at least one value")
    }

    /// Whether `fragment` states every fact this pack's constraints need.
    pub fn holds_for(&self, fragment: &KernelFragment) -> Result<(), ConstraintRefusal> {
        self.requires.hold_for(fragment)
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
    /// The plan stages nothing, so the pack steers nothing.
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
            PackRefusal::EmptyPlan { name } => write!(
                formatter,
                "the pack `{name}` hands over a plan that stages nothing, so it names no \
                 placement to apply (`docs/notes/graph-fusion.md` §5)"
            ),
        }
    }
}

/// Read `text` as the pack it names, checking it as it goes.
///
/// # Invariant
/// **Pure**: nothing is interned and no global is consulted, so the same text reads back as the
/// same pack in any process. The order is parse, hash, plan: every refusal happens before a pack
/// reaches the pass, and a pack that names no plan is refused rather than installed inert.
pub fn read_placement_pack(text: &str) -> Result<PlacementPack, PackRefusal> {
    let pack: artifact::Pack = facet_json::from_str(text).map_err(malformed)?;
    let schema =
        SchemaHash::parse(&pack.schema).ok_or(PackRefusal::BadSchemaHash { found: pack.schema })?;
    let plan: Plan = pack.plan.into();
    if plan.staging.is_empty() {
        return Err(PackRefusal::EmptyPlan { name: pack.name });
    }
    Ok(PlacementPack {
        schema,
        name: pack.name,
        requires: Constraints {
            class: pack.requires.class.map(ScalarClass::from),
        },
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
        }
    }
}

/// The pack's own text, as the JSON spells it.
///
/// # Invariant
/// Every type mirrors `lichen-kernel-ir`, and the plan is the fused artifact's own spelling
/// rather than a second one: one plan text serves both artifacts
/// (`crates/lichen-compute/src/compute/fused/schema.rs`).
mod artifact {
    use facet::Facet;

    use lichen_compute::compute::fused::schema::Plan;

    /// One scalar class, as a pack spells it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(u8)]
    #[facet(rename_all = "lowercase")]
    pub enum ScalarClass {
        /// The IR's `Int`.
        Int,
        /// The IR's `Float`.
        Float,
    }

    /// The fragment facts a pack states.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Constraints {
        /// The element class the node's first input buffer must be, or `null`.
        pub class: Option<ScalarClass>,
    }

    /// A placement pack, as the text spells it.
    #[derive(Debug, Clone, PartialEq, Eq, Facet)]
    #[repr(C)]
    #[facet(deny_unknown_fields)]
    pub struct Pack {
        /// The node schema hash this pack implements, as `sha256:<hex>`.
        pub schema: String,
        /// The pack's name, for the run report.
        pub name: String,
        /// The fragment facts the pack needs.
        pub requires: Constraints,
        /// Where the values the fusion stages live.
        pub plan: Plan,
    }
}

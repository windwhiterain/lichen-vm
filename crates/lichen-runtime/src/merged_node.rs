//! The merged node's conservative treatment, and the name it is identified by.
//! See docs/notes/graph-fusion.md §5.
//!
//! # Invariant
//! **A merged node is checked, never changed.** The conservative checks read the node's own
//! facts and refuse it by name; they never rewrite the fragment, the graph, or what runs.

use std::fmt;

use lichen_graph_ir::Count;
use lichen_kernel_ir::{KernelFragment, ScalarClass, fragment_digest};

/// The prefix a merged node's digest is printed under.
///
/// # Invariant
/// **One spelling, and the `u64` is sixteen hex digits**: a reader copies the text as it is
/// printed, so the digest is a fixed width and a shorter one would name another kernel.
const DIGEST_PREFIX: &str = "digest";

/// A merged node's digest, as the report prints it: `digest:<16 hex>`.
///
/// # Invariant
/// **The fragment's own content hash**, the one the registry keys its id by, so the text names
/// the node a later pass would find rather than a hash of the hash.
pub fn digest_of(fragment: &KernelFragment) -> String {
    format!("{DIGEST_PREFIX}:{:016x}", fragment_digest(fragment))
}

/// What the conservative checks found about one merged node.
///
/// # Invariant
/// **The facts are the node's own**: the extent its launch names, the number of elements that
/// extent covers, the lane count one workgroup runs, and the class of each output the
/// fragment declares.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedFacts {
    /// The extent the launch names.
    pub extent: Count,
    /// The number of elements the launch covers.
    pub elements: usize,
    /// The lane count one workgroup runs.
    pub lane_width: u32,
    /// The class of each output the fragment declares, in write order.
    pub outputs: Vec<ScalarClass>,
}

/// Why a merged node is refused by the conservative checks.
///
/// # Invariant
/// One variant per fact the checks can fail on, and each names what it met: a decision
/// guessed here would be a node that runs and answers another program.
#[derive(Debug, Clone, PartialEq)]
pub enum MergedRefusal {
    /// The launch's extent is not readable here.
    ExtentIsUnreadable {
        /// The graph node the checks are read at.
        node: usize,
        /// The extent the launch names.
        extent: Count,
    },
    /// The launch covers a partial workgroup.
    WorkgroupIsPartial {
        /// The graph node the checks are read at.
        node: usize,
        /// The number of elements the launch covers.
        elements: usize,
        /// The lane count one workgroup runs.
        lane_width: u32,
    },
    /// The declared output extents or classes disagree with the fragment.
    OutputExtentsDiffer {
        /// The graph node the checks are read at.
        node: usize,
        /// What the declaration states.
        declared: String,
        /// What the fragment states.
        found: String,
    },
}

impl MergedRefusal {
    /// The refusal's name, for a report that has to say which check refused a node.
    ///
    /// # Invariant
    /// One variant, one name: a report that printed the sentence alone could not be matched
    /// against the variant a caller reads, so the two would drift.
    pub fn name(&self) -> &'static str {
        match self {
            MergedRefusal::ExtentIsUnreadable { .. } => "MergedExtentUnreadable",
            MergedRefusal::WorkgroupIsPartial { .. } => "MergedWorkgroupIsPartial",
            MergedRefusal::OutputExtentsDiffer { .. } => "MergedOutputExtentDiffers",
        }
    }

    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for MergedRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MergedRefusal::ExtentIsUnreadable { node, extent } => write!(
                formatter,
                "the launch of node {node} names a {extent:?} extent, and this reader has no \
                 number of elements for it, so the checks that need that number cannot be made"
            ),
            MergedRefusal::WorkgroupIsPartial {
                node,
                elements,
                lane_width,
            } => write!(
                formatter,
                "the launch of node {node} covers {elements} element(s) and a workgroup runs \
                 {lane_width} lane(s), so the staging would not be a whole workgroup"
            ),
            MergedRefusal::OutputExtentsDiffer {
                node,
                declared,
                found,
            } => write!(
                formatter,
                "the declared output extents and classes of node {node} disagree with its \
                 fragment: {declared} against {found}"
            ),
        }
    }
}

/// The conservative checks one merged node is held to, or the refusal that names the one that
/// failed.
///
/// # Invariant
/// **Three facts and no algebra**: the launch's extent is readable, the launch covers a whole
/// number of the kernel's lane width, and the declared output extents and classes agree with
/// the fragment. A node that passes them is neither tiled nor composed again — the checks read.
pub fn check(
    node: usize,
    fragment: &KernelFragment,
    extent: Count,
    elements: Option<usize>,
) -> Result<MergedFacts, MergedRefusal> {
    let Some(elements) = elements else {
        return Err(MergedRefusal::ExtentIsUnreadable { node, extent });
    };
    let lane_width = fragment.target.lane_width.lanes();
    if lane_width == 0 || elements % lane_width as usize != 0 {
        return Err(MergedRefusal::WorkgroupIsPartial {
            node,
            elements,
            lane_width,
        });
    }
    let outputs = outputs_of(node, fragment)?;
    Ok(MergedFacts {
        extent,
        elements,
        lane_width,
        outputs,
    })
}

/// The classes of the outputs the fragment declares, or the refusal that says they disagree.
///
/// # Invariant
/// **The fragment's own outputs are the authority**: a declaration that names an output the
/// fragment does not declare, or a class the fragment does not state, is refused rather than
/// read as another output's.
fn outputs_of(node: usize, fragment: &KernelFragment) -> Result<Vec<ScalarClass>, MergedRefusal> {
    let outputs = fragment.outputs;
    if fragment.output_classes.len() != outputs {
        return Err(MergedRefusal::OutputExtentsDiffer {
            node,
            declared: format!("{} class(es)", fragment.output_classes.len()),
            found: format!("{outputs} output(s)"),
        });
    }
    for (ordinal, class) in fragment.roles.output_classes.iter().enumerate() {
        let Some(class) = class else {
            continue;
        };
        match fragment.output_classes.get(ordinal).copied() {
            None => {
                return Err(MergedRefusal::OutputExtentsDiffer {
                    node,
                    declared: format!("the class {class:?} at output {ordinal}"),
                    found: format!("{outputs} output(s)"),
                });
            }
            Some(found) if found != *class => {
                return Err(MergedRefusal::OutputExtentsDiffer {
                    node,
                    declared: format!("the class {class:?} at output {ordinal}"),
                    found: format!("the class {found:?} at output {ordinal}"),
                });
            }
            Some(_) => {}
        }
    }
    for (ordinal, extent) in fragment.roles.output_extents.iter().enumerate() {
        if extent.is_some() && ordinal >= outputs {
            return Err(MergedRefusal::OutputExtentsDiffer {
                node,
                declared: format!("the extent of output {ordinal}"),
                found: format!("{outputs} output(s)"),
            });
        }
    }
    Ok(fragment.output_classes.clone())
}

/// The line one merged node's checks report: the checks passed, or the refusal that named the
/// one that failed.
///
/// # Invariant
/// **Every line names the node's identity**: the digest is what a later pass or a fusion file
/// names the node by, so a line that omits it names a node no reader can find again.
pub fn line(
    node: usize,
    fragment: &KernelFragment,
    extent: Count,
    elements: Option<usize>,
) -> String {
    let digest = digest_of(fragment);
    match check(node, fragment, extent, elements) {
        Ok(facts) => {
            let classes = facts
                .outputs
                .iter()
                .map(|class| format!("{class:?}"))
                .collect::<Vec<_>>();
            let classes = if classes.is_empty() {
                "none".to_string()
            } else {
                classes.join(", ")
            };
            format!(
                "node {node}: the merged node passed the conservative checks: its launch extent \
                 is {:?} and covers {} element(s), a workgroup of {} lane(s) divides it exactly, \
                 and it declares {} output(s) of class {classes}; it is not tiled and no algebra \
                 is read from it, and its identity is {digest}",
                facts.extent,
                facts.elements,
                facts.lane_width,
                facts.outputs.len(),
            )
        }
        Err(refusal) => format!(
            "node {node}: refused: {name}: {message}; the merged node's identity is {digest}",
            name = refusal.name(),
            message = refusal.message(),
        ),
    }
}

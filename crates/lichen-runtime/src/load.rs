//! One call that lands an artifact into a graph this device can run.
//! See `docs/notes/taking-a-kernel-to-a-server.md`.

use lichen_graph_ir::{Graph, GraphRefusal};

use crate::device::{DeviceFacts, DeviceFactsSource, DeviceRefusal};
use crate::fused::{self, FusedKernel, FusedRefusal, read_fused_kernel, schema};

/// An artifact read, checked against a device, and landed in a graph.
///
/// # Invariant
/// It carries the facts the fragment was checked against as well as the kernel: "this
/// artifact runs here" is a claim about **two** things, and a run report that names only
/// the artifact cannot say which one would have to change for it to stop being true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedKernel {
    /// The kernel the text named, as it was read.
    pub kernel: FusedKernel,
    /// The device it was checked against.
    pub facts: DeviceFacts,
}

/// Why an artifact was not landed.
///
/// # Invariant
/// One variant per gate, in the order they run, and each names which one fired: a caller
/// reading a run report has to know whether to fix the text, the device, or the node the
/// artifact named — and a refusal that reports all three at once decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadRefusal {
    /// The text names no format version.
    VersionMissing,
    /// The text names a format version this build does not read.
    VersionMismatch {
        /// The version the text states.
        found: u32,
        /// The version this build reads.
        wanted: u32,
    },
    /// The reader refused the text, before the graph was touched.
    Read(FusedRefusal),
    /// The device cannot honour what the fragment assumes.
    Device(DeviceRefusal),
    /// The graph refused the fragment the text named.
    Graph(GraphRefusal),
}

impl LoadRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }

    /// The refusal's own name, for a report that has to say which gate stopped a load.
    ///
    /// # Invariant
    /// The loader's gate, not the inner refusal's: `Read` and `Device` and `Graph` each
    /// hand the detail on, and naming all three levels in one string would leave a caller
    /// matching a sentence rather than a variant.
    pub fn name(&self) -> &'static str {
        match self {
            LoadRefusal::VersionMissing => "VersionMissing",
            LoadRefusal::VersionMismatch { .. } => "VersionMismatch",
            LoadRefusal::Read(_) => "Read",
            LoadRefusal::Device(_) => "Device",
            LoadRefusal::Graph(_) => "Graph",
        }
    }
}

impl std::fmt::Display for LoadRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadRefusal::VersionMissing => write!(
                formatter,
                "the text names no format version, so this build cannot say which fields it is \
                 meant to have: an artifact is read by its version or not read \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            LoadRefusal::VersionMismatch { found, wanted } => write!(
                formatter,
                "the text is artifact format {found} and this build reads format {wanted}, so \
                 the artifact is refused before any of it is parsed: reading another format \
                 with this one's fields is how a loader dispatches a kernel nobody wrote \
                 (`docs/notes/taking-a-kernel-to-a-server.md`)"
            ),
            LoadRefusal::Read(refusal) => write!(
                formatter,
                "the artifact's text was refused, so the graph and the device are untouched: \
                 {refusal}"
            ),
            LoadRefusal::Device(refusal) => write!(
                formatter,
                "the device cannot honour what the artifact assumes, so nothing is dispatched: \
                 {refusal}"
            ),
            LoadRefusal::Graph(refusal) => write!(
                formatter,
                "the graph refused the artifact's fragment: {refusal}"
            ),
        }
    }
}

/// Read `text` as an artifact, check it against `device`, and put its fragment into `graph`'s
/// node.
///
/// # Invariant
/// **Every gate runs before the graph is touched**: the version, then the reader's
/// own, then the device's answers — so a refused load leaves the graph as it was,
/// and a version that does not match is a refusal rather than a mis-parse. The text
/// is read twice, once to name the version and once to build the kernel.
pub fn load_fused_kernel(
    text: &str,
    graph: &mut Graph,
    node: usize,
    device: &impl DeviceFactsSource,
) -> Result<LoadedKernel, LoadRefusal> {
    let probe: schema::VersionProbe =
        facet_json::from_str(text).map_err(|error| LoadRefusal::Read(fused::malformed(error)))?;
    let version = probe.version.ok_or(LoadRefusal::VersionMissing)?;
    if version != schema::FORMAT_VERSION {
        return Err(LoadRefusal::VersionMismatch {
            found: version,
            wanted: schema::FORMAT_VERSION,
        });
    }
    let kernel = read_fused_kernel(text).map_err(LoadRefusal::Read)?;
    let facts = device.device_facts();
    facts
        .check(&kernel.fragment, &kernel.plan)
        .map_err(LoadRefusal::Device)?;
    graph
        .replace_kernel(node, kernel.fragment.clone())
        .map_err(LoadRefusal::Graph)?;
    Ok(LoadedKernel { kernel, facts })
}

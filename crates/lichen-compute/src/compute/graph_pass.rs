//! The installed graph pass: the thread-local a host fills and a run consults.
//! See docs/notes/graph-fusion.md §5 and §8.
//!
//! # Invariant
//! The pass itself is `lichen_runtime::graph_pass`, which takes only a graph and a launch
//! extent: a host thread's session is what owns one installed pass, so the installation and
//! the consultation stay here.

use std::cell::RefCell;

use lichen_graph_ir::Graph;

pub use lichen_runtime::graph_pass::{GraphPass, LaunchExtent, pass_chain};

thread_local! {
    /// The installed pass, and the lines the last consultation recorded.
    ///
    /// # Invariant
    /// Thread-local for the reason [`super::GRAPH_POLICY`] is: a pass belongs to one host
    /// thread's session, and a graph run happens on the thread that asked for it. The pass
    /// is taken out for the length of its own call, so a pass that builds a graph of its
    /// own cannot find itself installed and recurse.
    static GRAPH_PASS: RefCell<GraphPassSlot> = const { RefCell::new(GraphPassSlot {
        pass: None,
        lines: Vec::new(),
    }) };
}

/// What the thread-local holds: the installed pass and the lines it last reported.
struct GraphPassSlot {
    pass: Option<Box<dyn GraphPass>>,
    lines: Vec<String>,
}

/// Install `pass` as this thread's graph pass, replacing any previous one.
///
/// # Invariant
/// Replacing rather than accumulating, the way [`super::set_graph_policy`] replaces a
/// policy: two passes installed for one run would both rewrite the graph, and the second
/// would be applying to something the first already moved.
pub fn set_graph_pass(pass: Box<dyn GraphPass>) {
    GRAPH_PASS.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.pass = Some(pass);
        slot.lines.clear();
    });
}

/// Remove this thread's graph pass, if one is installed.
pub fn clear_graph_pass() {
    GRAPH_PASS.with(|slot| {
        let mut slot = slot.borrow_mut();
        slot.pass = None;
        slot.lines.clear();
    });
}

/// Hand `graph` to the installed pass and answer what it wants run, or `None` for none.
///
/// # Invariant
/// The pass is taken out for the length of the call, so a *second* consultation on this
/// thread — a pass that runs a graph of its own — sees no pass rather than itself. The
/// lines are appended to the slot's own, so a refusal recorded before a run that then
/// failed is still readable after it.
pub fn consult_graph_pass(graph: &Graph, extent: &LaunchExtent<'_>) -> Option<Graph> {
    GRAPH_PASS.with(|slot| {
        let pass = slot.borrow_mut().pass.take()?;
        let mut lines = Vec::new();
        let rewritten = pass.apply(graph, extent, &mut lines);
        let mut slot = slot.borrow_mut();
        slot.lines.extend(lines);
        slot.pass = Some(pass);
        rewritten
    })
}

/// Take the lines the installed pass has recorded since they were last taken.
pub fn take_graph_pass_lines() -> Vec<String> {
    GRAPH_PASS.with(|slot| std::mem::take(&mut slot.borrow_mut().lines))
}

//! Which expression a node lowered from, and which slot of it.
//! See docs/notes/lowering-map.md.

use std::collections::HashMap;

use lichen_lowlevel::NodeId;

use crate::checker::{Build, ExprState};
use crate::ir::{ExprId, Loc, LocStep};
use crate::program::{HighProgram, ValueType};
use crate::shape;

/// The pair slot the leading step of `path` names.
///
/// # Invariant
/// `None` means the path names the expression as a whole or enters a structure,
/// neither of which is one slot.
pub fn slot_of_path(path: &[LocStep]) -> Option<usize> {
    match path.first() {
        Some(LocStep::Value) => Some(shape::PAIR_VALUE_SLOT),
        Some(LocStep::Type) => Some(shape::PAIR_TYPE_SLOT),
        Some(LocStep::Attr(index)) => Some(shape::attr_slot(*index)),
        _ => None,
    }
}

/// Every compiled slot of `expr`, each with the location naming it.
///
/// # Invariant
/// The attribute slot is **absent**: `ExprState::attr` records one node without
/// its index in the merged tail, so the checker stamps that node itself where
/// it knows the index (see `Checker::check_ann`).
fn slots_of(expr: ExprId, state: &ExprState) -> Vec<(NodeId, Loc)> {
    let step = |path: Vec<LocStep>| Loc { expr, path };
    [
        (state.term, step(Vec::new())),
        (state.val, step(vec![LocStep::Value])),
        (state.ty, step(vec![LocStep::Type])),
    ]
    .into_iter()
    .filter_map(|(node, loc)| Some((node?, loc)))
    .collect()
}

/// The lowering map: one entry per node this build attributed to an expression.
fn compile<P>(build: &Build<P>) -> HashMap<NodeId, Loc>
where
    P: HighProgram,
    P::Value: ValueType,
{
    let mut map: HashMap<NodeId, Loc> = HashMap::new();
    // The checker's own edges carry the nodes the slot layout does not own.
    map.extend(
        build
            .node_edges
            .iter()
            .map(|(&node, loc)| (node, loc.clone())),
    );
    // The slot layout fills the rest.
    for (index, state) in build.state.iter().enumerate() {
        for (node, loc) in slots_of(ExprId(index as u32), state) {
            map.entry(node).or_insert(loc);
        }
    }
    map
}

impl<P: HighProgram> Build<P>
where
    P::Value: ValueType,
{
    /// Every node this build attributed to an expression.
    /// See docs/notes/lowering-map.md.
    ///
    /// # Invariant
    /// The map is **total over the compiled slots**: an expression's pair,
    /// value and type are all here whether or not a check failed. A node
    /// outside them — an operation's own answer, a cell a unification created —
    /// names no expression, because nothing in this build knows which one
    /// asked for it.
    pub fn lowerings(&self) -> &HashMap<NodeId, Loc> {
        self.lowerings.get_or_init(|| compile(self))
    }

    /// The expression `node` lowered from.
    ///
    /// # Invariant
    /// `None` when this build attributed it to nothing. A clone resolves through
    /// its origin first, so a node an apply materialized answers for the
    /// template it instantiates — the same handoff
    /// [`lichen_lowlevel::Module::marks_of`] makes for a mark.
    pub fn lowering_of(&self, node: NodeId) -> Option<&Loc> {
        let anchored = self.module.node_origin(node).unwrap_or(node);
        self.lowerings().get(&anchored)
    }

    /// The pair slot `loc` names.
    ///
    /// # Invariant
    /// `None` when the path names the expression as a whole or enters a
    /// structure; see [`slot_of_path`].
    pub fn lowering_slot(loc: &Loc) -> Option<usize> {
        slot_of_path(&loc.path)
    }
}

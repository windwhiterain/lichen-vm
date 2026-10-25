//! The template instantiation: the walk that clones a template's value graph.
//!
//! # Invariant
//! The walk is the apply's own clone walk with two decisions replaced: membership
//! is reachability from the root, and freshness is the cell's own closure. Every
//! other rule — one clone per source cell, the operator travelling with its
//! operand, the class topology rebuilt by the shared regroup — is the apply's.
//! See `docs/notes/template-instantiation.md`.

use std::collections::{HashMap, HashSet};

use stacksafe::stacksafe;

use crate::{
    AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue, BlockId, FunctionId, LowValue,
    Module, NodeId, Operation, Program, StaticNodeId, TableItem,
};
use lichen_utils::disjoint;
use lichen_utils::extend::AsEnum;

/// The cells a template instantiation **shares** with the caller instead of cloning.
///
/// # Invariant
/// A marked cell is referenced in place: the instance's graph names the caller's
/// own cell, so the dependency the template states on it is kept, and the
/// marked cell's own content is the caller's, not the instance's.
#[derive(Debug, Clone, Default)]
pub struct TemplateBoundary {
    shared: HashSet<NodeId>,
}

impl TemplateBoundary {
    /// A boundary with nothing marked: every cell of the template is cloned.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `node` as shared: the instance references it, and its content with it.
    pub fn share(&mut self, node: NodeId) -> &mut Self {
        self.shared.insert(node);
        self
    }

    /// [`Self::share`], by value.
    pub fn with(mut self, node: NodeId) -> Self {
        self.share(node);
        self
    }

    /// Whether `node` is marked shared.
    pub fn shares(&self, node: NodeId) -> bool {
        self.shared.contains(&node)
    }

    /// Whether nothing is marked.
    pub fn is_empty(&self) -> bool {
        self.shared.is_empty()
    }

    /// How many cells are marked.
    pub fn len(&self) -> usize {
        self.shared.len()
    }
}

/// One instantiation of a template.
#[derive(Debug, Clone)]
pub struct TemplateInstance {
    /// The instance's own root: the clone of the template's root.
    pub root: NodeId,
    /// The template cell → the instance's cell.
    ///
    /// # Invariant
    /// A template cell the walk referenced in place is absent from the map and *is*
    /// its own instance cell.
    pub remap: HashMap<NodeId, NodeId>,
    /// The template cells the instance references in place.
    pub shared: HashSet<NodeId>,
}

impl TemplateInstance {
    /// The instance's cell for the template cell `template`.
    ///
    /// # Invariant
    /// A template cell the walk referenced in place is its own instance cell.
    pub fn node_of(&self, template: NodeId) -> NodeId {
        self.remap.get(&template).copied().unwrap_or(template)
    }
}

/// The walk's own state: where the clones land, who owns them, and what is shared.
struct TemplateCtx<'a> {
    /// The block every clone lands in.
    target: BlockId,
    /// The owner the clones are registered with, when the caller named one.
    owner: Option<FunctionId>,
    /// The boundary the caller marked.
    boundary: &'a TemplateBoundary,
    /// The template cell → the instance's cell.
    remap: HashMap<NodeId, NodeId>,
    /// The template cells the instance references in place.
    shared: HashSet<NodeId>,
    /// The frozen cell → the instance's cell.
    static_remap: HashMap<StaticNodeId, NodeId>,
    /// Whether a template cell's own content is closed.
    closed: HashMap<NodeId, bool>,
    /// Whether a frozen cell holds a decided value.
    static_closed: HashMap<StaticNodeId, bool>,
    /// Whether a budget stopped the walk.
    exhausted: bool,
}

impl<P: Program> Module<P> {
    /// Instantiate the template rooted at `root`.
    ///
    /// # Invariant
    /// The root is cloned, so an instantiation is an instance — unless the caller
    /// marked the root itself, which shares the whole template. A cell the caller
    /// did not mark is cloned, so the template keeps its own open cells; a marked
    /// cell is referenced in place, so the dependency on an outer binding is kept.
    /// `None` answers a walk a budget stopped: a budget is not a success.
    pub fn clone_template(
        &mut self,
        root: NodeId,
        target: BlockId,
        owner: Option<FunctionId>,
        boundary: &TemplateBoundary,
    ) -> Option<TemplateInstance> {
        if !self.nodes.contains_key(root) {
            return None;
        }
        let mut ctx = TemplateCtx {
            target,
            owner,
            boundary,
            remap: HashMap::new(),
            shared: HashSet::new(),
            static_remap: HashMap::new(),
            closed: HashMap::new(),
            static_closed: HashMap::new(),
            exhausted: false,
        };
        // The closure map comes first: a cell's shareability is a property of its
        // own content, not of the walk's order.
        self.template_closure_map(root, &mut ctx);
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(root) + 1;
        let clone = self.template_clone(root, &mut ctx, true);
        self.stamp_depth = stamp;
        if ctx.exhausted {
            return None;
        }
        self.template_rebuild_classes(&ctx);
        Some(TemplateInstance {
            root: clone,
            remap: ctx.remap,
            shared: ctx.shared,
        })
    }

    /// Whether each template cell of `root`'s graph holds closed content.
    ///
    /// # Invariant
    /// A cell is **open** when its own content is not closed: it holds no value, it
    /// holds the wildcard, it has an operator, or its value's payload reaches an
    /// open cell. A closed cell may be shared; an open one is cloned, so the
    /// instance's own cells are fresh where the template's are open.
    fn template_closure_map(&mut self, root: NodeId, ctx: &mut TemplateCtx<'_>) {
        // The payload graph: the cells a value's payload names, and their owners.
        let mut parents: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut open: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            // A marked cell is the caller's own: its content is the caller's too.
            if ctx.boundary.shares(node) {
                continue;
            }
            let children = self.template_payload_children(node);
            for child in &children {
                match child {
                    Dyn(child) => {
                        parents.entry(*child).or_default().push(node);
                        stack.push(*child);
                    }
                    // A frozen cell that holds nothing cannot be written, so the
                    // cell naming it is open.
                    AnyNodeId::Static(sref) => {
                        if !self.template_static_closed(*sref, ctx) {
                            open.insert(node);
                        }
                    }
                }
            }
            // The operator's operand is a cell of the template.
            if let Some(operation) = self.nodes[node].operation
                && let Some(operand) = operation.operand
            {
                stack.push(operand);
            }
        }
        // The open cells: the leaves open by their own account, and every cell whose
        // payload reaches one.
        for &node in &seen {
            if self.template_cell_is_open(node) {
                open.insert(node);
            }
        }
        // The reach runs upward, so a cycle among the open cells is open as a
        // whole: the graph is a graph, not a tree.
        let mut queue: Vec<NodeId> = open.iter().copied().collect();
        while let Some(node) = queue.pop() {
            let Some(owners) = parents.get(&node) else {
                continue;
            };
            for &owner in owners {
                if open.insert(owner) {
                    queue.push(owner);
                }
            }
        }
        for node in seen {
            let closed = !open.contains(&node);
            ctx.closed.insert(node, closed);
        }
    }

    /// Whether a cell is open by its own account: it holds nothing, it holds the
    /// wildcard, or it has an operator.
    fn template_cell_is_open(&self, node: NodeId) -> bool {
        let cell = &self.nodes[node];
        if cell.operation.is_some() {
            return true;
        }
        let Some(value) = cell.value else {
            return true;
        };
        // A value that is not structural is decided all the same; only the
        // wildcard is decided and never carried.
        matches!(value.as_enum(), Some(LowValue::Any))
    }

    /// The references a cell's own value names: an array's elements, a table's
    /// entries.
    fn template_payload_children(&self, node: NodeId) -> Vec<AnyNodeId> {
        let Some(value) = self.nodes[node].value else {
            return Vec::new();
        };
        match value.as_enum() {
            Some(LowValue::Array(array)) => {
                // SAFETY: `array` is the payload of a live cell of this module, so
                // its home block has not been dropped.
                unsafe { array.items() }
                    .iter()
                    .map(|item| item.node)
                    .collect()
            }
            Some(LowValue::Table(table)) => {
                // SAFETY: as above — `table` is a live cell's payload.
                let mut children = Vec::new();
                for item in unsafe { table.items() } {
                    children.push(item.key);
                    children.push(item.value);
                }
                children
            }
            _ => Vec::new(),
        }
    }

    /// Whether a frozen cell is closed: it holds a decided value.
    ///
    /// # Invariant
    /// The frozen module's own rule governs the cell's interior — an open cell
    /// lives only inside a function (`docs/notes/attribute-isomorphism.md`) — so the
    /// cell's own value decides it.
    fn template_static_closed(&self, sref: StaticNodeId, ctx: &mut TemplateCtx<'_>) -> bool {
        if let Some(&closed) = ctx.static_closed.get(&sref) {
            return closed;
        }
        let closed = self
            .static_read(sref)
            .is_some_and(|value| !matches!(value.as_enum(), Some(LowValue::Any)));
        ctx.static_closed.insert(sref, closed);
        closed
    }

    /// The instance's cell for the template's `source`.
    ///
    /// # Invariant
    /// One template cell has one instance cell: the map is written before the walk
    /// descends, so a diamond names one cell and a cycle names the instance's own.
    #[stacksafe]
    fn template_clone(
        &mut self,
        source: NodeId,
        ctx: &mut TemplateCtx<'_>,
        is_root: bool,
    ) -> NodeId {
        if let Some(&clone) = ctx.remap.get(&source) {
            return clone;
        }
        // The caller marked this cell: it is the caller's own, and its content too.
        if ctx.boundary.shares(source) {
            ctx.shared.insert(source);
            return source;
        }
        if self.depth_exhausted(source) {
            ctx.exhausted = true;
            return source;
        }
        // A closed cell is shared: nothing can be written into it, so the instance
        // gains nothing by copying it.
        if !is_root && ctx.closed.get(&source).copied().unwrap_or(false) {
            ctx.shared.insert(source);
            return source;
        }
        let (value, operation) = {
            let cell = &self.nodes[source];
            (cell.value, cell.operation)
        };
        // The instance's cell is reserved before the walk descends: a diamond names
        // one cell, a cycle the instance's own.
        let clone = self.add_node(ctx.target, None, None);
        ctx.remap.insert(source, clone);
        self.nodes[clone].origin = Some(source);
        self.nodes[clone].low_shape = self.class_low_shape(source);
        if let Some(owner) = ctx.owner {
            self.register_in_function(owner, clone);
        }
        // The operator travels with the instance; its operand is the instance's own.
        let operation = operation.map(|operation| Operation {
            operator: operation.operator,
            operand: operation
                .operand
                .map(|operand| self.template_clone(operand, ctx, false)),
        });
        // The wildcard is decided but never carried: the instance's own cell replaces
        // it, so the caller's value can fill it.
        let wildcard = matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any));
        let carried = match &operation {
            None => !wildcard,
            Some(_) => {
                // A computed answer travels only when the deep pass ran on it: a
                // fact of the template, not of the last call.
                self.nodes[source].evaluated_deep.is_some() && !wildcard
            }
        };
        let mapped = if carried {
            value.map(|value| self.template_map_value(value, ctx, false))
        } else {
            None
        };
        self.write_node_value(clone, mapped);
        // The claim follows the mapped answer, not the carry: an answer whose own
        // elements are open does not claim the run.
        let claimed = self.nodes[source]
            .evaluated_deep
            .is_some_and(|deep| !deep.undecided)
            && mapped.is_some_and(|value| {
                !crate::apply::answer_elements_are_undecided::<P>(value, |node| {
                    self.node_value(node).is_none()
                })
            });
        self.stamp_node_ran_if(clone, claimed);
        self.nodes[clone].operation = operation;
        clone
    }

    /// The instance's own value: the template's payload, every element replaced by
    /// the instance's cell.
    ///
    /// # Invariant
    /// A frozen element that holds a value stays a frozen reference — it is read in
    /// its own module and written by nobody (`docs/notes/attribute-isomorphism.md`);
    /// a frozen element that holds nothing becomes the instance's own cell, because
    /// a frozen cell cannot be written.
    #[stacksafe]
    fn template_map_value(
        &mut self,
        value: P::Value,
        ctx: &mut TemplateCtx<'_>,
        frozen: bool,
    ) -> P::Value {
        match value.as_enum() {
            Some(LowValue::Array(array)) => {
                let mut items = Vec::with_capacity(unsafe { array.items() }.len());
                // SAFETY: `array` is the payload of a live value this walk holds, and
                // the walk releases no block.
                for &item in unsafe { array.items() } {
                    items.push(ArrayItem {
                        node: self.template_map_ref(item.node, ctx, frozen),
                        ..item
                    });
                }
                P::Value::from(LowValue::Array(ArrayValue::with_layout(
                    self.alloc_array(&items, ctx.target),
                    array.layout(),
                )))
            }
            Some(LowValue::Table(table)) => {
                // SAFETY: `table` is the payload of a live value this walk holds.
                let items: Vec<TableItem> = unsafe { table.items() }
                    .iter()
                    .map(|item| TableItem {
                        key: self.template_map_ref(item.key, ctx, frozen),
                        value: self.template_map_ref(item.value, ctx, frozen),
                        hash: item.hash,
                    })
                    .collect();
                P::Value::from(LowValue::Table(self.alloc_table(&items, ctx.target)))
            }
            // A function value names a function: the instance shares it, as the
            // template named it.
            _ => value,
        }
    }

    /// The instance's cell for one payload reference.
    #[stacksafe]
    fn template_map_ref(
        &mut self,
        reference: AnyNodeId,
        ctx: &mut TemplateCtx<'_>,
        frozen: bool,
    ) -> AnyNodeId {
        match reference {
            Dyn(_) if frozen => reference,
            Dyn(node) => Dyn(self.template_clone(node, ctx, false)),
            AnyNodeId::Static(sref) => {
                // A frozen cell that holds a value is read in place: it is closed.
                if self.template_static_closed(sref, ctx) {
                    return reference;
                }
                // An open frozen cell cannot be written: it becomes this instance's own.
                Dyn(self.template_materialize_static(sref, ctx))
            }
        }
    }

    /// The instance's own cell for a frozen cell that holds nothing.
    #[stacksafe]
    fn template_materialize_static(
        &mut self,
        sref: StaticNodeId,
        ctx: &mut TemplateCtx<'_>,
    ) -> NodeId {
        if let Some(&clone) = ctx.static_remap.get(&sref) {
            return clone;
        }
        let clone = self.add_node(ctx.target, None, None);
        ctx.static_remap.insert(sref, clone);
        // The frozen value's payload: the instance's own cells where the frozen ones
        // are open.
        let value = self.static_read(sref);
        let mapped = value.map(|value| self.template_map_value(value, ctx, true));
        self.write_node_value(clone, mapped);
        clone
    }

    /// The instance's cells, grouped by the template's equality classes.
    ///
    /// # Invariant
    /// The groups are the template's own classes, so the instance's topology is the
    /// template's: two template cells that are one expression are one instance cell.
    /// A shared cell joins its own class's group, so the instance's cell for a cell
    /// that is one expression with it is one expression with it too.
    fn template_rebuild_classes(&mut self, ctx: &TemplateCtx<'_>) {
        let mut entries: Vec<(NodeId, NodeId)> = ctx
            .remap
            .iter()
            .map(|(&template, &clone)| (template, clone))
            .collect();
        entries.extend(ctx.shared.iter().map(|&node| (node, node)));
        if entries.len() < 2 {
            return;
        }
        let groups = crate::apply::regroup_clones(entries, |template| {
            disjoint::find(&mut self.nodes, template)
        });
        crate::apply::unify_clone_groups(groups, |first, clone| {
            self.unify(first, clone);
        });
    }
}

//! The template instantiation: the walk that clones a template's value graph.
//!
//! # Invariant
//! The walk is the apply's own clone walk with two decisions replaced:
//! membership is the class's scope, and freshness is the cell's own closure.
//! See `docs/notes/template-instantiation.md`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use stacksafe::stacksafe;

use crate::{
    AnyNodeId, AnyNodeId::Dynamic as Dyn, ArrayItem, ArrayValue, BlockId, FunctionId, LocalNodeId,
    LowValue, Module, NodeId, Operation, Program, Scope, StaticFunctionRef, StaticNodeId,
    TableItem, ValueExt,
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
    /// The scope the walk's decisions are made in: the scope of the function
    /// that owns the template's root.
    ///
    /// # Invariant
    /// The scope names the function the template was written in, so a class
    /// whose members reach outside it is shared whole rather than cloned.
    scope: Option<Arc<Scope<FunctionId>>>,
    /// The class verdicts this walk has judged, keyed by the class's root.
    ///
    /// # Invariant
    /// One entry per class judged, so every member of one class is cloned or
    /// shared alike and a class is never split by the instance.
    verdicts: HashMap<NodeId, bool>,
    /// The boundary the caller marked.
    boundary: &'a TemplateBoundary,
    /// The template cell → the instance's cell.
    remap: HashMap<NodeId, NodeId>,
    /// The template cells the instance references in place.
    shared: HashSet<NodeId>,
    /// The frozen cell → the instance's cell.
    static_remap: HashMap<StaticNodeId, NodeId>,
    /// The frozen cell → its class representative.
    ///
    /// # Invariant
    /// The immutable union-find is read once per frozen cell per pass; every
    /// later verdict uses the cached representative.
    static_representatives: HashMap<StaticNodeId, LocalNodeId>,
    /// Frozen class verdicts, keyed by source function and representative.
    ///
    /// # Invariant
    /// Every member of one frozen class reads the same verdict in one source
    /// scope; a different module or source scope has a different entry.
    static_verdicts: HashMap<(StaticFunctionRef, LocalNodeId), bool>,
    /// Whether a template cell's own content is closed.
    closed: HashMap<NodeId, bool>,
    /// Whether a frozen cell's own content is closed.
    static_closed: HashMap<StaticNodeId, bool>,
    /// Whether a budget stopped the walk.
    exhausted: bool,
}

impl<P: Program> Module<P> {
    /// Instantiate the template rooted at `root`.
    ///
    /// # Invariant
    /// A cell the caller did not mark is cloned when its **class** lies inside
    /// the template's own scope and the class needs a fresh cell; otherwise the
    /// class is referenced in place, as a whole. A marked cell is referenced in
    /// place, so the dependency on an outer binding is kept. `None` answers a
    /// walk a budget stopped: a budget is not a success.
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
        // The template's own scope: the function that wrote the root cell.
        let scope = self.nodes[root]
            .function
            .and_then(|owner| self.function_scope(owner));
        let mut ctx = TemplateCtx {
            target,
            owner,
            scope,
            verdicts: HashMap::new(),
            boundary,
            remap: HashMap::new(),
            shared: HashSet::new(),
            static_remap: HashMap::new(),
            static_representatives: HashMap::new(),
            static_verdicts: HashMap::new(),
            closed: HashMap::new(),
            static_closed: HashMap::new(),
            exhausted: false,
        };
        // The closure map comes first: a cell's shareability is a property of its
        // own content, not of the walk's order.
        self.template_closure_map(root, &mut ctx);
        let stamp = self.stamp_depth;
        self.stamp_depth = self.node_depth(root) + 1;
        let clone = self.template_clone(root, &mut ctx);
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
    /// holds the wildcard, it has an operator, or its content reaches an open cell.
    /// A closed cell may be shared; an open one is cloned, so the instance's own
    /// cells are fresh where the template's are open. A **trace edge** — a cell a
    /// plugin value keeps alive — decides closure, but is not cloned.
    fn template_closure_map(&mut self, root: NodeId, ctx: &mut TemplateCtx<'_>) {
        // A **payload edge** names a cell the value holds; the walk clones it.
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if !seen.insert(node) {
                continue;
            }
            for child in self.template_payload_children(node) {
                if let Dyn(child) = child {
                    stack.push(child);
                }
            }
            for child in self.template_traced_children(node) {
                stack.push(child);
            } // The operator's operand is a cell of the template, with no parent
            // edge: an operator's input is not the cell's content.
            if let Some(operation) = self.nodes[node].operation
                && let Some(operand) = operation.operand
            {
                stack.push(operand);
            }
        }
        // The verdict per cell: the same proof the apply's walk uses, so a
        // closed cell is closed for both.
        let mut memo: HashMap<NodeId, Option<bool>> = HashMap::new();
        for node in seen {
            let closed = self.template_content_is_closed(node, Some(ctx.boundary), &mut memo);
            ctx.closed.insert(node, closed);
        }
    }

    /// Whether a cell's own content is **closed**.
    ///
    /// # Invariant
    /// A closed cell holds a decided value that is not the wildcard, it has
    /// no operator, and every cell its content reaches is closed too. A cycle
    /// among decided cells is closed: the reach runs over decided content
    /// only, so a cycle is a fact about the content, not a hole. A **marked**
    /// cell is the caller's own: the reach stops there.
    pub(crate) fn template_content_is_closed(
        &mut self,
        node: NodeId,
        boundary: Option<&TemplateBoundary>,
        memo: &mut HashMap<NodeId, Option<bool>>,
    ) -> bool {
        match memo.get(&node) {
            Some(Some(closed)) => return *closed,
            // A cell on the current reach: a cycle among decided cells.
            Some(None) => return true,
            None => {}
        }
        memo.insert(node, None);
        let mut closed = !self.template_cell_is_open(node);
        let marked = boundary.is_some_and(|boundary| boundary.shares(node));
        if closed && !marked {
            for child in self.template_content_children(node) {
                if !self.template_content_is_closed(child, boundary, memo) {
                    closed = false;
                    break;
                }
            }
        }
        memo.insert(node, Some(closed));
        closed
    }

    /// The cells a cell's own content reaches: the ones its value names.
    fn template_content_children(&self, node: NodeId) -> Vec<NodeId> {
        let mut children: Vec<NodeId> = self
            .template_payload_children(node)
            .into_iter()
            .filter_map(|child| match child {
                Dyn(child) => Some(child),
                // A frozen cell is read in its own module; it is not a cell of
                // this one, so it cannot decide this cell's closure.
                AnyNodeId::Static(_) => None,
            })
            .collect();
        children.extend(self.template_traced_children(node));
        children
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

    /// The cells a cell's own value keeps alive: the edges the GC follows that the
    /// graph does not name.
    fn template_traced_children(&self, node: NodeId) -> Vec<NodeId> {
        let Some(value) = self.nodes[node].value else {
            return Vec::new();
        };
        let mut traced = Vec::new();
        value.traced(&*self, &mut traced);
        traced
    }

    /// Whether a frozen cell is closed: it holds a decided value whose own content
    /// is closed too.
    ///
    /// # Invariant
    /// A frozen cell is read in its own module and written by nobody, so an open
    /// cell inside it can never be filled: the cell that names it must be the
    /// instance's own, and the frozen one must not be shared.
    fn template_static_closed(&self, sref: StaticNodeId, ctx: &mut TemplateCtx<'_>) -> bool {
        if let Some(&closed) = ctx.static_closed.get(&sref) {
            return closed;
        }
        // The frozen payload graph: the cells a frozen value names, and their owners.
        let mut parents: HashMap<StaticNodeId, Vec<StaticNodeId>> = HashMap::new();
        let mut seen: HashSet<StaticNodeId> = HashSet::new();
        let mut stack = vec![sref];
        while let Some(cell) = stack.pop() {
            if !seen.insert(cell) {
                continue;
            }
            for child in self.template_static_children(cell) {
                parents.entry(child).or_default().push(cell);
                stack.push(child);
            }
        }
        // The open frozen cells: no value, the wildcard, or an operator that re-runs.
        let mut open: HashSet<StaticNodeId> = HashSet::new();
        for &cell in &seen {
            if self.template_static_cell_is_open(cell) {
                open.insert(cell);
            }
        }
        // The reach runs upward, so a cycle among the open frozen cells is open as
        // a whole: the graph is a graph, not a tree.
        let mut queue: Vec<StaticNodeId> = open.iter().copied().collect();
        while let Some(cell) = queue.pop() {
            let Some(owners) = parents.get(&cell) else {
                continue;
            };
            for &owner in owners {
                if open.insert(owner) {
                    queue.push(owner);
                }
            }
        }
        for cell in seen {
            let closed = !open.contains(&cell);
            ctx.static_closed.insert(cell, closed);
        }
        ctx.static_closed[&sref]
    }

    /// Whether a frozen cell is open by its own account.
    fn template_static_cell_is_open(&self, sref: StaticNodeId) -> bool {
        let module = self.static_module(sref.module);
        let Some(cell) = module.nodes.get(sref.index.index) else {
            return true;
        };
        if cell.operation.is_some() {
            return true;
        }
        let Some(value) = cell.value else {
            return true;
        };
        matches!(value.as_enum(), Some(LowValue::Any))
    }

    /// The frozen cells a frozen cell's own value names.
    fn template_static_children(&self, sref: StaticNodeId) -> Vec<StaticNodeId> {
        let Some(value) = self.static_read(sref) else {
            return Vec::new();
        };
        let children = match value.as_enum() {
            // SAFETY: the payload is a frozen module's arena, which its
            // registration holds alive for as long as the ref can be read.
            Some(LowValue::Array(array)) => unsafe { array.items() }
                .iter()
                .map(|item| item.node)
                .collect(),
            Some(LowValue::Table(table)) => {
                let mut children = Vec::new();
                for item in unsafe { table.items() } {
                    children.push(item.key);
                    children.push(item.value);
                }
                children
            }
            _ => Vec::new(),
        };
        children
            .into_iter()
            .filter_map(|child| match child {
                AnyNodeId::Static(child) => Some(child),
                // A frozen value names only frozen cells; a dynamic one is not a
                // cell of the frozen module and cannot be inspected.
                AnyNodeId::Dynamic(_) => None,
            })
            .collect()
    }

    /// Whether the class of `source` is cloned by this walk.
    ///
    /// # Invariant
    /// The verdict is a fact of the class: every member of one class is cloned
    /// or shared alike, so the instance never splits a class. A class is cloned
    /// when it lies inside the walk's scope **and** it needs a fresh cell — one
    /// of its members is open. The root's own class is cloned whatever it
    /// holds: an instantiation is an instance.
    fn template_class_is_cloned(&mut self, source: NodeId, ctx: &mut TemplateCtx<'_>) -> bool {
        let representative = disjoint::find(&mut self.nodes, source);
        if let Some(&verdict) = ctx.verdicts.get(&representative) {
            return verdict;
        }
        let verdict = self.template_class_verdict(representative, ctx);
        ctx.verdicts.insert(representative, verdict);
        verdict
    }

    /// The verdict of one class: cloned, or shared as a whole.
    fn template_class_verdict(
        &mut self,
        representative: NodeId,
        ctx: &mut TemplateCtx<'_>,
    ) -> bool {
        // The scope axis: every member of the class must lie inside the scope,
        // or the class is shared whole.
        if let Some(scope) = ctx.scope.clone()
            && !self.class_inside_scope(representative, &scope)
        {
            return false;
        }
        // The freshness axis: the class needs a fresh cell when one of its
        // members is open.
        self.template_class_needs_freshness(representative, ctx)
    }

    /// Whether one member of the class is open, so the class needs a fresh cell.
    fn template_class_needs_freshness(
        &self,
        representative: NodeId,
        ctx: &TemplateCtx<'_>,
    ) -> bool {
        let mut member = Some(representative);
        while let Some(current) = member {
            member = self.nodes[current].equality.next();
            // A cell the walk never reached is not one of the class's open
            // cells: the walk does not instance it.
            if !ctx.closed.get(&current).copied().unwrap_or(true) {
                return true;
            }
        }
        false
    }

    /// The instance's cell for the template's `source`.
    ///
    /// # Invariant
    /// One template cell has one instance cell: the map is written before the walk
    /// descends, so a diamond names one cell and a cycle names the instance's own.
    #[stacksafe]
    fn template_clone(&mut self, source: NodeId, ctx: &mut TemplateCtx<'_>) -> NodeId {
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
        // The **class** decides: the whole class is cloned or the whole class is
        // shared, so the instance never splits one.
        if !self.template_class_is_cloned(source, ctx) {
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
                .map(|operand| self.template_clone(operand, ctx)),
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
            value.map(|value| self.template_map_value(value, source, ctx, false))
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
    /// A frozen element that holds a decided value and is closed stays a frozen
    /// reference — it is read in its own module and written by nobody
    /// (`docs/notes/attribute-isomorphism.md`); a frozen element that is open
    /// becomes the instance's own cell, because a frozen cell cannot be written.
    #[stacksafe]
    fn template_map_value(
        &mut self,
        value: P::Value,
        named_by: NodeId,
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
                        node: self.template_map_ref(item.node, named_by, ctx, frozen),
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
                        key: self.template_map_ref(item.key, named_by, ctx, frozen),
                        value: self.template_map_ref(item.value, named_by, ctx, frozen),
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

    /// Whether a frozen cell's class lies inside the template's source scope.
    ///
    /// # Invariant
    /// Only the template scope's own function supplies a source scope, through
    /// a same-module `static_origin`. No origin, module, or scope is external;
    /// a dynamic ancestor is never a substitute lexical scope.
    fn template_static_inside_scope(
        &mut self,
        sref: StaticNodeId,
        ctx: &mut TemplateCtx<'_>,
    ) -> bool {
        let Some(scope) = ctx.scope.clone() else {
            return false;
        };
        let Some(origin) = self
            .functions
            .get(scope.function())
            .and_then(|entry| entry.static_origin)
        else {
            return false;
        };
        if origin.module != sref.module {
            return false;
        }
        let module = self.static_module(sref.module);
        let Some(source_scope) = module.static_function_scope(origin.index) else {
            return false;
        };
        let representative = if let Some(&representative) = ctx.static_representatives.get(&sref) {
            representative
        } else {
            let representative = crate::static_module::static_find(&module.nodes, sref.index);
            ctx.static_representatives.insert(sref, representative);
            representative
        };
        let key = (origin, representative);
        if let Some(&verdict) = ctx.static_verdicts.get(&key) {
            return verdict;
        }
        let verdict = module.static_class_inside_scope(representative, &source_scope);
        ctx.static_verdicts.insert(key, verdict);
        verdict
    }

    /// The instance's cell for one payload reference.
    #[stacksafe]
    fn template_map_ref(
        &mut self,
        reference: AnyNodeId,
        named_by: NodeId,
        ctx: &mut TemplateCtx<'_>,
        frozen: bool,
    ) -> AnyNodeId {
        match reference {
            Dyn(_) if frozen => reference,
            Dyn(node) => Dyn(self.template_clone(node, ctx)),
            AnyNodeId::Static(sref) => {
                // A frozen cell that is closed is read in place: it holds a value
                // nothing can write into.
                if self.template_static_closed(sref, ctx) {
                    return reference;
                }
                // An open frozen cell cannot be written: it becomes this instance's
                // own cell.
                Dyn(self.template_materialize_static(sref, named_by, ctx))
            }
        }
    }

    /// The instance's own cell for a frozen cell that is open.
    ///
    /// # Invariant
    /// One source cell has one canonical image per pass. An external class's
    /// image is unowned; an inside class's image belongs to the instance owner.
    /// The frozen artifact is never written.
    #[stacksafe]
    fn template_materialize_static(
        &mut self,
        sref: StaticNodeId,
        named_by: NodeId,
        ctx: &mut TemplateCtx<'_>,
    ) -> NodeId {
        let module = self.static_module(sref.module);
        let (undecided, operation, evaluated_deep) = {
            let node = &module.nodes[sref.index.index];
            (node.undecided(), node.operation, node.evaluated_deep)
        };
        let inside = !undecided || self.template_static_inside_scope(sref, ctx);
        if let Some(&clone) = ctx.static_remap.get(&sref) {
            if undecided
                && inside
                && self.nodes[clone].function.is_none()
                && let Some(owner) = ctx.owner
            {
                self.register_in_function(owner, clone);
            }
            return clone;
        }
        let clone = self.add_node(ctx.target, None, None);
        ctx.static_remap.insert(sref, clone);
        self.nodes[clone].origin = Some(named_by);
        // The frozen cell's own shape is the clone's shape.
        let shape = module.nodes[sref.index.index].low_shape.clone();
        if let Some(shape) = shape {
            self.seed_class_low_type(clone, shape);
        }
        if undecided
            && inside
            && let Some(owner) = ctx.owner
        {
            self.register_in_function(owner, clone);
        }
        // The operator and its operand travel through the same canonical map.
        let operation = operation.map(|operation| Operation {
            operator: operation.operator,
            operand: operation.operand.map(|operand| {
                self.template_materialize_static(
                    StaticNodeId {
                        module: sref.module,
                        index: operand,
                    },
                    clone,
                    ctx,
                )
            }),
        });
        let value = self.static_read(sref);
        let wildcard = matches!(value.and_then(|value| value.as_enum()), Some(LowValue::Any));
        let carried = match operation {
            None => !wildcard,
            Some(_) => evaluated_deep.is_some() && !wildcard,
        };
        let mapped = if carried {
            value.map(|value| self.template_map_value(value, clone, ctx, true))
        } else {
            None
        };
        self.write_node_value(clone, mapped);
        let claimed = evaluated_deep.is_some_and(|deep| !deep.undecided)
            && mapped.is_some_and(|value| {
                !crate::apply::answer_elements_are_undecided::<P>(value, |node| {
                    self.node_value(node).is_none()
                })
            });
        self.stamp_node_ran_if(clone, claimed);
        self.nodes[clone].operation = operation;
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
        if entries.len() > 1 {
            let groups = crate::apply::regroup_clones(entries, |template| {
                disjoint::find(&mut self.nodes, template)
            });
            crate::apply::unify_clone_groups(groups, |first, clone| {
                self.unify(first, clone);
            });
        }
        // Frozen classes rebuild inside their own module: local indices from
        // different modules never share a group.
        let entries: Vec<_> = ctx
            .static_remap
            .iter()
            .map(|(&sref, &clone)| ((sref.module, sref.index), clone))
            .collect();
        if entries.len() < 2 {
            return;
        }
        let groups = crate::apply::regroup_clones(entries, |(module, index)| {
            let frozen = self.static_module(module);
            (
                module,
                crate::static_module::static_find(&frozen.nodes, index),
            )
        });
        crate::apply::unify_clone_groups(groups, |first, clone| {
            self.unify(first, clone);
        });
    }
}

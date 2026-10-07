//! The abstract-interpretation pass: low types for a function **template**,
//! computed before any apply.
//!
//! Observation alone cannot compile a kernel pre-apply.  A template is never
//! evaluated, and an apply binds the *clones*, so nothing is ever written onto
//! a template's own parameter cell: observation is silent there, and so is the
//! merge join.  This module is the third and last computation route — a
//! fixed-point pass over the template's value graph, seeded by the caller
//! (see [`docs/notes/lowlevel-low-types.md`]).
//!
//! # What is deliberately *not* here
//!
//! The pass never learns the `[value, type]` pair layout, and it never invents
//! a parameter's type.  Seeding is the caller's job — a layer above the
//! lowlevel decodes the parameter's *type slot* and calls
//! [`Module::seed_class_low_type`].  That is what keeps the lowlevel honest:
//! the graph facts (a transfer per operator, a class per value) live here, the
//! type facts live with whoever owns the encoding.
//!
//! # Termination
//!
//! Every refinement is monotone on a finite lattice — a class's low type only
//! ever leaves [`LowShape::Unknown`] — and the worklist re-runs a node only
//! when one of its operands' classes actually changed.  So the pass halts at a
//! fixed point even on a cyclic or recursive template; a position the seeds
//! and the transfers never decide simply stays undecided, which is the honest
//! answer rather than a wrong one.

use std::collections::{HashMap, HashSet, VecDeque};

use lichen_utils::extend::AsEnum;

use crate::{
    AnyNodeId, AnyNodeId::Dynamic as Dyn, FunctionId, LowOperator, LowShape, LowValue, Module,
    NodeId, OperatorExt as _, Program,
};

impl<P: Program> Module<P> {
    /// Compute the low types of a function **template**'s body, to a fixed
    /// point, refining each class through [`Module::refine_class_low_type`].
    ///
    /// Run it at the moment a backend needs the answer — pay-per-use, so a
    /// program that never compiles a template never pays.  Seed the parameter
    /// classes with [`Module::seed_class_low_type`] first: the pass has no seed
    /// of its own, and an unseeded parameter position stays
    /// [`LowShape::Unknown`], which is exactly the pre-apply signal a backend
    /// treats conservatively.
    ///
    /// The template's own value graph is the whole domain — a module-level
    /// constant a body closes over keeps the low type observation gave it, and
    /// a node outside the template is not refined by this pass.
    pub fn infer_template_low_types(&mut self, function: FunctionId) {
        let members: Vec<NodeId> = self.functions[function].nodes.clone();
        let scope: HashSet<NodeId> = members.iter().copied().collect();
        // Reverse operand edges, restricted to the template: a node is
        // re-queued exactly when one of the classes its transfer reads has
        // moved.  Edges leaving the template are dropped — the body reads a
        // module-level constant's low type, it does not decide it.
        let mut users: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for &node in &members {
            for operand in self.operand_elements(node) {
                if scope.contains(&operand) {
                    users.entry(operand).or_default().push(node);
                }
            }
        }
        let mut queue: VecDeque<NodeId> = members.iter().copied().collect();
        let mut queued: HashSet<NodeId> = members.iter().copied().collect();
        while let Some(node) = queue.pop_front() {
            queued.remove(&node);
            // Read-only transfer, then the single write side.  A transfer that
            // declines, or a class that does not move, schedules nothing.
            let Some(shape) = self.low_type_transfer(node) else {
                continue;
            };
            if !self.refine_class_low_type(node, shape) {
                continue;
            }
            for &user in users.get(&node).map_or(&[][..], Vec::as_slice) {
                if queued.insert(user) {
                    queue.push_back(user);
                }
            }
        }
    }

    /// The transfer function of one node: the low type its result has, given
    /// the low types its operand array's elements currently have.
    ///
    /// `None` means "this node states nothing" — the class is left as
    /// observation, the seed, and the other passes left it.  A leaf (no
    /// operation) always declines: the pass is a *computation* route, and a
    /// leaf has no computation.
    fn low_type_transfer(&self, node: NodeId) -> Option<LowShape> {
        let operation = self.nodes.get(node)?.operation?;
        let Some(structural) = AsEnum::<LowOperator>::as_enum(&operation.operator) else {
            // Not a structural operator: ask the vocabulary that defined it.
            return self.extension_low_type(&operation.operator, self.operand_low_types(node));
        };
        Some(match structural {
            // `Index(container, k)` yields the container's element at `k`.  The
            // index must be a decided scalar — an index that is still a
            // parameter is exactly the pre-apply case this pass cannot decide,
            // and a mis-selected element would be a wrong type, not an
            // undecided one.
            LowOperator::Index => {
                let container = self.low_type_of_node(self.operand_at(node, 0)?)?;
                let index = self.constant_index(self.operand_at(node, 1)?)?;
                match container {
                    LowShape::Tuple(items) => items.get(index)?.clone(),
                    LowShape::Array(element, length) if index < length => *element,
                    _ => return None,
                }
            }
            // `Apply(callee, _)` yields the callee's codomain.
            LowOperator::Apply => {
                let LowShape::Function(_, codomain) =
                    self.low_type_of_node(self.operand_at(node, 0)?)?
                else {
                    return None;
                };
                *codomain
            }
            // `TableGet(table, _)` yields the table's value shape.  A miss
            // computes nothing (`Error`, which states no shape), so the table's
            // value shape is still the right answer.
            LowOperator::TableGet => {
                let LowShape::Table(_, value) = self.low_type_of_node(self.operand_at(node, 0)?)?
                else {
                    return None;
                };
                *value
            }
        })
    }

    /// The low type an extension operator states for its result, from the
    /// low types of its operand array's elements.  The dispatch mirrors the
    /// VM's: the structural leaves are already handled by
    /// [`Self::low_type_transfer`], so reaching an extension leaf here is the
    /// ordinary case, and an operator that declines leaves its result
    /// undecided.
    fn extension_low_type(
        &self,
        operator: &P::Operator,
        arguments: Vec<Option<LowShape>>,
    ) -> Option<LowShape> {
        operator.low_type(&arguments)
    }

    /// The operand array's elements, as dynamic node ids — the argument vector
    /// every transfer and every extension hook is phrased over.  A static ref
    /// is a decided leaf with no class to refine, so it contributes nothing.
    fn operand_elements(&self, node: NodeId) -> Vec<NodeId> {
        let Some(operation) = self.nodes.get(node).and_then(|node| node.operation) else {
            return Vec::new();
        };
        let Some(operand) = operation.operand else {
            return Vec::new();
        };
        // SAFETY: the operand node is read out of `self.nodes` on this borrow,
        // so its home block — and the arena the array payload lives in — stays
        // alive for as long as `items` is walked below.
        let Some(items) = (unsafe { self.array_items(operand) }) else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| match item.node {
                Dyn(node) => Some(node),
                AnyNodeId::Static(_) => None,
            })
            .collect()
    }

    /// The low types of `node`'s operand array's elements, positionally.
    fn operand_low_types(&self, node: NodeId) -> Vec<Option<LowShape>> {
        self.operand_elements(node)
            .into_iter()
            .map(|element| self.low_type_of_node(element))
            .collect()
    }

    /// The `index`-th dynamic element of `node`'s operand array.
    fn operand_at(&self, node: NodeId, index: usize) -> Option<NodeId> {
        self.operand_elements(node).into_iter().nth(index)
    }

    /// A node's value read as a decided `USize` constant — the only shape of
    /// index the `Index` transfer accepts.  An undecided or computed-later index
    /// states nothing, which is what keeps a data-dependent read undecided
    /// rather than picking an element.
    fn constant_index(&self, node: NodeId) -> Option<usize> {
        let Some(LowValue::USize(index)) = self.node_value(Dyn(node))?.as_enum() else {
            return None;
        };
        Some(index)
    }
}

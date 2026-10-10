//! Abstract interpretation: a template's low types, to a fixed point.
//! See docs/notes/lowlevel-low-types.md §2.1.

use std::collections::{HashMap, HashSet, VecDeque};

use lichen_utils::extend::AsEnum;

use crate::{
    AnyNodeId, AnyNodeId::Dynamic as Dyn, FunctionId, LowOperator, LowShape, LowValue, Module,
    NodeId, OperatorExt as _, Program,
};

impl<P: Program> Module<P> {
    /// Compute a template's low types to a fixed point, refining each class
    /// through [`Module::refine_class_low_type`].
    ///
    /// # Invariant
    ///
    /// Seed the parameter classes with [`Module::seed_class_low_type`] first: the
    /// pass has no seed of its own, so an unseeded position stays
    /// [`LowShape::Unknown`], the signal a backend treats conservatively. The
    /// template's own value graph is the whole domain — a node outside it is
    /// not refined here.
    pub fn infer_template_low_types(&mut self, function: FunctionId) {
        let members: Vec<NodeId> = self.functions[function].nodes.clone();
        let scope: HashSet<NodeId> = members.iter().copied().collect();
        // Reverse operand edges, restricted to the template: a node is
        // re-queued when one of its operands' classes moves.
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
            // A transfer that declines, or a class that does not move,
            // schedules nothing.
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
    /// its operand elements' current low types.
    ///
    /// # Invariant
    ///
    /// `None` states nothing — the class is left as observation, the seed and
    /// the other passes left it. A leaf always declines: the pass is a
    /// *computation* route, and a leaf has no computation.
    fn low_type_transfer(&self, node: NodeId) -> Option<LowShape> {
        let operation = self.nodes.get(node)?.operation?;
        let Some(structural) = AsEnum::<LowOperator>::as_enum(&operation.operator) else {
            // Not a structural operator: ask the vocabulary that defined it.
            return self.extension_low_type(&operation.operator, self.operand_low_types(node));
        };
        Some(match structural {
            // `Index(container, k)` yields the element at `k`; the index must
            // be decided, or a mis-selected element is a wrong type.
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
            // `TableGet(table, _)` yields the table's value shape: a miss
            // computes `Error`, which states no shape.
            LowOperator::TableGet => {
                let LowShape::Table(_, value) = self.low_type_of_node(self.operand_at(node, 0)?)?
                else {
                    return None;
                };
                *value
            }
        })
    }

    /// The low type an extension operator states for its result; the
    /// structural leaves are `low_type_transfer`'s own.
    fn extension_low_type(
        &self,
        operator: &P::Operator,
        arguments: Vec<Option<LowShape>>,
    ) -> Option<LowShape> {
        operator.low_type(&arguments)
    }

    /// The operand array's elements, as dynamic node ids; a static ref has
    /// no class to refine.
    fn operand_elements(&self, node: NodeId) -> Vec<NodeId> {
        let Some(operation) = self.nodes.get(node).and_then(|node| node.operation) else {
            return Vec::new();
        };
        let Some(operand) = operation.operand else {
            return Vec::new();
        };
        // SAFETY: the operand is read out of `self.nodes` on this borrow, so
        // its arena stays alive while `items` is walked.
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

    /// A node's value read as a decided `USize` constant — the only index the
    /// `Index` transfer accepts.
    fn constant_index(&self, node: NodeId) -> Option<usize> {
        let Some(LowValue::USize(index)) = self.node_value(Dyn(node))?.as_enum() else {
            return None;
        };
        Some(index)
    }
}

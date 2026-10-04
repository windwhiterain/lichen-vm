//! What an operator **reads** — one authority for the shapes [`LowOperator`]
//! documents, and the ground the control-flow graph will stand on.
//!
//! # Why this is here rather than in a consumer
//!
//! Each backend used to read its own operands out of each operator's operand
//! array: `lichen-compute` had `operand_pair`, `apply_pair` and `operand_items`
//! decoding three shapes between them, and `lichen-compute-gpu` had its own.
//! **Two consumers, one fact, and no way for them to disagree loudly** — they
//! could only disagree by being wrong in one file.
//!
//! # The graph is not SSA over nodes, and that matters more than it looks
//!
//! The obvious next step here is "order the body's nodes and hand each backend a
//! list", and it does not work. **A `NodeId` is not a value.** Two node ids can
//! be one value — the apply clone walk *unifies* a substituted parameter with
//! its argument — and a cell with no value can resolve through its equality class
//! to a node that computes it, which is **not one of its operands**. So a block
//! ordered over `NodeId`s cannot say which node defines a value, and every
//! consumer would re-derive the aliasing. `lichen-compute`'s `emit_node` does
//! exactly that today, in its `equality_rep` and `class_computation_node` arms,
//! and the SPIR-V emitter would have to do it again.
//!
//! **The control-flow graph is therefore SSA over resolved *values*, and the
//! resolution rule belongs here** — beside the cells and equality classes it is
//! about, rather than duplicated in every consumer that walks a body. That is
//! the next piece of this module; what is here now is the operand half of it.
//!
//! # Why the instruction list this replaces was the wrong shape
//!
//! `KernelInstr` was a **stack machine**: instructions named no values, so both
//! consumers walked an operand stack and each *derived* the form it wanted —
//! `waffle` derives a stack from SSA, `spirv.rs` derives SSA ids from a stack.
//! Both paid to undo what the IR omitted, and the omission was not cosmetic: a
//! stack machine has nowhere to put a value that outlives an expression, so a
//! **loop-carried value had no representation at all**
//! (`docs/notes/loop-conversion.md` §8.5 item 1c).

use lichen_utils::extend::AsEnum;

use crate::{AnyNodeId, LowOperator, Module, NodeId, Program};

impl<P: Program> Module<P> {
    /// Every node whose value `node` reads, in operand order.
    ///
    /// **One authority for "what does this operator read"**, beside the
    /// [`LowOperator`] enum that documents each shape. The three structural
    /// operators name their operands; a program's own operator does not, so this
    /// returns the **operand array** itself — the array is a value like any
    /// other, is ordered before its operator, and its own items are reached
    /// through the array's own definition.
    ///
    /// A node with no operation reads nothing: it is a value (a parameter, a
    /// literal, a resolved cell).
    ///
    /// This is the **node-level** answer. The control-flow graph needs the
    /// value-level one — see the module doc — because two node ids can be one
    /// value, and a consumer that resolved that itself would have to know the
    /// aliasing rules this crate already holds.
    pub fn operands_of(&self, node: NodeId) -> Result<Vec<NodeId>, String> {
        let Some(operation) = self.node_operation(node) else {
            return Ok(Vec::new());
        };
        let Some(operand) = operation.operand else {
            return Ok(Vec::new());
        };
        match operation.operator.as_enum() {
            // The checker's apply operands are `[function, argument, result_cell]`
            // and the result cell is not codegen, so this takes the first two and
            // tolerates the rest.
            Some(LowOperator::Apply) => self.operand_pair(operand, "Apply"),
            Some(LowOperator::Index) | Some(LowOperator::TableGet) => {
                self.operand_pair(operand, "operator")
            }
            // A program's own operator: its operand array, whole.
            _ => Ok(vec![operand]),
        }
    }

    /// The `[left, right]` of a two-element operand array, all of this module's.
    fn operand_pair(&self, operand: NodeId, what: &str) -> Result<Vec<NodeId>, String> {
        // SAFETY: `operand` is a live node of `self`; nothing here releases it.
        let items = unsafe { self.array_items(operand) }
            .ok_or_else(|| format!("{what} operand is not an array value"))?;
        if items.len() < 2 {
            return Err(format!(
                "{what} operand array must have two elements, and has {}",
                items.len()
            ));
        }
        items[..2]
            .iter()
            .map(|item| match item.node {
                AnyNodeId::Dynamic(node) => Ok(node),
                // A static element is a reference into a frozen module, which has
                // no node here; a body that reads one cannot be lowered.
                AnyNodeId::Static(_) => Err(format!(
                    "{what} operand element is a static reference into a frozen module, which has \
                     no node in this graph"
                )),
            })
            .collect()
    }

    /// The scalar leaves of a value: itself, or an array's items, one level deep.
    ///
    /// **One level, because a kernel's domain is at most a tuple of scalars.**
    /// A deeper structure would flatten to more than the ABI passes, and
    /// `KernelShape::flat_arity` is the authority on how many leaves that is.
    pub fn value_leaves(&self, value: NodeId) -> Result<Vec<NodeId>, String> {
        // SAFETY: `value` is a live node of `self`.
        match unsafe { self.array_items(value) } {
            Some(items) => items
                .iter()
                .map(|item| match item.node {
                    AnyNodeId::Dynamic(node) => Ok(node),
                    AnyNodeId::Static(_) => Err(
                        "a body reads a static reference into a frozen module, which has no node in \
                         this graph"
                            .into(),
                    ),
                })
                .collect(),
            None => Ok(vec![value]),
        }
    }
}

//! The compound **value** reader: the one place a tuple's, an array's and a
//! struct's element is read.

use lichen_lowlevel::{AnyNodeId, LowOperator, LowValue, NodeId};

use lichen_utils::extend::AsEnum;

use crate::ir::ExprId;
use crate::program::{HighProgram, TypeOperator, ValueType};
use crate::shape;

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// The element a compound value holds at `index`, and the value and type
    /// that element states.
    ///
    /// # Invariant
    /// An element is a **complete expression**: the read's value is the
    /// element's own value slot and its type the element's own type slot, so
    /// an element's type and attributes are read from the element.
    ///
    /// A **decided** container and a constant index are read directly, so the
    /// read's type is decided where the read is checked; anything else keeps
    /// the lazy read.
    pub(super) fn value_slot_read(
        &mut self,
        container_value: NodeId,
        index: NodeId,
        constant: Option<usize>,
    ) -> (NodeId, NodeId, NodeId) {
        if let Some(at) = constant
            && let Some(element) =
                shape::value_item(&self.module, AnyNodeId::Dynamic(container_value), at)
            && let Some(element) = element.dynamic()
        {
            let value_node = shape::value_item(
                &self.module,
                AnyNodeId::Dynamic(element),
                shape::EXPR_VALUE_SLOT,
            )
            .and_then(|node| node.dynamic());
            let ty_node = shape::value_item(
                &self.module,
                AnyNodeId::Dynamic(element),
                shape::EXPR_TYPE_SLOT,
            )
            .and_then(|node| node.dynamic());
            if let (Some(value_node), Some(ty_node)) = (value_node, ty_node) {
                return (element, value_node, ty_node);
            }
        }
        // The lazy read: the element is the container's own item, and the
        // read's value and type are the element's own slots.
        let element = self.index_of(container_value, index);
        let value_node = self.index_of(element, self.zero());
        let ty_node = self.index_of(element, self.one());
        (element, value_node, ty_node)
    }

    /// The lazy read `Index(base, at)`.
    pub(super) fn index_of(&mut self, base: NodeId, at: NodeId) -> NodeId {
        let operands = self.array_node(self.current_block, &[base, at]);
        let node = self.op_node(
            self.current_block,
            P::Operator::from(LowOperator::Index),
            Some(operands),
        );
        self.stamp_read_node(node);
        node
    }

    /// The constant position a subscript states; `None` otherwise, where the
    /// read keeps its lazy form.
    pub(super) fn constant_position(&self, key: ExprId) -> Option<usize> {
        let value = self.state[key].val?;
        match self
            .module
            .node_value(AnyNodeId::Dynamic(value))
            .and_then(|held| held.as_enum())
        {
            Some(LowValue::USize(n)) => Some(n),
            _ => None,
        }
    }
}

//! The `@gc` scope marker: an expression checked into a block of its own.

use lichen_lowlevel::{LowOperator, NodeId};

use crate::ir::ExprId;
use crate::program::{HighProgram, TypeOperator, ValueType};

use super::Checker;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// `@gc inner` — check `inner` into a **child** block, released when `inner` decides.
    ///
    /// # Invariant
    ///
    /// The parent's handles for `inner`'s value and type are **parent reads** of the
    /// scope's expr (`Index(expr, 0)` and `Index(expr, 1)`), so the parent names the
    /// declared root only; reading that handle is what collects the scope. See
    /// `docs/notes/gc-block.md`, "The block boundary".
    pub(super) fn check_gc(&mut self, e: ExprId, inner: ExprId) -> NodeId {
        let enclosing = self.current_block;
        self.current_block = self.module.add_block(Some(enclosing));
        let root = self.check_term(inner);
        self.current_block = enclosing;
        self.module.mark_scope_root(root);
        self.gc_scopes.push(e);
        self.state[e].term = Some(root);
        self.state[e].val = Some(self.expr_slot(enclosing, root, self.zero()));
        self.state[e].ty = Some(self.expr_slot(enclosing, root, self.one()));
        root
    }

    /// A parent-block `Index(expr, slot)` read of the scope's expr.
    fn expr_slot(&mut self, block: lichen_lowlevel::BlockId, expr: NodeId, slot: NodeId) -> NodeId {
        let operands = self.array_node(block, &[expr, slot]);
        self.op_node(block, P::Operator::from(LowOperator::Index), Some(operands))
    }
}

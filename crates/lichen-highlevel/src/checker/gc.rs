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
    /// `@gc inner` — check `inner` into a **child** block, released when
    /// `inner` is deeply decided.
    ///
    /// # Invariant
    ///
    /// The marker allocates no cell of its own: `inner`'s node is the scope's
    /// root and every slot is the inner expression's, so the block boundary is
    /// the only thing `@gc` adds. That root is what a read from the parent
    /// block collects (`docs/notes/gc-block.md`).
    pub(super) fn check_gc(&mut self, e: ExprId, inner: ExprId) -> NodeId {
        let enclosing = self.current_block;
        self.current_block = self.module.add_block(Some(enclosing));
        let root = self.check_term(inner);
        self.current_block = enclosing;
        self.module.mark_scope_root(root);
        self.gc_scopes.push(e);
        self.state[e].term = Some(root);
        self.state[e].val = self.state[inner].val;
        self.state[e].ty = self.state[inner].ty;
        root
    }
}

//! The `$name(args…)` native-operator call rule.

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A `$name(args…)` call: look the name up in this module's private
    /// [`NativeOps`] registry and adopt its value node.
    ///
    /// # Invariant
    /// The checker knows nothing of what the operator does: the plugin's
    /// registration owns the lowering, and the wrapper around `$name` owns the
    /// types. The call's type is the checker's, a fresh cell per call. An
    /// unregistered name is a diagnostic, never a panic: the frontend compiles
    /// `$name` blind, so the checker is the first layer that sees the registry.
    pub(super) fn check_native_call(
        &mut self,
        e: ExprId,
        op: &'static str,
        args: ChildRange,
    ) -> NodeId {
        let arg_ids: Vec<ExprId> =
            self.ir.children[args.start as usize..args.end as usize].to_vec();
        for &arg in &arg_ids {
            self.check_expr(arg);
        }
        let native_args: Vec<NativeArg> = arg_ids
            .iter()
            .map(|&arg| NativeArg {
                expr: arg,
                value: self.value_of(arg),
            })
            .collect();
        let loc = self.loc(e, 0);
        let ops = self.native_ops;
        let built = match ops
            .iter()
            .find(|(name, _)| *name == op)
            .map(|(_, operator)| operator)
        {
            Some(operator) => operator.build(self, e, &native_args, loc.clone()),
            None => {
                // An unregistered name is a check-time refusal, not a broken
                // invariant: only the checker sees the registry.

                // The guard leaves the expression uncompiled, so the definition
                // pass is skipped and nothing evaluates the hole.
                self.record_guard(
                    self.type_expr,
                    self.type_expr,
                    loc,
                    DiagKind::NativeOpUnresolved,
                    Some(op),
                );
                let pair = self.pair_of(self.type_expr, self.type_expr);
                self.state[e].term = Some(pair);
                self.state[e].val = None;
                self.state[e].ty = Some(self.type_expr);
                return pair;
            }
        };
        // The type is not the plugin's to give: the boundary mints a fresh
        // cell per call, through [`Checker::pair_of`].
        let ty = self.fresh_cell();
        let pair = self.pair_of(built.value, ty);
        self.state[e].term = Some(pair);
        self.state[e].val = built.decided.then_some(built.value);
        self.state[e].ty = Some(ty);
        pair
    }
}

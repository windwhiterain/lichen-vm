//! The `$name(args…)` native-operator call rule.

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A `$name(args…)` call: compile each argument, look `name` up in this
    /// module's private [`NativeOps`] registry, and adopt the value node the
    /// plugin's [`NativeOp`] builder returns, typed by a fresh cell.  The
    /// checker has no knowledge of what the operator does — the plugin's
    /// registration owns the lowering, and the types live in the lichen
    /// wrapper that calls `$name` (the private contract with its own source).
    /// An unregistered `name` is refused with a diagnostic rather
    /// than a panic (the frontend compiles `$name` blind, so the checker is the
    /// first layer that can see the registry).
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
                // An unregistered name is an ordinary check-time refusal, not
                // a broken invariant: only the checker can see the registry,
                // so this is the one place it can be reported.  The guard
                // leaves the expression uncompiled, which `check_failed` picks
                // up — the definition pass is skipped, so nothing ever
                // evaluates the hole.
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
        // The builder states the operator's **value**; the checker states the
        // call's **type**.  A native operator is an operator — raw operands in,
        // one raw result node out — so the type is not the plugin's to give:
        // the boundary mints a fresh cell for every call, unconditionally, and
        // the `[value, type]` pair is built through its one construction site
        // ([`Checker::pair_of`]).  Whatever the wrapper around `$name` knows
        // about the result's type, it states as an ordinary annotation.
        let ty = self.fresh_cell();
        let pair = self.pair_of(built.value, ty);
        self.state[e].term = Some(pair);
        self.state[e].val = built.decided.then_some(built.value);
        self.state[e].ty = Some(ty);
        pair
    }
}

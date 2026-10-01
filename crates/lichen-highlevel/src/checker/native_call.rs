//! The `$name(args…)` native-operator call rule.

use super::*;

impl<P: HighProgram> Checker<P>
where
    P::Value: ValueType,
    P::Operator: From<LowOperator> + From<TypeOperator>,
{
    /// A `$name(args…)` call: compile each argument, look `name` up in this
    /// module's private [`NativeOps`] registry, and adopt the `[value, type]`
    /// pair the plugin's [`NativeOp`] builder returns.  The checker has no
    /// knowledge of what the operator does — the plugin's registration owns the
    /// lowering and the type construction (the private contract with its own
    /// source).  An unregistered `name` is refused with a diagnostic rather
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
                ty: self.state[arg].ty.expect("a compiled argument has a type"),
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
        // The plugin's contract — the one thing the checker relies on and
        // cannot see.  Every downstream read of an expression's term reads it
        // as a `[value, type]` pair (`value_of` indexes element 0; the apply
        // wiring and the type checks read element 1), and `Ctx` builds exactly
        // that shape — but the three records a builder returns are raw node
        // ids, so the shape is validated here rather than assumed: `node` must
        // be a two-slot array in the block this call is compiled into, element
        // 1 exactly `ty`, and element 0 exactly `val` when the value is a
        // decided node (a builder like compute's `$jit` legitimately returns
        // `val: None`, leaving element 0 the op node the runtime reads).  A
        // violated contract is a guard rather than an adopted malformed term,
        // exactly as the imported export's pair contract above.
        //
        // Only `node`'s block is checked: the checker's canonical shared type
        // expressions (`int_type`, the kind markers, the universe) are
        // allocated once in the root block and deliberately referenced from
        // pairs built in child blocks — compute's `$range` returns
        // `int_type()` from a kernel body — so `ty`'s block is not part of the
        // contract.
        let pair_items = match self.module.node_value(AnyNodeId::Dynamic(built.node)) {
            Some(value) => match value.as_enum() {
                // SAFETY: `array` is the value payload of `built.node`, a node
                // the plugin holds from this module, so its home block is alive
                // for this read.
                Some(LowValue::Array(array)) => Some(unsafe { array.items() }),
                _ => None,
            },
            None => None,
        };
        let contract_holds = pair_items.is_some_and(|items| {
            items.len() == 2
                && self.module.nodes.contains_key(built.node)
                && self.module.node_block(built.node) == self.current_block
                && items[1].node == AnyNodeId::Dynamic(built.ty)
                && built
                    .val
                    .is_none_or(|val| items[0].node == AnyNodeId::Dynamic(val))
        });
        if !contract_holds {
            let cell = self.fresh_cell();
            let pair = self.pair_of(cell, cell);
            self.state[e].term = Some(pair);
            self.state[e].val = Some(cell);
            self.state[e].ty = Some(cell);
            self.record_guard(pair, pair, loc, DiagKind::NativeOpContract, Some(op));
            return pair;
        }
        self.state[e].term = Some(built.node);
        self.state[e].val = built.val;
        self.state[e].ty = Some(built.ty);
        built.node
    }
}

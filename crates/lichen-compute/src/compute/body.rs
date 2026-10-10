//! A kernel body, lowered: a graph walk that emits SSA.
//!
//! Background: `docs/notes/lichen-compute.md` §4.
//!
//! # Invariant
//! A node emits once and is named afterwards through [`Lower::values`]; the
//! buffer ordinals a backend reads are [`Positions`], filled by this walk so
//! that the fragment's input and output classes cannot disagree.

use std::collections::HashMap;

use lichen_highlevel::program::ValueType;
use lichen_kernel_ir::{
    Br, KernelBin, KernelBody, KernelInstr, KernelShape, ScalarClass, Terminator, ValueId,
};
use lichen_lowlevel::{
    AnyFunctionId, AnyNodeId, Define, FunctionId, LoopArm, LoopConversion, LowOperator, LowValue,
    Module, NodeId, Program,
};
use lichen_utils::extend::AsEnum;

use super::{
    ComputeOperator, ComputeValue, ParamSlot, Positions, TypeOperator, const_bits, conv_of,
    flat_arity, float_bits, is_static_function, kernel_bin, kernel_id_of, kernels, node_class,
    node_class_in, parallel_buffer_pos, param_classes, peeled_argument, scalar_literal,
};

/// The parameter slots' classes in flattening order — the ABI's own list, not the body's.
fn param_classes_of(params: &[ParamSlot]) -> Vec<ScalarClass> {
    params
        .iter()
        .flat_map(|slot| super::scalar_classes_of(&slot.shape))
        .collect()
}

/// How many parameter leaves the body's entry block receives — the declared
/// shape's arity, not the domain node's.
///
/// # Invariant
/// The count is the slots' [`flat_arity`] summed: a kernel's parameter is a
/// struct carrying a native wrapper and a signature, so the node a caller hands
/// over is the wrapper's cell rather than the tuple the domain declares.
fn domain_arity(params: &[ParamSlot]) -> usize {
    params
        .iter()
        .map(|slot| super::flat_arity(&slot.shape))
        .sum()
}

/// One function's lowering: the body being built, and everything the walk needs
/// to place a value in it.
pub struct Lower<'a, P: Program> {
    module: &'a Module<P>,
    /// The parameter slots, for the class and path reads the buffer arms need.
    params: &'a [ParamSlot],
    /// The domain's **value** node — its leaves are this body's parameters, stated by the caller.
    domain: NodeId,
    /// The function the domain belongs to, so `Module::define_in` derives the domain once.
    function: FunctionId,
    tally: &'a mut Positions,
    body: KernelBody,
    /// A node to the value it produced — **emission happens once**, so a shared
    /// subexpression is computed once.
    values: HashMap<NodeId, ValueId>,
    /// How many definitions deep the walk is — the guard that replaced a
    /// hand-rolled operand stack's failure mode.
    depth: usize,
    /// The class each emitted value was declared in, keyed by the node it came from.
    classes: HashMap<NodeId, ScalarClass>,
    /// The entry block's parameters' classes in flattening order; [`param_classes_of`]
    /// states whose list that is.
    leaf_classes: Vec<ScalarClass>,
    /// The node whose definition is being emitted, so [`Lower::emit`] can record
    /// the class it declared against it.
    defining: NodeId,
    /// The block instructions are emitted into.
    ///
    /// # Invariant
    /// It moves to a loop's merge block when a nest is built, so the caller's
    /// remaining instructions land in the block the nest leaves.
    block: usize,
    /// The active `@loop` nests, innermost last.
    loops: Vec<ActiveLoop>,
}

/// One `@loop` nest being emitted: the conversion's roles and the state each
/// block of the nest receives.
///
/// # Invariant
/// A state read of the marked function resolves to one of `block_slots`, which
/// are the parameters of the block being emitted — the same binding the host
/// loop reads through `Instantiation::node_of` (`loop_run.rs`).
struct ActiveLoop {
    function: FunctionId,
    conversion: LoopConversion,
    /// The parameters of [`Self::block`], in the conversion's slot order.
    block_slots: Vec<ValueId>,
    /// The block the nest starts from: it dominates every block of the nest.
    entry: usize,
    /// The block the nest is currently emitting into.
    block: usize,
    /// The state's home, and one block per test, step and exit level.
    header: usize,
    tests: Vec<usize>,
    steps: Vec<usize>,
    exits: Vec<usize>,
    /// Where the nest leaves its result.
    merge: usize,
}

/// A `write` in a conditional's arm is refused (see §6 of the loop-conversion note).
const CONDITIONAL_WRITE: &str = "a `compute.write` inside a conditional's arm is refused: a kernel body's conditional is a `Select`, which emits both arms on every lane, so the arm's write would run on every lane and overwrite the selected arm's own. A real branch is what makes it legal (`docs/notes/loop-conversion.md` §6)";

/// Refusal: a cross-kernel callee returns exactly one value, in the caller's class.
///
/// # Invariant
/// A cross-kernel call is an ordinary wasm `call`, so a float caller passing an
/// `f32` to an integer callee — or reading an `i64` back into a float body —
/// would be a module that does not validate.
const CROSS_KERNEL_RESULT_ARITY: &str = "a cross-kernel call to a kernel that returns more than one value is not supported: a kernel body reads a callee result as a single value, and re-materialising a tuple result needs local slots the kernel instruction set does not yet have";

/// The walk's depth ceiling: a refusal, not a panic, and what a deeply **expanded**
/// recursion runs into.
const MAX_KERNEL_BODY_DEPTH: usize = 512;

impl<'a, P: Program> Lower<'a, P>
where
    P::Value: From<ComputeValue> + AsEnum<ComputeValue> + ValueType,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    /// Lower `codomain` — the value the function returns — into a body.
    ///
    /// # Invariant
    /// The entry block's parameters are the domain's leaves **in the ABI's
    /// order**, which is what makes an old `LocalGet(k)` and a loop's carried
    /// value the same read.
    pub fn lower(
        module: &'a Module<P>,
        params: &'a [ParamSlot],
        domain: NodeId,
        function: FunctionId,
        codomain: NodeId,
        tally: &'a mut Positions,
    ) -> Result<KernelBody, String> {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        // The count is the shape's, not the domain node's: see [`domain_arity`].
        for _ in 0..domain_arity(params) {
            body.add_param(entry);
        }
        let mut lower = Lower {
            module,
            params,
            domain,
            function,
            tally,
            body,
            values: HashMap::new(),
            depth: 0,
            classes: HashMap::new(),
            leaf_classes: param_classes_of(params),
            defining: NodeId::default(),
            block: entry,
            loops: Vec::new(),
        };
        let mut values = Vec::new();
        for node in lower.leaves_of(codomain)? {
            values.push(lower.value(node)?);
        }
        // **The return belongs where the walk ended** — the entry block, or a nest's merge block.
        lower
            .body
            .set_terminator(lower.block, Terminator::Return { values });
        Ok(lower.body)
    }

    /// A parallel index function, lowered: each output is a `compute.write`.
    ///
    /// # Invariant
    /// The outputs are emitted in position order, so write `k` takes the ordinal
    /// `k`. The writes produce nothing, so the terminator returns a dummy of the
    /// fragment's class to match the result the signature declares.
    pub fn lower_index_function(
        module: &'a Module<P>,
        params: &'a [ParamSlot],
        domain: NodeId,
        function: FunctionId,
        outputs: &[NodeId],
        class: ScalarClass,
        tally: &'a mut Positions,
    ) -> Result<KernelBody, String> {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        for _ in 0..domain_arity(params) {
            body.add_param(entry);
        }
        // **The fragment's domain is the config's leaves plus the invocation index**,
        // which no shape contains.
        body.add_param(entry);
        let mut lower = Lower {
            module,
            params,
            domain,
            function,
            tally,
            body,
            values: HashMap::new(),
            depth: 0,
            classes: HashMap::new(),
            leaf_classes: param_classes_of(params),
            defining: NodeId::default(),
            block: entry,
            loops: Vec::new(),
        };
        for output in outputs {
            // The value a write leaves is discarded on purpose: see the doc.
            lower.value(*output)?;
        }
        let dummy = lower.emit_const(class, const_bits(class, 0));
        lower.body.set_terminator(
            lower.block,
            Terminator::Return {
                values: vec![dummy],
            },
        );
        Ok(lower.body)
    }

    // ---------------------------------------------------------------- values

    /// The value `node` names, emitting its definition the first time it is asked
    /// for.
    fn value(&mut self, node: NodeId) -> Result<ValueId, String> {
        // **Inside a `@loop` nest the roles are the binding**: a marked function's read
        // resolves to its block's parameters.
        if let Some(index) = self.loop_owner(node)
            && let Some(value) = self.loop_value(index, node)?
        {
            return Ok(value);
        }
        if let Some(&value) = self.values.get(&node) {
            return Ok(value);
        }
        if self.depth > MAX_KERNEL_BODY_DEPTH {
            return Err(super::kernel_body_too_deep());
        }
        // A nest's role is emitted in its current block; one it does not own, at the
        // nest's entry.
        let enclosing = self.block;
        let target = match self.loop_owner(node) {
            Some(index) => Some(self.loops[index].block),
            None => self.current_loop().map(|index| self.loops[index].entry),
        };
        if let Some(target) = target {
            self.block = target;
        }
        self.depth += 1;
        // **Every emission records its class against this node**, so `defining` is set
        // here and not in `definition`.
        self.defining = node;
        // **A read of the domain at a path is matched structurally**, from the `Index`
        // chain: see [`Lower::parameter_path`].
        let value = match self.parameter_path(node)? {
            Some(slot) => {
                // **A parameter's class is the ABI's**, recorded so an operator learns it
                // without the fragment.
                let class = self
                    .leaf_classes
                    .get(slot)
                    .copied()
                    .unwrap_or(ScalarClass::Int);
                self.classes.insert(node, class);
                self.parameter(slot)?
            }
            // **A literal is answered first**: a decided literal may have no operation
            // that computes it.
            None => match self.literal(node) {
                Some(value) => value,
                None => match self.module.define_in(self.function, node) {
                    Define::Parameter(leaf) => self.parameter(self.slot_of(leaf)?)?,
                    Define::Computed(definition) => self.definition(definition)?,
                    Define::Opaque => self.opaque(node)?,
                },
            },
        };
        self.depth -= 1;
        // Only the frame that moved the cursor puts it back: a nest's move to its
        // merge block stands.
        if target.is_some() {
            self.block = enclosing;
        }
        self.values.insert(node, value);
        Ok(value)
    }

    // ------------------------------------------------------------------ loops

    /// The innermost active nest, copied out so the borrow ends before the
    /// emission it governs.
    fn current_loop(&self) -> Option<usize> {
        self.loops.len().checked_sub(1)
    }

    /// The innermost active nest whose marked function owns `node`.
    fn loop_owner(&self, node: NodeId) -> Option<usize> {
        for index in (0..self.loops.len()).rev() {
            let function = self.loops[index].function;
            if self.module.belongs_to(node, function).unwrap_or(false) {
                return Some(index);
            }
        }
        None
    }

    /// The value a nest's state read names, or `None` for a node that is not a
    /// state read of that nest.
    ///
    /// # Invariant
    /// A read is matched by the conversion's own path ([`LoopConversion::state`])
    /// against [`Module::parameter_value_path`]; a read covering no slot is a
    /// disagreement between the readers, not a shape to guess at.
    fn loop_value(&self, index: usize, node: NodeId) -> Result<Option<ValueId>, String> {
        let active = &self.loops[index];
        let Some(path) = self.module.parameter_value_path(active.function, node) else {
            return Ok(None);
        };
        let Some(slot) = active
            .conversion
            .state
            .iter()
            .position(|candidate| *candidate == path)
        else {
            return Err(format!(
                "a `@loop` nest read its carried state at path {path:?}, which the conversion's \
                 slots do not name"
            ));
        };
        let value = active.block_slots.get(slot).copied().ok_or_else(|| {
            format!(
                "a `@loop` nest read state slot {slot}, and the entering call bound {} slot(s)",
                active.block_slots.len()
            )
        })?;
        Ok(Some(value))
    }

    /// The parameter read `node` names, through the wrappers it may arrive in.
    ///
    /// # Invariant
    /// A call's argument arrives wrapped where a body's read does not, so the same
    /// path walk is tried on the node, on what one peel takes off it, and on its
    /// view, and the first that lands on a slot wins.
    fn parameter_path_of(&self, node: NodeId) -> Result<Option<usize>, String> {
        let peeled = self.module.pair_value_half(node);
        let viewed = match self.module.selection_of(node) {
            Some(lichen_lowlevel::Selection::Views(view)) => Some(view),
            _ => None,
        };
        if let Some(slot) = self.parameter_path(node)? {
            return Ok(Some(slot));
        }
        if let Some(peeled) = peeled
            && let Some(slot) = self.parameter_path(peeled)?
        {
            return Ok(Some(slot));
        }
        Ok(viewed
            .map(|viewed| self.parameter_path(viewed))
            .transpose()?
            .flatten())
    }

    /// The entry parameter a read of the domain at a path names, if that is what
    /// `node` is.
    ///
    /// # Invariant
    /// The chain of `Index` nodes is the test, not the base's class: a parameter the
    /// deep pass unified with its argument is a computation by codegen time. The base
    /// is matched against the slots themselves — each slot's pair and its value — or
    /// against one of the domain's leaves, because a struct parameter, a tuple domain
    /// and a scalar each reach the walk through a different node.
    fn parameter_path(&self, node: NodeId) -> Result<Option<usize>, String> {
        let mut positions: Vec<usize> = Vec::new();
        let mut cursor = node;
        loop {
            // **A base with no operation ends the chain**, rather than failing to answer.
            let Some(operation) = self.module.node_operation(cursor) else {
                break;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                break;
            }
            let Some(arguments) = self.arguments(cursor).ok() else {
                return Ok(None);
            };
            let (Some(target), Some(index)) = (arguments.first(), arguments.get(1)) else {
                return Ok(None);
            };
            let Some(step) = self.module.usize_value(*index) else {
                // **A named step is answered by the parameter's own type**, which is
                // `param_read_offset`'s walk, not this one's.
                return Ok(super::param_read_offset(self.module, self.params, node)?
                    .map(|offset| offset as usize));
            };
            let AnyNodeId::Dynamic(target) = target else {
                return Ok(None);
            };
            // **`Index(param_pair, k)` is the pair's value half, not a path step**, and the
            // cursor stops on the pair.
            if self
                .params
                .iter()
                .any(|slot| self.module.class_root(slot.pair) == self.module.class_root(*target))
            {
                cursor = *target;
                break;
            }
            positions.push(step);
            cursor = *target;
        }
        let root = self.module.class_root(cursor);
        let on_a_slot = self.params.iter().any(|slot| {
            root == self.module.class_root(slot.pair) || root == self.module.class_root(slot.value)
        });
        // **Or one of the domain's leaves**: a read of `p` reaches the wrapper's field
        // first, so the chain ends at a leaf.
        let on_a_leaf = self
            .domain_leaves()
            .map(|leaves| {
                leaves
                    .iter()
                    .any(|leaf| self.module.class_root(*leaf) == root)
            })
            .unwrap_or(false);
        if !(on_a_slot || on_a_leaf) {
            return Ok(None);
        }
        // **The slot is the path flattened**, summing each skipped element's arity;
        // the steps are reversed first.
        positions.reverse();
        let mut offset = 0usize;
        let mut shape = match self.params.first() {
            Some(slot) => slot.shape.clone(),
            None => return Ok(None),
        };
        for position in &positions {
            let lichen_lowlevel::LowShape::Tuple(items) = &shape else {
                return Ok(None);
            };
            let Some(element) = items.get(*position) else {
                return Ok(None);
            };
            for skipped in &items[..*position] {
                offset += super::flat_arity(skipped);
            }
            shape = element.clone();
        }
        Ok(Some(offset))
    }

    /// The domain's leaves, flattened — the ABI's argument list.
    fn domain_leaves(&self) -> Result<Vec<NodeId>, String> {
        self.module.value_leaves(self.domain)
    }

    /// A value's leaves: itself, or an array's items. A codomain is at most a tuple of scalars.
    fn leaves_of(&self, value: NodeId) -> Result<Vec<NodeId>, String> {
        self.module.value_leaves(value)
    }

    /// The ABI slot the domain's `leaf`-th leaf takes.
    ///
    /// # Invariant
    /// A struct domain's whole value is a parameter too, and its fields are the ones
    /// that take slots, so a body reading the parameter and a body reading one of
    /// its fields are both parameter reads.
    fn slot_of(&self, leaf: NodeId) -> Result<usize, String> {
        let root = self.module.class_root(leaf);
        let domain = self.domain;
        let leaves = self.domain_leaves()?;
        if self.module.class_root(domain) == root {
            return Ok(0);
        }
        leaves
            .iter()
            .position(|candidate| self.module.class_root(*candidate) == root)
            .ok_or_else(|| {
                format!("node {leaf:?} reads a value that is not one of this body's parameters")
            })
    }

    /// The body's `k`-th parameter — what `KernelInstr::LocalGet(k)` was, now a
    /// named read.
    fn parameter(&self, k: usize) -> Result<ValueId, String> {
        self.body.parameters().get(k).copied().ok_or_else(|| {
            format!(
                "a body reads parameter {k}, which a domain of {} leaf/leaves does not have",
                self.body.parameters().len()
            )
        })
    }

    /// Emit `instr` over `args` in the current block, declaring what it leaves.
    fn emit(&mut self, instr: KernelInstr, args: Vec<ValueId>, class: ScalarClass) -> ValueId {
        let declared = if instr.produces() == 0 {
            Vec::new()
        } else {
            vec![class]
        };
        let value = self.body.add_op(self.block, instr, args, declared);
        if instr.produces() > 0 {
            // **Recorded against the node being defined**, which an operator consults.
            self.classes.insert(self.defining, class);
        }
        value
    }

    fn emit_const(&mut self, class: ScalarClass, bits: i64) -> ValueId {
        self.emit(KernelInstr::Const(class, bits), Vec::new(), class)
    }

    /// A literal, folded where the graph has one.
    fn literal(&mut self, node: NodeId) -> Option<ValueId> {
        match scalar_literal(self.module, node) {
            Some(LowValue::USize(n)) => {
                let class = node_class(self.module, node);
                Some(self.emit_const(class, const_bits(class, n as i64)))
            }
            Some(LowValue::Float(f)) => Some(self.emit_const(ScalarClass::Float, float_bits(f))),
            _ => None,
        }
    }

    /// A node nothing can resolve, **described rather than numbered**.
    fn opaque(&self, node: NodeId) -> Result<ValueId, String> {
        Err(format!(
            "a kernel body reached a node with neither a value nor an operation, so there is \
             nothing to emit for it (node={node:?}). A kernel is compiled from a template before \
             any apply, so a binding the body would fill in at run time is still empty here — a \
             `let` alias fed by a buffer read, a helper defined in the body rather than at module \
             level, and a `compute.call`'s wrapper all have this shape. Move the binding to module \
             level, or write what it would have computed directly into the expression the kernel \
             uses"
        ))
    }

    // ------------------------------------------------------------ definitions

    /// Emit `node`'s definition and return the value it leaves.
    fn definition(&mut self, node: NodeId) -> Result<ValueId, String> {
        let class = node_class(self.module, node);
        // A literal is the cheapest thing a node can be, and it is not a computation.
        if let Some(value) = self.literal(node) {
            return Ok(value);
        }
        let Some(operation) = self.module.node_operation(node) else {
            return self.opaque(node);
        };
        let Some(operand) = operation.operand else {
            return Err(format!("{operation:?} has no operand array"));
        };
        let op = &operation.operator;

        // The structural core, dispatched through the lowlevel.
        if let Some(low) = AsEnum::<LowOperator>::as_enum(op) {
            return match low {
                LowOperator::Index => self.index(node),
                LowOperator::Apply => self.apply(node, operand),
                LowOperator::TableGet => Err(
                    "unsupported tableget operator in kernel body (kernel-safe subset is scalar \
                     arith)"
                        .into(),
                ),
            };
        }

        // The two class crossings, `int2float` and `float2int`.
        if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(op)
            && let Some((from, to, name)) = conv_of(ty_op)
        {
            return self.conversion(operand, from, to, name);
        }

        // The language's arithmetic and comparison operators.
        if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(op)
            && let Some(bin) = kernel_bin(ty_op)
        {
            let arguments = self.arguments(node)?;
            let (Some(lhs), Some(rhs)) = (arguments.first().copied(), arguments.get(1).copied())
            else {
                return Err(format!("{ty_op:?} is missing an operand"));
            };
            return self.arithmetic(ty_op, bin, lhs, rhs, class);
        }

        // The compute plugin's own operators.
        if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
            return self.compute_operator(node, compute_op, operand);
        }
        Err(format!(
            "unsupported operation in kernel body: {op:?} (kernel-safe subset is scalar arith)"
        ))
    }

    /// One compute operator over its own operand array, wherever the walk meets it.
    ///
    /// # Invariant
    /// Shared by an operator node and by a routed apply whose frozen callee's body
    /// names one, so the two routes cannot drift.
    fn compute_operator(
        &mut self,
        node: NodeId,
        compute_op: ComputeOperator,
        operand: NodeId,
    ) -> Result<ValueId, String> {
        match compute_op {
            ComputeOperator::Launch | ComputeOperator::Call => {
                // **A program's operator reads its operand array's elements**, not the
                // array `operands_of` names.
                let operands = self.arguments(node)?;
                let Some(kernel) = operands.first() else {
                    return Err(format!(
                        "cross-kernel call's operand array is empty, and a call reads [callee, \
                         argument]"
                    ));
                };
                let Some(arg) = operands.get(1) else {
                    return Err(format!(
                        "cross-kernel call's operand array holds {} element(s), and a call \
                         reads [callee, argument]",
                        operands.len()
                    ));
                };
                self.cross_kernel_call(*kernel, *arg)
            }
            // The loop index of the current parallel invocation, after the cfg params.
            ComputeOperator::Range => {
                let index: usize = self.params.iter().map(|p| flat_arity(&p.shape)).sum();
                self.parameter(index)
            }
            ComputeOperator::Read => self.buffer_read(operand),
            ComputeOperator::Write => self.buffer_write(operand),
            other => Err(format!(
                "unsupported compute operator in kernel body: {other:?}"
            )),
        }
    }

    /// One arithmetic or comparison operator over two operands, at `fallback_class`
    /// when their classes say nothing.
    ///
    /// # Invariant
    /// **The operator's class is its operands' class**, not the node's: a float
    /// body's index and count are `Int` positions, so an operator adding two floats
    /// computes in `Float` whatever node the checker hung them on.
    fn arithmetic(
        &mut self,
        ty_op: TypeOperator,
        bin: KernelBin,
        left_operand: AnyNodeId,
        right_operand: AnyNodeId,
        fallback_class: ScalarClass,
    ) -> Result<ValueId, String> {
        // **The operands first**: the class is read off what they emitted, so asking
        // earlier reads nothing.
        let left = self.value_item(left_operand)?;
        let right = self.value_item(right_operand)?;
        let class = self
            .emitted_class(left_operand)
            .or_else(|| self.emitted_class(right_operand))
            .unwrap_or(fallback_class);
        // **A float has no `%` or bitwise form**, and the refusal is here at the
        // operand because the class is still visible.
        if class == ScalarClass::Float
            && matches!(
                bin,
                KernelBin::Rem | KernelBin::BitAnd | KernelBin::BitOr | KernelBin::BitXor
            )
        {
            return Err(format!(
                "`{ty_op:?}` has no float form: a kernel's float operators are `+ - * /` and the \
                 four order comparisons, not `%` or the bitwise operators"
            ));
        }
        Ok(self.emit(KernelInstr::Bin(class, bin), vec![left, right], class))
    }

    /// The operator a routed apply names, read from its frozen callee body, or `None`.
    ///
    /// # Invariant
    /// The operator is read from the artifact's structure, never from a value:
    /// the callee is reachable by `FunctionId` whether or not anything evaluated,
    /// which is why this answers where the class channel cannot — the argument is
    /// a read the kernel emits and the host never decides.
    fn routed_operator(
        &mut self,
        node: NodeId,
        callee: NodeId,
        operand: NodeId,
    ) -> Result<Option<ValueId>, String> {
        let function = match self
            .module
            .node_value(AnyNodeId::Dynamic(callee))
            .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
        {
            Some(LowValue::Function(AnyFunctionId::Static(function))) => function,
            // The cell's value, or the projection into the frozen module's own
            // export array.
            _ => match self.module.static_function_of_callee(callee) {
                Some(function) => function,
                None => return Ok(None),
            },
        };
        let Some((operator, _)) = self.module.static_function_compute_operator(function) else {
            return Ok(None);
        };
        // The operator's operands are the call site's argument's value half.
        let Some(argument) = self
            .module
            .operand_items(operand)
            .ok()
            .and_then(|items| items.get(1))
            .map(|item| item.node)
            .and_then(|item| item.dynamic())
        else {
            return Ok(None);
        };
        let value = self.module.pair_value_half(argument).unwrap_or(argument);
        if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(&operator) {
            return self.compute_operator(node, compute_op, value).map(Some);
        }
        if let Some(ty_op) = AsEnum::<TypeOperator>::as_enum(&operator)
            && let Some(bin) = kernel_bin(ty_op)
        {
            let Some(argument) = self
                .module
                .operand_items(operand)
                .ok()
                .and_then(|items| items.get(1))
                .map(|item| item.node)
                .and_then(|item| item.dynamic())
            else {
                return Ok(None);
            };
            let value = self.module.pair_value_half(argument).unwrap_or(argument);
            let elements = self
                .module
                .value_leaves(value)
                .map_err(|reason| format!("{ty_op:?}'s argument is not readable: {reason}"))?;
            let (Some(lhs), Some(rhs)) = (elements.first().copied(), elements.get(1).copied())
            else {
                return Err(format!("{ty_op:?} is missing an operand"));
            };
            return self
                .arithmetic(
                    ty_op,
                    bin,
                    AnyNodeId::Dynamic(lhs),
                    AnyNodeId::Dynamic(rhs),
                    ScalarClass::Int,
                )
                .map(Some);
        }
        Ok(None)
    }

    /// An `Apply`: a `@loop` recursion, a routed operator's call, or a cross-kernel call.
    ///
    /// # Invariant
    /// The routing lowers `x + 1` to a call of the prelude's binding, so the frozen
    /// callee is a body this module cannot walk and the residual the lowlevel's clone
    /// wrote is emitted instead; a callee that is a kernel value is a cross-kernel
    /// call, and a marked recursion whose shape converts is the loop nest. A plain
    /// lichen-function call is refused by name.
    fn apply(&mut self, node: NodeId, operand: NodeId) -> Result<ValueId, String> {
        let Some(operands) = self.module.operand_items(operand).ok() else {
            return Err("Apply operand is missing".into());
        };
        let callee = dynamic(
            operands
                .first()
                .map(|item| item.node)
                .unwrap_or(operands[0].node),
        )?;
        // The routing case: emit the residual the clone wrote.
        let frozen = is_static_function(self.module, callee);
        if frozen {
            let residual = (unsafe { self.module.array_items(node) })
                .and_then(|items| items.first())
                .map(|item| item.node)
                .and_then(|item| match item {
                    AnyNodeId::Dynamic(node) => Some(node),
                    AnyNodeId::Static(_) => None,
                });
            if let Some(residual) = residual {
                return self.value(residual);
            }
        }
// The operator's identity is a static fact of its callee, residual or not,
        // so the frozen body is read by `FunctionId`.
        if let Some(value) = self.routed_operator(node, callee, operand)? {
            return Ok(value);
        }
        if frozen {
            return Err(
                "this kernel body applies a prelude operator where the kernel cannot reach the \
                 body it lowered to, and its callee names no compute operator: the operator is \
                 a call of the prelude's binding, the call sits in the function's own template \
                 (which the compiler never evaluates), and an operator applied *inside a call's \
                 argument* has no residual to read (`docs/notes/loop-conversion.md` §8.5)"
                    .into(),
            );
        }
        // **A marked recursion** converts to a loop; the callee's own template decides
        // the shape.
        if let Some(function) = self.module.callee_function(node)
            && self.module.functions[function].looping
        {
            return match self.module.loop_conversion(function) {
                Ok(conversion) => self.lower_loop(node, function, &conversion),
                Err(refusal) => Err(format!(
                    "a `@loop`-marked recursion this kernel body applies is not convertible, and a \
                     kernel body has no expansion to fall back on: the shape is {} \
                     (`docs/notes/loop-conversion.md` §4)",
                    refusal.name()
                )),
            };
        }
        // A cross-kernel call.
        if kernel_id_of(self.module, callee).is_some() {
            let arg = operands.get(1).map(|item| item.node);
            let Some(arg) = arg else {
                return Err(format!(
                    "an Apply's operand array holds {} element(s), and a cross-kernel call reads \
                     [callee, argument]",
                    operands.len()
                ));
            };
            return self.cross_kernel_call(AnyNodeId::Dynamic(callee), arg);
        }
        // **Style 1, an ordinary lichen-function call, is refused by name**.
        let _ = node;
        Err(
            "kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; \
             inline lichen-function calls are not yet supported"
                .into(),
        )
    }

    // ------------------------------------------------------------ loop nesting

    /// The entering call lowered to a `@loop` nest over the conversion's roles.
    ///
    /// # Invariant
    /// Every block of the nest receives the whole carried state as its `params`,
    /// so a read of the marked function's parameter means one thing in all of
    /// them; the header is the outermost test, because a merge block is joined
    /// from the header and a backend's structured control flow requires the
    /// header to be the block that chooses.
    fn lower_loop(
        &mut self,
        node: NodeId,
        function: FunctionId,
        conversion: &LoopConversion,
    ) -> Result<ValueId, String> {
        let slots = self.loop_state(node, conversion)?;
        let entry = self.block;
        let header = self.body.add_block();
        for _ in 0..slots.len() {
            self.body.add_param(header);
        }
        let tests: Vec<usize> = (0..conversion.tests.len())
            .map(|position| {
                if position == 0 {
                    header
                } else {
                    self.loop_block(slots.len())
                }
            })
            .collect();
        let steps: Vec<usize> = (0..conversion.steps.len())
            .map(|_| self.loop_block(slots.len()))
            .collect();
        let exits: Vec<usize> = (0..conversion.exits.len())
            .map(|_| self.loop_block(slots.len()))
            .collect();
        let merge = self.loop_block(1);
        self.body.set_terminator(
            entry,
            Terminator::Br(Br {
                target: header,
                args: slots.clone(),
            }),
        );
        // A nested outermost decision gets its own block; the header's own
        // conditional branch is set with the other tests.
        let Some(&first) = tests.first() else {
            return Err(
                "a `@loop` conversion names no test, so its nest has no decision to make — the \
                 conversion refuses a spine with no base test, and the two readers of one \
                 conversion disagree"
                    .to_string(),
            );
        };
        if first != header {
            self.body.set_terminator(
                header,
                Terminator::Br(Br {
                    target: first,
                    args: self.body.blocks[header].params.clone(),
                }),
            );
        }
        self.loops.push(ActiveLoop {
            function,
            conversion: conversion.clone(),
            block_slots: self.body.blocks[header].params.clone(),
            entry,
            block: header,
            header,
            tests,
            steps,
            exits,
            merge,
        });
        let index = self.loops.len() - 1;
        self.lower_loop_tests(index)?;
        self.lower_loop_steps(index)?;
        let result = self.lower_loop_exit(index)?;
        self.loops.pop();
        // The caller's remaining instructions go in the merge block, which is
        // where the nest leaves its result.
        self.block = merge;
        self.values.insert(node, result);
        Ok(result)
    }

    /// The carried state the entering call binds, one value per slot, in the
    /// conversion's order.
    ///
    /// # Invariant
    /// The addressing is [`LoopConversion::state`], so a one-slot empty path is
    /// the whole argument and `[k]` is element `k` of its value; a tuple of the
    /// wrong arity is refused rather than guessed at.
    fn loop_state(
        &mut self,
        node: NodeId,
        conversion: &LoopConversion,
    ) -> Result<Vec<ValueId>, String> {
        let Some(argument) = self.module.operands_of(node).ok().and_then(|operands| {
            operands
                .get(1)
                .copied()
                .or_else(|| operands.first().copied())
        }) else {
            return Err(
                "a `@loop` call names no argument: an Apply reads [callee, argument] and this one \
                 has neither"
                    .into(),
            );
        };
        let value = self.module.pair_value_half(argument).unwrap_or(argument);
        let scalar = conversion.state.len() == 1 && conversion.state[0].is_empty();
        if scalar {
            return Ok(vec![self.value(value)?]);
        }
        let elements = unsafe { self.module.array_items(value) }.ok_or_else(|| {
            format!(
                "a `@loop` call carries {} state slot(s), and its argument is not a tuple of that \
                 arity — the state a kernel loop carries is the entering call's own element list",
                conversion.state.len()
            )
        })?;
        if elements.len() != conversion.state.len() {
            return Err(format!(
                "a `@loop` call carries {} state slot(s) and its argument has {} element(s): the \
                 conversion's slots and the call's argument are the same list",
                conversion.state.len(),
                elements.len()
            ));
        }
        elements
            .iter()
            .map(|element| {
                let element = dynamic(element.node)?;
                self.value(element)
            })
            .collect()
    }

    /// A fresh nest block receiving `slots` parameters.
    fn loop_block(&mut self, slots: usize) -> usize {
        let block = self.body.add_block();
        for _ in 0..slots {
            self.body.add_param(block);
        }
        block
    }

    /// One test: its condition, then a two-way branch on the arm chosen.
    fn lower_loop_tests(&mut self, index: usize) -> Result<(), String> {
        for position in 0..self.loops[index].tests.len() {
            let block = self.loops[index].tests[position];
            self.enter_loop_block(index, block);
            let condition = self.loops[index].conversion.tests[position].condition;
            let condition = self.value(condition)?;
            let test = self.loops[index].conversion.tests[position];
            let (if_true, if_false) = self.loop_arms(index, test.on_one, test.on_zero)?;
            self.body.set_terminator(
                block,
                Terminator::CondBr {
                    cond: condition,
                    if_true,
                    if_false,
                },
            );
        }
        Ok(())
    }

    /// One step: the next state, then the backedge that hands it to the header.
    fn lower_loop_steps(&mut self, index: usize) -> Result<(), String> {
        for position in 0..self.loops[index].steps.len() {
            let block = self.loops[index].steps[position];
            self.enter_loop_block(index, block);
            let next = self.loops[index].conversion.steps[position].next.clone();
            let next: Vec<ValueId> = next
                .into_iter()
                .map(|value| self.value(value))
                .collect::<Result<_, _>>()?;
            let header = self.loops[index].header;
            self.body.set_terminator(
                block,
                Terminator::Br(Br {
                    target: header,
                    args: next,
                }),
            );
        }
        Ok(())
    }

    /// One exit: the base's value, then the branch into the merge block.
    fn lower_loop_exit(&mut self, index: usize) -> Result<ValueId, String> {
        let mut result = None;
        for position in 0..self.loops[index].exits.len() {
            let block = self.loops[index].exits[position];
            self.enter_loop_block(index, block);
            let value = self.loops[index].conversion.exits[position].value;
            let value = self.value(value)?;
            let merge = self.loops[index].merge;
            self.body.set_terminator(
                block,
                Terminator::Br(Br {
                    target: merge,
                    args: vec![value],
                }),
            );
            result = Some(value);
        }
        result.ok_or_else(|| {
            "a `@loop` conversion has no exit, so the nest it names has no result — the conversion \
             refuses that shape, and the two readers of one conversion disagree"
                .to_string()
        })
    }

    /// Set the nest's current block; a state read resolves to that block's parameters.
    fn enter_loop_block(&mut self, index: usize, block: usize) {
        self.block = block;
        self.loops[index].block = block;
        self.loops[index].block_slots = self.body.blocks[block].params.clone();
    }

    /// The two branches one test's arms take.
    fn loop_arms(
        &mut self,
        index: usize,
        on_one: LoopArm,
        on_zero: LoopArm,
    ) -> Result<(Br, Br), String> {
        Ok((
            self.loop_arm(index, on_one)?,
            self.loop_arm(index, on_zero)?,
        ))
    }

    /// Where one arm goes, handing the current state on unchanged.
    fn loop_arm(&mut self, index: usize, arm: LoopArm) -> Result<Br, String> {
        let active = &self.loops[index];
        let target = match arm {
            LoopArm::Test(next) => active.tests[next],
            LoopArm::Step(next) => active.steps[next],
            LoopArm::Exit(next) => active.exits[next],
        };
        Ok(Br {
            target,
            args: active.block_slots.clone(),
        })
    }

    /// The class a value this walk has already emitted was declared in.
    ///
    /// # Invariant
    /// Read from what was emitted, not from the node: an operator's operands are
    /// emitted before it, and a float body's index and count are `Int` positions —
    /// so an operator adding two floats computes in `Float` whatever class the
    /// checker's node carries.
    fn emitted_class(&self, value: AnyNodeId) -> Option<ScalarClass> {
        let AnyNodeId::Dynamic(value) = value else {
            return None;
        };
        self.classes.get(&value).copied()
    }

    /// The values a program's own operator reads — its operand array's elements.
    ///
    /// # Invariant
    /// This is the other half of [`Module::operands_of`]: that answers which nodes a
    /// definition depends on — for a program operator, the whole array, built before
    /// the operator that indexes it — while this answers what the operator reads,
    /// which is whatever the array holds.
    fn arguments(&self, node: NodeId) -> Result<Vec<AnyNodeId>, String> {
        let Some(operation) = self.module.node_operation(node) else {
            return Ok(Vec::new());
        };
        let Some(operand) = operation.operand else {
            return Ok(Vec::new());
        };
        Ok(self
            .module
            .operand_items(operand)?
            .iter()
            .map(|item| item.node)
            .collect())
    }

    /// The value an operand node names, frozen or not.
    ///
    /// # Invariant
    /// A static operand is a constant the apply clone carried across, not a value
    /// this graph computes: a routed operator's arguments include the frozen
    /// residual of the call it was lowered from, and the only part of a static
    /// module a kernel can carry is a scalar. It becomes a `Const` of what it holds,
    /// and anything else is refused by name rather than skipped.
    fn value_item(&mut self, item: AnyNodeId) -> Result<ValueId, String> {
        let AnyNodeId::Dynamic(node) = item else {
            let held = self
                .module
                .node_value(item)
                .and_then(|value| AsEnum::<LowValue>::as_enum(&value));
            return match held {
                Some(LowValue::USize(value)) => Ok(self.emit_const(ScalarClass::Int, value as i64)),
                Some(LowValue::Float(value)) => {
                    Ok(self.emit_const(ScalarClass::Float, float_bits(value)))
                }
                other => Err(format!(
                    "a kernel body reached a frozen module''s node holding {other:?} where it needs \
                     a value: a static module is the callee of an apply, and the only part of it \
                     a kernel can carry across is a scalar constant"
                )),
            };
        };
        self.value(node)
    }

    /// An `Index`: a view, a parameter read, or the language's conditional.
    fn index(&mut self, node: NodeId) -> Result<ValueId, String> {
        // **The conditional first**: a view never computes, so only the index tells them apart.
        if let Some(lichen_lowlevel::Selection::Computed) = self.module.selection_of(node) {
            return self.conditional(node);
        }
        // **A read at a path is a parameter read** even where `define_in` said
        // `Computed`; see [`Lower::parameter_path`].
        if let Some(slot) = self.parameter_path(node)? {
            return self.parameter(slot);
        }
        // Anything else that is a view was resolved by `define_in`, so this index
        // names nothing this body can place.
        let arguments = self.arguments(node)?;
        let Some(target) = arguments.first() else {
            return Err("an index read is missing its target".into());
        };
        let index = arguments.get(1);
        // **What it saw, in the graph's terms**: the refusal names the target, the index
        // and the chain.
        let target_kind = match target {
            AnyNodeId::Dynamic(node) => {
                let items = unsafe { self.module.array_items_of(AnyNodeId::Dynamic(*node)) };
                if self.module.node_operation(*node).is_some() {
                    "a computation".to_string()
                } else if let Some(items) = items {
                    format!("an array of {} element(s)", items.len())
                } else if self.module.structural_value(*node).is_some() {
                    "a literal".to_string()
                } else {
                    "a bare cell".to_string()
                }
            }
            AnyNodeId::Static(_) => "a frozen node".to_string(),
        };
        let index_kind = match index {
            Some(index) => match self.module.usize_value(*index) {
                Some(k) => format!("the constant {k}"),
                None => "a value the graph cannot decide".to_string(),
            },
            None => "no index at all".to_string(),
        };
        // **The base the chain reaches**, which is the question when a read should
        // have placed.
        let mut base = Some(node);
        let mut walked = 0usize;
        while let Some(cursor) = base {
            let Some(operation) = self.module.node_operation(cursor) else {
                break;
            };
            if !matches!(
                AsEnum::<LowOperator>::as_enum(&operation.operator),
                Some(LowOperator::Index)
            ) {
                break;
            }
            let Ok(arguments) = self.arguments(cursor) else {
                break;
            };
            let (Some(target), Some(step)) = (arguments.first(), arguments.get(1)) else {
                break;
            };
            walked += 1;
            let _ = self.module.usize_value(*step);
            let AnyNodeId::Dynamic(target) = target else {
                break;
            };
            base = Some(*target);
        }
        let base_kind = match base {
            Some(base) => format!(
                "{base:?} (class {:?}, holds {:?})",
                self.module.class_root(base),
                self.module
                    .structural_value(base)
                    .map(|value| format!("{value:?}"))
            ),
            None => "nothing".to_string(),
        };
        let slots: Vec<String> = self
            .params
            .iter()
            .map(|slot| {
                format!(
                    "pair {:?}/class {:?}, value {:?}/class {:?}",
                    slot.pair,
                    self.module.class_root(slot.pair),
                    slot.value,
                    self.module.class_root(slot.value)
                )
            })
            .collect();
        let leaves: Vec<String> = self
            .domain_leaves()
            .unwrap_or_default()
            .iter()
            .map(|leaf| format!("{leaf:?}/class {:?}", self.module.class_root(*leaf)))
            .collect();
        Err(format!(
            "a kernel body's index cannot be placed: its target is {target_kind} and its index is \
             {index_kind}. The chain runs {walked} step(s) and ends at {base_kind}. The slots are \
             [{}] and the domain is {:?} (class {:?}) with leaves [{}]",
            slots.join("; "),
            self.domain,
            self.module.class_root(self.domain),
            leaves.join("; ")
        ))
    }

    /// `if c then a else b` — a selection over an undecided index.
    ///
    /// # Invariant
    /// The language's conditional is `[else, then][condition]`, so the arms come off
    /// the target array in that order and both are emitted before the select —
    /// which is what leaves a real branch available to build.
    fn conditional(&mut self, node: NodeId) -> Result<ValueId, String> {
        let arguments = self.arguments(node)?;
        let Some((target, selector)) = arguments.first().zip(arguments.get(1)) else {
            return Err("a conditional is missing its arms or its selector".into());
        };
        let Some(arms) = (unsafe { self.module.array_items_of(*target) }) else {
            return Err("a conditional's arms are not an array value".into());
        };
        if arms.len() != 2 {
            return Err(format!(
                "a conditional's arms are a two-element array, and this one has {}",
                arms.len()
            ));
        }
// **`[else, then]`**, so element 0 runs when the condition is false.
        let (otherwise, then) = (arms[0].node, arms[1].node);
        let emitted = self.tally.writes;
        let otherwise = self.value_item(otherwise)?;
        if self.tally.writes != emitted {
            return Err(CONDITIONAL_WRITE.into());
        }
        let emitted = self.tally.writes;
        let then = self.value_item(then)?;
        if self.tally.writes != emitted {
            return Err(CONDITIONAL_WRITE.into());
        }
        let selector = self.value_item(*selector)?;
        // **`I32WrapI64` is the narrowing**, a target-width fact the consumer must not
        // re-derive.
        let narrow = self.emit(KernelInstr::I32WrapI64, vec![selector], ScalarClass::Int);
        Ok(self.emit(
            KernelInstr::Select,
            vec![then, otherwise, narrow],
            node_class(self.module, node),
        ))
    }

    fn conversion(
        &mut self,
        operand: NodeId,
        from: ScalarClass,
        to: ScalarClass,
        name: &'static str,
    ) -> Result<ValueId, String> {
        let items = self.module.operand_items(operand)?;
        if items.len() != 1 {
            return Err(format!(
                "`{name}` takes one operand and its operand array has {}",
                items.len()
            ));
        }
        let operand = dynamic(items[0].node)?;
        // **A literal converts here, in the language's own classes**, rather than at
        // run time.
        if let Some(literal) = scalar_literal(self.module, operand) {
            let (held, number) = match literal {
                LowValue::USize(n) => (ScalarClass::Int, n as i64),
                LowValue::Float(f) => {
                    let truncated = f.trunc();
                    // A kernel has no channel to record a diagnostic, so an out-of-range
                    // literal is refused by name.
                    if !f.is_finite()
                        || truncated < 0.0
                        || (truncated as f64) >= (usize::MAX as f64)
                    {
                        return Err(format!(
                            "`{name}` of {f} is out of range for the language's unsigned `Int`: a \
                             kernel cannot record the diagnostic the interpreter would, so the \
                             conversion is refused rather than answered with a trap or an undefined \
                             value"
                        ));
                    }
                    (ScalarClass::Float, truncated as i64)
                }
                _ => unreachable!("`scalar_literal` answers only the two scalar classes"),
            };
            // The direction is the operator's, so a literal of the other class means the
            // graph and the word disagree.
            if held != from {
                return Err(format!(
                    "`{name}` converts a {from:?} and its operand is a {held:?} literal — the two \
                     classes do not convert to each other on their own, so this graph names one \
                     operator and carries a value of the other"
                ));
            }
            return Ok(self.emit_const(to, const_bits(to, number)));
        }
        let operand = self.value(operand)?;
        Ok(self.emit(KernelInstr::Conv { from, to }, vec![operand], to))
    }

    /// A cross-kernel call: `compute.launch k x` / `compute.call k x`.
    fn cross_kernel_call(&mut self, kernel: AnyNodeId, arg: AnyNodeId) -> Result<ValueId, String> {
        let kernel = dynamic(kernel)?;
        let arg = dynamic(arg)?;
        let kid = kernel_id_of(self.module, kernel)
            .ok_or_else(|| "cross-kernel call target is not a kernel value".to_string())?;
        // The callee's shape, arity and class are read and the registry released before
        // any emission.
        let (shape, callee_params, results) = {
            let fragments = kernels().lock().unwrap();
            let fragment = fragments
                .get(&kid)
                .ok_or_else(|| "cross-kernel callee is not a registered kernel".to_string())?;
            (
                fragment.param_shape.clone(),
                param_classes(fragment),
                fragment.result_classes.len(),
            )
        };
        if results != 1 {
            return Err(format!(
                "cross-kernel call to kernel {kid}, which returns {results} value(s): \
                 {CROSS_KERNEL_RESULT_ARITY}"
            ));
        }
        // **The argument's class must be the callee's**: a wasm `call` types its operand
        // by the signature.
        let peeled = self.module.pair_value_half(arg).unwrap_or(arg);
        let arg_class = node_class(self.module, peeled);
        if let Some(expected) = callee_params.first()
            && *expected != arg_class
        {
            return Err(format!(
                "cross-kernel call to kernel {kid}, whose parameter is lowered in {expected:?}, \
                 from an argument lowered in {arg_class:?}: Int and Float do not convert, so the \
                 call has no signature"
            ));
        }
        let args = self.callee_args(arg, &shape)?;
        Ok(self.emit(KernelInstr::CallKernel(kid), args, arg_class))
    }

    /// A call's argument as the callee domain's scalar leaves, in callee parameter
    /// order.
    ///
    /// # Invariant
    /// Three shapes, tried in the order the graph can rule them out: a read of the
    /// domain at a path ([`Lower::parameter_path`]), which passes through as the
    /// caller's own contiguous parameters; the `[value, type]` pair's value; and a
    /// concrete tuple value, element by element, recursing for a nested domain. Each
    /// is ruled out by a fact rather than by a later one failing.
    fn callee_args(&mut self, arg: NodeId, shape: &KernelShape) -> Result<Vec<ValueId>, String> {
        let arity = shape.flat_arity();
        if arity == 0 {
            return Ok(Vec::new());
        }
        if arity == 1 {
            return Ok(vec![
                self.value(self.module.pair_value_half(arg).unwrap_or(arg))?,
            ]);
        }
        let KernelShape::Tuple(items) = shape else {
            return Ok(vec![self.value(arg)?]);
        };
        // (1) A read of the domain, passed through.
        if let Some(base) = self.parameter_path_of(arg)?
            && base + arity <= self.body.parameters().len()
        {
            let mut args = Vec::with_capacity(arity);
            for offset in 0..arity {
                args.push(self.parameter(base + offset)?);
            }
            return Ok(args);
        }
        // (2) and (3): the pair's value half, read as a concrete tuple.
        let array = self.module.pair_value_half(arg).unwrap_or(arg);
        // **The read may be at the wrapper's class**: the pair's value half is its own
        // singleton cell.
        if let Ok(args) = self.tuple_leaves(array, items) {
            return Ok(args);
        }
        let by_class = self.module.class_root(arg);
        if by_class != array {
            return self.tuple_leaves(by_class, items);
        }
        Err(CALLEE_ARGUMENT.into())
    }

    /// The leaves of one concrete tuple value against the domain elements `items`.
    fn tuple_leaves(
        &mut self,
        node: NodeId,
        items: &[KernelShape],
    ) -> Result<Vec<ValueId>, String> {
        let arity: usize = items.iter().map(KernelShape::flat_arity).sum();
        let Some(elements) = (unsafe { self.module.array_items(node) }) else {
            return Err(CALLEE_ARGUMENT.into());
        };
        if elements.len() != items.len() {
            return Err(CALLEE_ARGUMENT.into());
        }
        let mut args = Vec::with_capacity(arity);
        for (element, item) in elements.iter().zip(items) {
            let element = dynamic(element.node)?;
            match item {
                KernelShape::Scalar(_) => {
                    let value = self.value(element)?;
                    args.push(value)
                }
                KernelShape::Tuple(nested) => args.extend(self.tuple_leaves(element, nested)?),
            }
        }
        Ok(args)
    }

    /// Read an input buffer element: `read [cfg(1)(k), idx]`.
    fn buffer_read(&mut self, operand: NodeId) -> Result<ValueId, String> {
        let operands = self.module.operand_pair(operand, "read")?;
        let (Some(buffer), Some(index)) = (operands.first(), operands.get(1)) else {
            return Err("read's operand array must have two elements".into());
        };
        // The buffer operand comes through the wrapper's slot-read
        // destructuring, so resolve it to the actual buffer node.
        let unpeeled = AnyNodeId::Dynamic(*buffer);
        let buffer = peeled_argument(self.module, unpeeled)?;
        let pos = parallel_buffer_pos(self.module, self.params, buffer, unpeeled)?
            .ok_or_else(|| {
            let seen = match self.params.first().and_then(|slot| slot.roles.as_ref()) {
                Some(roles) => format!("the parameter's inputs are {:?}", roles.inputs),
                None => "the parameter declares no inputs".to_string(),
            };
            format!(
                "read's buffer argument is not an input buffer of the parallel parameter ({seen})"
            )
        })?;
        // **A read's element class is declared, not inferred**: the host binds the
        // buffer.
        let element = self.tally.element_class.unwrap_or(ScalarClass::Int);
        self.tally.reads = self.tally.reads.max(pos + 1);
        self.tally.read_classes.push(element);
        // The position is an `Int`, always: a compile-time ordinal in the input
        // space, not data.
        let position = self.emit_const(ScalarClass::Int, pos as i64);
        let index = self.value(*index)?;
        Ok(self.emit(
            KernelInstr::BufferReadCall(element),
            vec![position, index],
            element,
        ))
    }

    /// Write a buffer element: `write [buffer, idx, val]`.
    ///
    /// # Invariant
    /// The ordinal is taken before the operands are emitted, so a write nested inside
    /// another write's value still consumes an ordinal of its own — which the
    /// codomain count check refuses.
    fn buffer_write(&mut self, operand: NodeId) -> Result<ValueId, String> {
        let items = self.module.operand_items(operand)?;
        if items.len() < 3 {
            return Err(format!(
                "a write takes three operands and its operand array has {}",
                items.len()
            ));
        }
        let index = dynamic(items[1].node)?;
        let value_node = dynamic(items[2].node)?;
        let ordinal = self.tally.writes;
        self.tally.writes += 1;
        // The element a write fills is the class of the value written.
        let element = node_class_in(self.module, self.params, value_node);
        self.tally.write_classes.push(element);
        let position = self.emit_const(ScalarClass::Int, ordinal as i64);
        let index = self.value(index)?;
        let value = self.value(value_node)?;
        Ok(self.emit(
            KernelInstr::BufferWriteCall(element),
            vec![position, index, value],
            element,
        ))
    }
}

/// Refusal: a cross-kernel call's argument is neither a whole-parameter read nor a
/// concrete tuple value.
///
/// # Invariant
/// The flattened layout is what makes the other cases work, so a wrong one would read
/// a parameter the argument does not own.
pub(super) const CALLEE_ARGUMENT: &str = "a cross-kernel call's argument must be a concrete tuple value or a whole parameter read; \
     build the argument from its elements (or pass the parameter through)";

fn dynamic(node: lichen_lowlevel::AnyNodeId) -> Result<NodeId, String> {
    match node {
        lichen_lowlevel::AnyNodeId::Dynamic(node) => Ok(node),
        lichen_lowlevel::AnyNodeId::Static(_) => Err(
            "a kernel body reads a static reference into a frozen module, which has no node in \
             this graph"
                .into(),
        ),
    }
}

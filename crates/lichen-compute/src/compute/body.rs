//! A kernel body, lowered: **a graph walk that emits SSA**.
//!
//! # What this replaced, and why the shape changed
//!
//! This was `emit_node`: a recursive walk that pushed [`KernelInstr`]s onto a
//! `Vec`. Instructions named no values, so the walk's *recursion* was its
//! ordering, and three things followed that were properties of the IR rather
//! than of any kernel:
//!
//! - **a shared subexpression was emitted once per use**, because nothing could
//!   name it after the first;
//! - **every consumer reconstructed the operand stack**, each deriving the form
//!   it wanted — `waffle` derives a stack from SSA, `spirv.rs` derives SSA ids
//!   from a stack;
//! - **the walk recursed over the graph**, so it needed a depth budget and
//!   `#[stacksafe]` to survive a deeply expanded body.
//!
//! Here the walk emits into a [`KernelBody`] — SSA values, blocks with
//! parameters — and the memo [`Lower::values`] is what makes a node emit **once**.
//!
//! # Resolution is the lowlevel's, not this file's
//!
//! Which value a node names — a parameter leaf, a computation, nothing — is
//! answered by `define_in` and `selection_of` (`lichen_lowlevel::resolve`),
//! because it is a fact about cells and equality classes. This file used to
//! re-derive it on every bare cell it reached, through `equality_rep` and
//! `class_computation_node`; both moved down and neither exists here.
//!
//! # The one thing that is still a count
//!
//! [`Positions`] — the buffer ordinals a parallel body reads and writes. **It is
//! filled by this walk rather than by a consumer**, because it is a *shared*
//! fact: the fragment's `input_classes` and `output_classes` and the refusal
//! wording a backend uses have to agree, and two counters that disagreed is the
//! defect `mixed_classes`'s contract exists to prevent.

use std::collections::HashMap;

use lichen_kernel_ir::{
    KernelBin, KernelBody, KernelInstr, KernelShape, ScalarClass, Terminator, ValueId,
};
use lichen_lowlevel::{AnyNodeId, Define, LowOperator, LowValue, Module, NodeId, Program};
use lichen_utils::extend::AsEnum;

use super::{
    ComputeOperator, ComputeValue, ParamSlot, Positions, TypeOperator, const_bits, conv_of,
    flat_arity, float_bits, is_static_function, kernel_bin, kernel_id_of, kernels, node_class,
    node_class_in, parallel_buffer_pos, param_classes, peeled_argument, scalar_literal,
};

/// The parameter slots' classes in flattening order — **the ABI's own list**,
/// since a leaf's class is the parameter's and not the body's.
fn param_classes_of(params: &[ParamSlot]) -> Vec<ScalarClass> {
    params
        .iter()
        .flat_map(|slot| super::scalar_classes_of(&slot.shape))
        .collect()
}

/// How many parameter leaves the body's entry block receives — **the declared
/// shape's arity**, which is the ABI's count and not the domain node's.
///
/// A kernel's parameter is a struct carrying a native wrapper and a signature, so
/// the node a caller hands over is the wrapper's cell rather than the tuple the
/// domain declares. Reading the count off the node gave one leaf for a two-leaf
/// domain, and every tuple read then failed to place.
fn domain_arity(params: &[ParamSlot]) -> usize {
    params
        .iter()
        .map(|slot| super::flat_arity(&slot.shape))
        .sum()
}

/// One function's lowering: the body being built, and everything the walk needs
/// One function's lowering: the body being built, and everything the walk needs
/// to place a value in it.
pub struct Lower<'a, P: Program> {
    module: &'a Module<P>,
    /// The parameter slots, for the class and path reads the buffer arms need.
    params: &'a [ParamSlot],
    /// The domain's **value** node — its leaves are this body's parameters, and
    /// the caller states it rather than this file decoding a pair out of
    /// `Function::parameter`.
    domain: NodeId,
    tally: &'a mut Positions,
    body: KernelBody,
    entry: usize,
    /// A node to the value it produced. **This is what makes emission happen
    /// once**, so a shared subexpression is computed once.
    values: HashMap<NodeId, ValueId>,
    /// How many definitions deep the walk is — the guard that replaced a
    /// hand-rolled operand stack's failure mode.
    depth: usize,
    /// The class each emitted value was declared in, keyed by the node it came
    /// from — what an operator consults to learn what class its operands are.
    classes: HashMap<NodeId, ScalarClass>,
    /// The entry block's parameters' classes, in flattening order — **the ABI's
    /// own list**, since a leaf's class is the parameter's and not the body's.
    leaf_classes: Vec<ScalarClass>,
    /// The node whose definition is being emitted, so [`Lower::emit`] can record
    /// the class it declared against it.
    defining: NodeId,
}

/// A cross-kernel callee must return exactly one value, and it must be the
/// caller's own class: a cross-kernel call is an ordinary wasm `call`, so a float
/// caller passing an `f32` to an integer callee — or reading an `i64` back into a
/// float body — would be a module that does not validate.
const CROSS_KERNEL_RESULT_ARITY: &str = "a cross-kernel call to a kernel that returns more than one value is not supported: a kernel body reads a callee result as a single value, and re-materialising a tuple result needs local slots the kernel instruction set does not yet have";

/// The walk''s depth ceiling. **A refusal, not a panic**: this is the condition a
/// program can be written against, and it is what a deeply *expanded* recursion
/// runs into.
const MAX_KERNEL_BODY_DEPTH: usize = 512;

impl<'a, P: Program> Lower<'a, P>
where
    P::Value: From<ComputeValue> + AsEnum<ComputeValue>,
    P::Operator: AsEnum<TypeOperator> + AsEnum<ComputeOperator>,
{
    /// Lower `codomain` — the value the function returns — into a body.
    ///
    /// The entry block's parameters are the domain's leaves **in the ABI's
    /// order**, which is what makes an old `LocalGet(k)` and a loop's carried
    /// value the same read.
    pub fn lower(
        module: &'a Module<P>,
        params: &'a [ParamSlot],
        domain: NodeId,
        codomain: NodeId,
        tally: &'a mut Positions,
    ) -> Result<KernelBody, String> {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        // **The parameter count is the shape's, not the domain node's.** A kernel's
        // parameter is a struct carrying a native wrapper and a signature, so the
        // node the caller hands over is the wrapper's cell rather than the tuple the
        // domain declares — reading the count off the node gave one leaf for a
        // two-leaf domain, and every tuple read then failed to place.
        for _ in 0..domain_arity(params) {
            body.add_param(entry);
        }
        let mut lower = Lower {
            module,
            params,
            domain,
            tally,
            body,
            entry,
            values: HashMap::new(),
            depth: 0,
            classes: HashMap::new(),
            leaf_classes: param_classes_of(params),
            defining: NodeId::default(),
        };
        let mut values = Vec::new();
        for node in lower.leaves_of(codomain)? {
            values.push(lower.value(node)?);
        }
        lower
            .body
            .set_terminator(lower.entry, Terminator::Return { values });
        Ok(lower.body)
    }

    /// A parallel index function, lowered: **each output is a `compute.write`**.
    ///
    /// The outputs are emitted **in position order**, so write `k` takes the
    /// ordinal `k` — the ordinal is a fact of the codomain's order, not of when
    /// the walk happened to reach it.
    ///
    /// **The writes produce nothing** (`KernelInstr::produces` is `0` for a
    /// write), so the body has no value to return from them. A parallel kernel's
    /// results *are* its output buffers, read out of what the `write` import
    /// filled, so the signature still declares one result and the terminator
    /// returns a value of the fragment's class to match it. That is a dummy, and
    /// it is stated here rather than left implicit: a body whose returned value
    /// was supposed to mean something would be indistinguishable from one where
    /// it does not.
    pub fn lower_index_function(
        module: &'a Module<P>,
        params: &'a [ParamSlot],
        domain: NodeId,
        outputs: &[NodeId],
        class: ScalarClass,
        tally: &'a mut Positions,
    ) -> Result<KernelBody, String> {
        let mut body = KernelBody::new();
        let entry = body.add_block();
        for _ in 0..domain_arity(params) {
            body.add_param(entry);
        }
        // **A parallel fragment's domain is its config's leaves *plus* the
        // invocation index**, which is the one value `compute.range` reads. The
        // index is not part of the config parameter's own shape — it is this
        // target's way of naming "which lane am I" — so it is added here rather
        // than counted from a shape that does not contain it.
        body.add_param(entry);
        let mut lower = Lower {
            module,
            params,
            domain,
            tally,
            body,
            entry,
            values: HashMap::new(),
            depth: 0,
            classes: HashMap::new(),
            leaf_classes: param_classes_of(params),
            defining: NodeId::default(),
        };
        for output in outputs {
            // The value a write leaves is discarded on purpose: see the doc.
            lower.value(*output)?;
        }
        let dummy = lower.emit_const(class, const_bits(class, 0));
        lower.body.set_terminator(
            lower.entry,
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
        if let Some(&value) = self.values.get(&node) {
            return Ok(value);
        }
        if self.depth > MAX_KERNEL_BODY_DEPTH {
            return Err(super::kernel_body_too_deep());
        }
        self.depth += 1;
        // **Every emission for this node records its class against this node.**
        // A literal is emitted straight from here rather than from `definition`, so
        // setting it once here covers both — and recording it against the wrong
        // node is how a float constant ended up unrecorded and its operator fell
        // back to the node's own class.
        self.defining = node;
        // **A read of the domain at a path, matched structurally.** `x(0)` on a
        // tuple domain is a chain of `Index` nodes ending at the parameter, and the
        // walk recognises it from the chain rather than from the class — which
        // matters because the deep pass may have unified the parameter with the
        // argument it was passed, so the node the read names is a *computation* and
        // only the chain says where it came from.
        let value = match self.parameter_path(node) {
            Some(slot) => {
                // **A parameter's class is the ABI's**, recorded here so an operator
                // that reads one learns what class it is without the fragment.
                let class = self
                    .leaf_classes
                    .get(slot)
                    .copied()
                    .unwrap_or(ScalarClass::Int);
                self.classes.insert(node, class);
                self.parameter(slot)?
            }
            // **A literal is a value, answered before anything else** — before the
            // operation test and before the class walk. A node can hold a decided
            // literal *and* have no operation and no class member that computes it,
            // and answering it as opaque refuses a constant the graph already
            // decided.
            None => match self.literal(node) {
                Some(value) => value,
                None => match self.module.define_in(self.domain, node) {
                    Define::Parameter(leaf) => self.parameter(self.slot_of(leaf)?)?,
                    Define::Computed(definition) => self.definition(definition)?,
                    Define::Opaque => self.opaque(node)?,
                },
            },
        };
        self.depth -= 1;
        self.values.insert(node, value);
        Ok(value)
    }

    /// The parameter read `node` names, through the wrappers it may arrive wrapped in.
    ///
    /// **A call's argument arrives wrapped**, where a body's read does not: a bare
    /// kernel apply carries a fresh `[value, type]` pair whose value half is the
    /// argument, and that half is the parameter — while the pair itself is a node
    /// this body's slots do not name. So the same path walk is tried on the node and
    /// on what one peel takes off it, and the first that lands on a slot wins.
    fn parameter_path_of(&self, node: NodeId) -> Option<usize> {
        let peeled = self.module.pair_value_node(node);
        let viewed = match self.module.selection_of(node) {
            Some(lichen_lowlevel::Selection::Views(view)) => Some(view),
            _ => None,
        };
        self.parameter_path(node)
            .or_else(|| peeled.and_then(|peeled| self.parameter_path(peeled)))
            .or_else(|| viewed.and_then(|viewed| self.parameter_path(viewed)))
    }

    /// The entry parameter a read of the domain at a path names, if that is what
    /// `node` is.
    ///
    /// **Matched from the chain, not from the class of the base.** A parameter the
    /// deep pass unified with an argument is *a computation* by the time codegen
    /// sees it, and a class walk finds nothing there; the chain of `Index` nodes
    /// is what says `x(1)` is a read of the domain rather than of anything else.
    ///
    /// **The base is matched against the parameter slots themselves** — each
    /// slot's pair and its value — rather than against one domain node the caller
    /// happened to hand over. That is the old walk's test, and it is the robust
    /// one: a struct parameter, a tuple domain and a scalar all reach the walk
    /// through a different node, and the slots are what all three share.
    ///
    fn parameter_path(&self, node: NodeId) -> Option<usize> {
        let mut positions: Vec<usize> = Vec::new();
        let mut cursor = node;
        loop {
            // **The base has no operation, and that is the end of the chain** —
            // not a failure to answer. `?` here would return from the whole walk
            // on the ordinary case.
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
                return None;
            };
            let (Some(target), Some(index)) = (arguments.first(), arguments.get(1)) else {
                return None;
            };
            let Some(step) = self.module.usize_value(*index) else {
                return None;
            };
            let AnyNodeId::Dynamic(target) = target else {
                return None;
            };
            // **`Index(param_pair, k)` is the pair's value half, not a step of the
            // domain's path.** It is how a read reaches the parameter at all, and
            // counting it would descend the parameter's *wrapper* struct as though
            // it were the domain's shape.
            //
            // **The cursor still moves onto the pair** before the walk stops: the
            // pair is where the read ended up, and it is the base the check below
            // has to see. Stopping one node short of it left the base unmatched and
            // the whole-parameter read unplaced.
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
        // **Or one of the domain's leaves.** A kernel's parameter is a struct
        // carrying a native wrapper and a signature, and a read of `p` reaches the
        // wrapper's field first — so the chain ends at a *leaf* of the domain
        // rather than at the domain, which is why matching the slots alone placed
        // six tuple-domain reads nowhere.
        let on_a_leaf = self
            .domain_leaves()
            .map(|leaves| {
                leaves
                    .iter()
                    .any(|leaf| self.module.class_root(*leaf) == root)
            })
            .unwrap_or(false);
        if !(on_a_slot || on_a_leaf) {
            return None;
        }
        // **The slot is the path *flattened*, and the difference matters at one
        // level of nesting.** A flat tuple's positions sum, because every element
        // before the one named holds exactly one leaf. A nested tuple's do not: in
        // `<<Int, Int>, Int>`, `p(1)` names the outer element at position 1, which
        // starts after the inner tuple's **two** leaves — offset **2**, not 1.
        // Summing read the wrong parameter and the fragment answered `2 + 3 + 3`.
        //
        // **The steps come out innermost-first** — the walk descends from the read
        // — so they are reversed before flattening, or the descent enters the
        // inner tuple at the outer position.
        positions.reverse();
        let mut offset = 0usize;
        let mut shape = match self.params.first() {
            Some(slot) => slot.shape.clone(),
            None => return None,
        };
        for position in &positions {
            let lichen_lowlevel::LowShape::Tuple(items) = &shape else {
                return None;
            };
            let Some(element) = items.get(*position) else {
                return None;
            };
            for skipped in &items[..*position] {
                offset += super::flat_arity(skipped);
            }
            shape = element.clone();
        }
        Some(offset)
    }

    /// The domain's leaves, flattened — the ABI's argument list.
    fn domain_leaves(&self) -> Result<Vec<NodeId>, String> {
        self.module.value_leaves(self.domain)
    }

    /// A value's leaves: itself, or an array's items. **A codomain is at most a
    /// tuple of scalars**, so one level is the whole of it.
    fn leaves_of(&self, value: NodeId) -> Result<Vec<NodeId>, String> {
        self.module.value_leaves(value)
    }

    /// The ABI slot the domain's `leaf`-th leaf takes.
    ///
    /// **A struct domain's whole value is a parameter too**, and its fields are
    /// the ones that take slots — so a body reading the parameter itself and a
    /// body reading one of its fields are both parameter reads, and only the
    /// field names an index.
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

    /// The body's `k`-th parameter.
    ///
    /// **This is what `KernelInstr::LocalGet(k)` was, and it is now a read of a
    /// named value.** That is the whole of what a loop's carried state needs.
    fn parameter(&self, k: usize) -> Result<ValueId, String> {
        self.body.parameters().get(k).copied().ok_or_else(|| {
            format!(
                "a body reads parameter {k}, which a domain of {} leaf/leaves does not have",
                self.body.parameters().len()
            )
        })
    }

    /// Emit `instr` over `args` in the entry block, declaring what it leaves.
    fn emit(&mut self, instr: KernelInstr, args: Vec<ValueId>, class: ScalarClass) -> ValueId {
        let declared = if instr.produces() == 0 {
            Vec::new()
        } else {
            vec![class]
        };
        let value = self.body.add_op(self.entry, instr, args, declared);
        if instr.produces() > 0 {
            // **Recorded against the node being defined**, which is what an
            // operator consults to learn what class its operands are.
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
        // A literal is a value, not a computation, and it is the cheapest thing a
        // node can be.
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
            let Some(rhs) = arguments.get(1) else {
                return Err(format!("{ty_op:?} is missing an operand"));
            };
            let Some(lhs) = arguments.first() else {
                return Err(format!("{ty_op:?} is missing an operand"));
            };
            // **The operator's class is its operands' class**, read off them and
            // falling back to the node's. The node's own class is a hint, not the
            // answer: a float body's index and count are `Int` positions, so an
            // operator that adds two floats computes in `Float` whatever the node
            // the checker hung them on says. Trusting the node here made a float
            // kernel declare `Bin(Int, Add)` over two float values, and the class
            // check refused it — correctly, and for the wrong reason.
            // **The operands first**: the class below is read off what they
            // emitted, and asking before they exist reads nothing and falls back
            // to the node's own.
            let left = self.value_item(*lhs)?;
            let right = self.value_item(*rhs)?;
            let operand_class = self
                .emitted_class(*lhs)
                .or_else(|| self.emitted_class(*rhs))
                .unwrap_or(class);
            let class = operand_class;
            // **A float has no `%` or bitwise form**, and the refusal is here at
            // the operand because the class is still visible.
            if class == ScalarClass::Float
                && matches!(
                    bin,
                    KernelBin::Rem | KernelBin::BitAnd | KernelBin::BitOr | KernelBin::BitXor
                )
            {
                return Err(format!(
                    "`{ty_op:?}` has no float form: a kernel's float operators are `+ - * /` and \
                     the four order comparisons, not `%` or the bitwise operators"
                ));
            }
            return Ok(self.emit(KernelInstr::Bin(class, bin), vec![left, right], class));
        }

        // The compute plugin's own operators.
        if let Some(compute_op) = AsEnum::<ComputeOperator>::as_enum(op) {
            return match compute_op {
                ComputeOperator::Launch | ComputeOperator::Call => {
                    // **A program's own operator reads its operand array's
                    // elements**, not the array: `operands_of` answers "which
                    // nodes does this definition depend on", and for a program
                    // operator that is the array itself — one node, built before
                    // the operator that indexes it.
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
                // The loop index of the current parallel invocation: the
                // parameter immediately after the cfg scalar params.
                ComputeOperator::Range => {
                    let index: usize = self.params.iter().map(|p| flat_arity(&p.shape)).sum();
                    self.parameter(index)
                }
                ComputeOperator::Read => self.buffer_read(operand),
                ComputeOperator::Write => self.buffer_write(operand),
                other => Err(format!(
                    "unsupported compute operator in kernel body: {other:?}"
                )),
            };
        }
        Err(format!(
            "unsupported operation in kernel body: {op:?} (kernel-safe subset is scalar arith)"
        ))
    }

    /// An `Apply`: a call of a routed operator, or a cross-kernel call.
    ///
    /// **Three cases, and the third is the one that is not yet.** The routing
    /// lowers `x + 1` to a call of the prelude's binding, so the frozen callee is
    /// a body this module cannot walk and the **residual** the lowlevel's clone
    /// wrote for the call is what gets emitted. A callee that is a *kernel value*
    /// is a cross-kernel call. An ordinary lichen-function call is Style 1 —
    /// inlining its body — and is refused by name, which is where a marked
    /// recursion reaches the emitter today.
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
        if is_static_function(self.module, callee) {
            let residual = (unsafe { self.module.array_items(node) })
                .and_then(|items| items.first())
                .map(|item| item.node)
                .and_then(|item| match item {
                    AnyNodeId::Dynamic(node) => Some(node),
                    AnyNodeId::Static(_) => None,
                });
            let Some(residual) = residual else {
                return Err(
                    "this kernel body applies a prelude operator where the kernel cannot reach the \
                     body it lowered to: the operator is a call of the prelude's binding, the call \
                     sits in the function's own template (which the compiler never evaluates), and \
                     an operator applied *inside a call's argument* is not materialised the way one \
                     that is the body's own result is. A kernel body can cross-call a kernel with \
                     an argument it reads directly (`k0 x`), and it can apply an operator to a \
                     call's result (`k0 x + 1`); this shape (`k0 (x + 1)`) is the one the emitter \
                     has no node for yet"
                        .into(),
                );
            };
            return self.value(residual);
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
        // Style 1: an ordinary lichen-function call. **This is where a marked
        // recursion reaches the emitter today** — refused by name rather than
        // inlined, because a cycle cannot be inlined.
        let _ = node;
        Err(
            "kernel body Apply is supported only for a cross-kernel (kernel-value) callee v1; \
             inline lichen-function calls are not yet supported"
                .into(),
        )
    }

    /// The class a value this walk has already emitted was declared in.
    ///
    /// **Read from what was emitted, not from the node.** An operator's operands
    /// are emitted before the operator, so by the time the operator asks what
    /// class its operands are, this walk knows — and that is the *whole* reason
    /// the class belongs here: a float body's index and count are `Int` positions,
    /// so an operator adding two floats computes in `Float` whatever the node the
    /// checker hung them on says. Trusting the node made a float kernel declare
    /// `Bin(Int, Add)` over two float values, and the class check refused it —
    /// correctly, and for the wrong reason.
    fn emitted_class(&self, value: AnyNodeId) -> Option<ScalarClass> {
        let AnyNodeId::Dynamic(value) = value else {
            return None;
        };
        self.classes.get(&value).copied()
    }

    /// The values a **program's own** operator reads — its operand array's elements.
    ///
    /// This is the other half of [`Module::operands_of`], and the two are different
    /// questions: that one answers *which nodes a definition depends on* (so a
    /// program operator's whole array is one dependency, the array being built before
    /// the operator that indexes it), while this answers *what the operator reads*,
    /// which for a program's operator is whatever its array holds — two operands for
    /// an arithmetic operator, three for a write.
    ///
    /// **Which is which is the lowlevel's to answer** because `LowOperator`'s enum
    /// documents the structural shapes and says nothing about a program's own.
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
    /// **A static operand is a constant the apply clone carried across**, not a
    /// value this graph computes: a routed operator''s arguments include the
    /// frozen residual of the call it was lowered from, and the only part of a
    /// static module a kernel can carry is a scalar. So it becomes a `Const` of
    /// what it holds, and anything else is refused by name rather than skipped.
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
        // **The conditional first**, because a view never computes and the two
        // can only be told apart by the index.
        if let Some(lichen_lowlevel::Selection::Computed) = self.module.selection_of(node) {
            return self.conditional(node);
        }
        // **A read of the domain at a path is a parameter read**, even where
        // `define_in` answered `Computed` for it: the node the read names may be a
        // computation — the parameter unified with its argument — and only the
        // chain says where it came from. Checked here as well as in `value`,
        // because a definition is reached from whatever used it, not from itself.
        if let Some(slot) = self.parameter_path(node) {
            return self.parameter(slot);
        }
        // Anything else that is a view has already been resolved by
        // `define_in`, so reaching it here means the index named nothing this
        // body can place.
        let arguments = self.arguments(node)?;
        let Some(target) = arguments.first() else {
            return Err("an index read is missing its target".into());
        };
        let index = arguments.get(1);
        // **What it saw, in the terms the graph has.** A refusal that says only
        // "cannot place" leaves the reader guessing between a target that is not an
        // array, an index that is not a constant, and an arm count that is not two
        // — and those are three different defects.
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
        // **The base the chain reaches**, so a reader can see whether this is the
        // domain, a pair, or something else entirely — which is the whole question
        // when a read should have placed and did not.
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
    /// **Both arms are emitted before the select**, and they are separate values:
    /// the walk emits the arms because the graph holds them, not because a target
    /// needs two arms. That is what leaves a real branch available to build.
    ///
    /// The language's conditional is `[else, then][condition]` — an ordinary lazy
    /// index — so the arms come off **the target array**, not off the index's own
    /// operand array, and they are in that order.
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
        // **`[else, then]`**, so element 0 is the arm that runs when the condition
        // is false.
        let (otherwise, then) = (arms[0].node, arms[1].node);
        let otherwise = self.value_item(otherwise)?;
        let then = self.value_item(then)?;
        let selector = self.value_item(*selector)?;
        // The selector is the language's `0`/`1` scalar; a consumer whose own
        // condition is narrower narrows it here. **`I32WrapI64` is that
        // narrowing, as an instruction, because it is a target-width fact the
        // consumer must not have to re-derive.**
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
        // **A literal converts here, in the language's own classes.** The
        // conversion is the one instruction whose operand and result differ, so a
        // literal the checker already decided folds rather than being emitted as
        // a constant of the operand's class and converted at run time.
        if let Some(literal) = scalar_literal(self.module, operand) {
            let (held, number) = match literal {
                LowValue::USize(n) => (ScalarClass::Int, n as i64),
                LowValue::Float(f) => {
                    let truncated = f.trunc();
                    // The range rule the interpreter answers with
                    // `operator.out_of_range`. A kernel has no channel to record
                    // a diagnostic — wasm traps and SPIR-V is undefined — so a
                    // literal this layer can see is refused by name, where the
                    // answer is the same for both backends.
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
            // The direction is the operator's, and a literal of the class it does
            // not name means the graph and the word disagree.
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
        // The callee's domain, its result arity **and its class** are facts of
        // the *callee's* registration, read here and released before any
        // emission: emitting can reach a further call, which locks the same
        // registry again, and the lock is not reentrant.
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
        // **The argument's class must be the callee's parameter class**,
        // because a wasm `call` types its operand by the callee's signature.
        let peeled = self.module.pair_value_node(arg).unwrap_or(arg);
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

    /// A call's argument as the callee domain's scalar leaves, in callee
    /// parameter order.
    ///
    /// **Three shapes, tried in the order the graph can rule them out** — and this
    /// is not the encoding-guessing loop it replaced. The order is:
    ///
    /// 1. **a read of the domain at a path** — `k x` and `k x(0)` are the caller's
    ///    own parameters, contiguous in the flattened layout, so they pass through.
    ///    Matched by [`Lower::parameter_path`], which walks the chain; the earlier
    ///    version compared against the domain's leaves, which is the wrong shape
    ///    for a parameter that is a struct wrapper.
    /// 2. **the `[value, type]` pair's value** — a bare kernel apply carries the
    ///    pair and the argument is its element 0.
    /// 3. **a concrete tuple value**, element by element, recursing for a nested
    ///    domain.
    ///
    /// The old walk tried four encodings in a loop and kept the first that worked.
    /// That is a guess that happens to be checked; the three above are shapes, and
    /// each is ruled out by a fact rather than by a later one failing.
    fn callee_args(&mut self, arg: NodeId, shape: &KernelShape) -> Result<Vec<ValueId>, String> {
        let arity = shape.flat_arity();
        if arity == 0 {
            return Ok(Vec::new());
        }
        if arity == 1 {
            return Ok(vec![
                self.value(self.module.pair_value_node(arg).unwrap_or(arg))?,
            ]);
        }
        let KernelShape::Tuple(items) = shape else {
            return Ok(vec![self.value(arg)?]);
        };
        // (1) A read of the domain, passed through.
        if let Some(base) = self.parameter_path_of(arg)
            && base + arity <= self.body.parameters().len()
        {
            let mut args = Vec::with_capacity(arity);
            for offset in 0..arity {
                args.push(self.parameter(base + offset)?);
            }
            return Ok(args);
        }
        // (2) and (3): the pair's value half, read as a concrete tuple.
        let array = self.module.pair_value_node(arg).unwrap_or(arg);
        self.tuple_leaves(array, items)
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
                KernelShape::Scalar(_) => args.push(self.value(element)?),
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
        let buffer = peeled_argument(self.module, *buffer)?;
        let pos = parallel_buffer_pos(self.module, self.params, buffer)?.ok_or_else(|| {
            let seen = match self.params.first().and_then(|slot| slot.roles.as_ref()) {
                Some(roles) => format!("the parameter's inputs are {:?}", roles.inputs),
                None => "the parameter declares no inputs".to_string(),
            };
            format!(
                "read's buffer argument is not an input buffer of the parallel parameter ({seen})"
            )
        })?;
        // **A read's element class is declared, not inferred** — the buffer is
        // bound by the host rather than computed by the body.
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
    /// **The ordinal is taken before the operands are emitted**, so a write nested
    /// inside another write's value still consumes an ordinal of its own — which
    /// is what the codomain count check refuses.
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

/// A cross-kernel call's tuple argument that is neither a whole-parameter read
/// nor a concrete tuple value, under any encoding. **The flattened layout is
/// what makes the other cases work**, so a wrong one would read a parameter the
/// argument does not own.
pub(super) const CALLEE_ARGUMENT: &str = "a cross-kernel call's argument must be a concrete tuple value or a whole parameter read; \
     build the argument from its elements (or pass the parameter through)";

/// The node an array element names, or the reason there is none.
fn target_item(node: lichen_lowlevel::AnyNodeId) -> NodeId {
    match node {
        lichen_lowlevel::AnyNodeId::Dynamic(node) => node,
        // A frozen arm is a constant the clone carried across, and `array_items_of`
        // answers `None` for it below — so this arm is never reached with one.
        lichen_lowlevel::AnyNodeId::Static(_) => NodeId::default(),
    }
}

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

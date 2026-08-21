use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use stacksafe::stacksafe;
use std::alloc::Layout;
use std::collections::{HashMap, HashSet};
use std::ptr;

use crate::utils::disjoint::{self};

pub trait Program: Sized + Copy {
    type Value: ValueExt;
    type Operator: OperatorExt<Self>;
}

pub trait ValueExt: Copy {
    fn is_ptr(&self) -> bool;
    /// Avaliable if [`Self::is_ptr()`].
    fn ptr(&self) -> *const [u8] {
        unreachable!()
    }
    /// Avaliable if [`Self::is_ptr()`].
    fn set_ptr(&mut self, _ptr: *const [u8]) {
        unreachable!()
    }
    /// Avaliable if [`Self::is_ptr()`].
    fn alignment() -> usize {
        unreachable!()
    }
}

pub trait OperatorExt<P: Program>: Copy {
    fn run(
        &self,
        operand: Value<P>,
        block: &mut Block,
        nodes: &SlotMap<NodeId, Node<P>>,
    ) -> Value<P>;
}

#[derive(Clone, Copy)]
pub enum Value<P: Program> {
    Ext(P::Value),
    Array(*const [NodeId]),
    Function(FunctionId),
    USize(usize),
    None,
    Parameterized,
}

#[derive(Clone, Copy)]
pub enum Operator<P: Program> {
    Ext(P::Operator),
    /// - operand[0]: array.
    /// - operand[1]: index.
    Index,
    /// - operand[0]: function.
    /// - operand[1]: argument.
    Apply,
}

#[derive(Clone, Copy)]
pub struct Operation<P: Program> {
    pub operator: Operator<P>,
    pub operand: Option<NodeId>,
}

new_key_type! {pub struct NodeId;}
new_key_type! {pub struct BlockId;}
new_key_type! {pub struct FunctionId;}

pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
    /// Functions homed in this block, registered like nodes so
    /// [`Module::release_block`] drops them (and their scopes) with it.
    pub functions: Vec<FunctionId>,
}

#[derive(Clone)]
pub struct Function {
    /// # Contract:
    /// Only `nodes`` that may reference `parameter`, including `r#return` and `parameter`.
    pub nodes: Vec<NodeId>,
    pub r#return: NodeId,
    pub parameter: NodeId,
    /// Owner.
    pub block: BlockId,
}

pub struct Node<P: Program> {
    pub value: Option<Value<P>>,
    pub operation: Option<Operation<P>>,
    /// Owner.
    /// # Contract
    /// - Only one node can be referenced from parent block
    /// - Referencing a node whose block was released is a panic
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Any node in self's reachable subtree has a [`Value::Parameterized`].
    /// Computed during [`Module::evaluate_node_deep`].
    pub parameterized_deep: Option<bool>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::root_node`].
    pub equality: disjoint::Meta<NodeId>,
}

impl<P: Program> disjoint::Node for Node<P> {
    type Key = NodeId;
    fn meta(&self) -> &disjoint::Meta<NodeId> {
        &self.equality
    }
    fn meta_mut(&mut self) -> &mut disjoint::Meta<NodeId> {
        &mut self.equality
    }
}

pub struct Module<P: Program> {
    pub nodes: SlotMap<NodeId, Node<P>>,
    pub blocks: SlotMap<BlockId, Block>,
    pub functions: SlotMap<FunctionId, Function>,
    /// Nested-application guard: a run panics when function applications
    /// nest deeper than this (a non-terminating function applying itself
    /// directly, e.g. `f(x) = f(x)`).  Defaults to
    /// [`Self::MAX_APPLY_DEPTH`]; tests lower it to panic fast.
    pub apply_depth_limit: usize,
    /// Deep-evaluation guard: a run panics when [`Self::evaluate_node_deep`]
    /// nests deeper than this (deep-evaluating an infinitely growing value,
    /// e.g. `f(x) = [x, f(x)]`).  Defaults to [`Self::MAX_DEEP_DEPTH`],
    /// which sits above the legitimately ~200k-deep block chains exercised
    /// by the `#[stacksafe]` tests; tests lower it to panic fast.
    pub deep_depth_limit: usize,
    apply_depth: usize,
    deep_depth: usize,
}

impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub const MAX_APPLY_DEPTH: usize = 10_000;
    pub const MAX_DEEP_DEPTH: usize = 300_000;

    pub fn new() -> Self {
        Module {
            nodes: SlotMap::with_key(),
            blocks: SlotMap::with_key(),
            functions: SlotMap::with_key(),
            apply_depth_limit: Self::MAX_APPLY_DEPTH,
            deep_depth_limit: Self::MAX_DEEP_DEPTH,
            apply_depth: 0,
            deep_depth: 0,
        }
    }

    pub fn add_block(&mut self, parent: Option<BlockId>) -> BlockId {
        let block = self.blocks.insert(Block {
            arena: Bump::new(),
            parent,
            children: Vec::new(),
            nodes: Vec::new(),
            functions: Vec::new(),
        });
        if let Some(parent) = parent {
            self.blocks[parent].children.push(block);
        }
        block
    }

    pub fn add_node(
        &mut self,
        block: BlockId,
        operation: Option<Operation<P>>,
        value: Option<Value<P>>,
    ) -> NodeId {
        let node = self.nodes.insert(Node {
            value,
            operation,
            block,
            visiting: false,
            parameterized_deep: None,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        node
    }

    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: &[NodeId],
    ) -> NodeId {
        let function = self.functions.insert(Function {
            nodes: nodes.to_vec(),
            r#return: ret,
            parameter: param,
            block,
        });
        self.blocks[block].functions.push(function);
        self.add_node(block, None, Some(Value::Function(function)))
    }

    /// If `id` lives in a child of `referer`, its a block root, [`Self::evaluate_block`] will be called on it.
    pub fn evaluate_node(&mut self, node: NodeId, referer: Option<BlockId>) -> Value<P> {
        let block = self.nodes[node].block;
        debug_assert!(
            self.blocks.contains_key(block),
            "node {node:?} references released block {block:?}"
        );
        if let Some(referer) = referer
            && self.blocks[block].parent == Some(referer)
        {
            return self.evaluate_block(block, node);
        }
        if let Some(value) = self.nodes[node].value {
            return value;
        }
        if self.nodes[node].visiting {
            unreachable!("cycle detected: node {node:?} is being evaluated");
        }
        self.nodes[node].visiting = true;
        let operation = self.nodes[node].operation.unwrap();
        let value = match operation.operator {
            Operator::Index => {
                let Some(operands) = operation.operand else {
                    unreachable!("Index expects an operand array node")
                };
                // A marker anywhere in the operand chain means the index
                // can't be resolved yet — stay lazy so the definition pass
                // can flag the node.
                match self.evaluate_node(operands, Some(block)) {
                    Value::Parameterized => Value::Parameterized,
                    Value::Array(ptr) => {
                        let operands = unsafe { &*ptr };
                        match self.evaluate_node(operands[1], Some(block)) {
                            Value::Parameterized => Value::Parameterized,
                            Value::USize(index) => {
                                match self.evaluate_node(operands[0], Some(block)) {
                                    Value::Parameterized => Value::Parameterized,
                                    Value::Array(ptr) => {
                                        let array = unsafe { &*ptr };
                                        self.evaluate_node(array[index], Some(block))
                                    }
                                    _ => unreachable!("Index target must be an array"),
                                }
                            }
                            _ => unreachable!("Index needs a USize index node"),
                        }
                    }
                    _ => unreachable!("Index operand must be an array of [array, index]"),
                }
            }
            Operator::Ext(ext) => {
                let operand = match operation.operand {
                    Some(operand) => {
                        let value = self.evaluate_node_deep(operand, Some(block));
                        if self.nodes[operand].parameterized_deep.unwrap() {
                            Value::Parameterized
                        } else {
                            value
                        }
                    }
                    None => Value::None,
                };
                let Module { nodes, blocks, .. } = self;
                ext.run(operand, &mut blocks[block], nodes)
            }
            Operator::Apply => {
                let Some(operands) = operation.operand else {
                    unreachable!("Call expects an operand array node")
                };
                // A marker target — the body's own parameter during the
                // definition pass — stays lazy instead of panicking.
                match self.evaluate_node(operands, Some(block)) {
                    Value::Parameterized => Value::Parameterized,
                    Value::Array(ptr) => {
                        let operands = unsafe { &*ptr };
                        match self.evaluate_node(operands[0], Some(block)) {
                            Value::Parameterized => Value::Parameterized,
                            Value::Function(function) => {
                                self.function_apply(function, operands[1], block)
                            }
                            _ => unreachable!("Call target must be a function value"),
                        }
                    }
                    _ => unreachable!("Call operand must be an array of [function, argument]"),
                }
            }
        };
        self.nodes[node].value = Some(value);
        self.nodes[node].visiting = false;
        value
    }

    /// Run [`Self::evaluate_node`] for all nodes in the reachable subtree of `id`.
    #[stacksafe]
    pub fn evaluate_node_deep(&mut self, node: NodeId, current: Option<BlockId>) -> Value<P> {
        self.deep_depth += 1;
        if self.deep_depth > self.deep_depth_limit {
            panic!(
                "recursion depth exceeded in deep evaluation (limit {}) — non-terminating evaluation?",
                self.deep_depth_limit
            );
        }
        let value = self.evaluate_node(node, current);
        if let Value::Array(array) = value {
            let block = self.nodes[node].block;
            for &id in unsafe { &*array } {
                self.evaluate_node_deep(id, Some(block));
            }
        }
        let parameterized = matches!(value, Value::Parameterized)
            || matches!(
                value,
                Value::Array(array)
                    if unsafe { &*array }.iter().any(|&id| self.nodes[id].parameterized_deep == Some(true))
            )
            || self.nodes[node].operation.is_some_and(|op| {
                op.operand.is_some_and(|operand| {
                    // Operands are static graph edges, not value-reachable, so a
                    // nested block release may have dropped the node by now.
                    self.nodes
                        .get(operand)
                        .is_some_and(|node| node.parameterized_deep == Some(true))
                })
            });
        self.nodes[node].parameterized_deep = Some(parameterized);
        self.deep_depth -= 1;
        value
    }

    fn evaluate_block(&mut self, block: BlockId, root: NodeId) -> Value<P> {
        debug_assert_eq!(self.nodes[root].block, block);
        self.evaluate_node_deep(root, None);
        let value = self.move_to_parent(root).expect("evaluated return node");
        self.release_block(block);
        value
    }

    /// Copy `nodes` into `block`'s arena and return the new `nodes`.
    fn copy_nodes(&self, nodes: &[NodeId], block: BlockId) -> *const [NodeId] {
        let slice = self.blocks[block].arena.alloc_slice_copy(nodes);
        ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
    }

    /// Copy `ext`'s payload bytes into `arena` and re-point it there,
    /// preserving the payload's alignment.
    fn copy_ext(&self, mut ext: P::Value, block: BlockId) -> Value<P> {
        let arena = &self.blocks[block].arena;
        if !ext.is_ptr() {
            return Value::Ext(ext);
        }
        let old = ext.ptr();
        let layout = Layout::from_size_align(old.len(), P::Value::alignment()).unwrap();
        let dst = arena.alloc_layout(layout);
        unsafe { ptr::copy_nonoverlapping(old as *const u8, dst.as_ptr(), old.len()) };
        ext.set_ptr(ptr::slice_from_raw_parts(dst.as_ptr(), old.len()));
        Value::Ext(ext)
    }

    /// Move a node into its parent block: re-point it, relocate any arena
    /// data it carries (array slices, Ext payloads) into the parent's
    /// arena, and return the relocated value.
    ///
    /// The node may not hold a value yet (a static function template is
    /// unevaluated) — in that case only the block mapping is updated and
    /// `None` is returned.  Value edges are followed like the original
    /// contract: array elements and function scope members that still live
    /// in `block` move with it, so a function's template survives the
    /// closing block exactly like an array's elements.  A function value
    /// itself is a [`FunctionId`] into the module's slotmap and needs no
    /// relocation; its scope outlives the block through the moved nodes,
    /// and it is dropped only when its home node eventually is.
    #[stacksafe]
    fn move_to_parent(&mut self, node: NodeId) -> Option<Value<P>> {
        let block = self.nodes[node].block;
        let parent = self.blocks[block].parent;
        let Some(parent) = parent else {
            return self.nodes[node].value;
        };
        self.nodes[node].block = parent;
        self.blocks[parent].nodes.push(node);
        let current = self.nodes[node].value;
        let value = current.map(|value| match value {
            Value::Array(array) => {
                let nodes = unsafe { &*array };
                for &node in nodes {
                    if self.nodes[node].block == block {
                        self.move_to_parent(node);
                    }
                }
                Value::Array(self.copy_nodes(nodes, parent))
            }
            Value::Function(function) => {
                // The template's nodes must outlive the closing block, so
                // the scope is mapped like an array slice: each member that
                // lives here moves into the parent.  The function itself is
                // homed like a node — if it lives in the closing block it
                // is re-pointed to the parent and registered there, so
                // release skips it and it stays callable.
                let ids = self.functions[function].nodes.clone();
                for &id in &ids {
                    if self.nodes[id].block == block {
                        self.move_to_parent(id);
                    }
                }
                if self.functions[function].block == block {
                    self.functions[function].block = parent;
                    self.blocks[parent].functions.push(function);
                }
                Value::Function(function)
            }
            Value::Ext(ext) => Self::copy_ext(self, ext, parent),
            value => value,
        });
        self.nodes[node].value = value;
        value
    }

    #[stacksafe]
    fn release_block(&mut self, block: BlockId) {
        let children = std::mem::take(&mut self.blocks[block].children);
        for child in children {
            if self.blocks.contains_key(child) {
                self.release_block(child);
            }
        }
        let functions = std::mem::take(&mut self.blocks[block].functions);
        for function in functions {
            // A function homed in this block is dropped with it: removing it
            // releases the function's scope.  Functions re-pointed to the
            // parent by compaction stay, still callable — the stale id in
            // this list is skipped, like a moved node's.
            if self
                .functions
                .get(function)
                .is_some_and(|function| function.block == block)
            {
                self.functions.remove(function);
            }
        }
        let nodes = std::mem::take(&mut self.blocks[block].nodes);
        for node in nodes {
            if self.nodes.get(node).is_some_and(|node| node.block == block) {
                self.nodes.remove(node);
            }
        }
        self.blocks.remove(block);
    }
}

/// The fixed context of one clone pass: where the clones land, what the
/// parameter maps onto, and the template's membership set plus the running
/// node-id remap.
struct ApplyCtx<'a> {
    target: BlockId,
    param: NodeId,
    argument: NodeId,
    members: &'a HashSet<NodeId>,
    remap: &'a mut HashMap<NodeId, NodeId>,
}

impl<P:Program> Module<P>{
    fn function_apply(
        &mut self,
        function: FunctionId,
        argument: NodeId,
        block: BlockId,
    ) -> Value<P> {
        self.apply_depth += 1;
        if self.apply_depth > self.apply_depth_limit {
            panic!(
                "recursion depth exceeded in function application (limit {}) — non-terminating function application?",
                self.apply_depth_limit
            );
        }
        let (r#return, parameter) = {
            let function = &self.functions[function];
            (function.r#return, function.parameter)
        };
        debug_assert!(self.functions[function].nodes.contains(&r#return));
        debug_assert!(self.functions[function].nodes.contains(&parameter));
        let members: HashSet<NodeId> = self.functions[function].nodes.iter().copied().collect();
        let mut remap = HashMap::new();
        let mut ctx = ApplyCtx {
            target: block,
            param: parameter,
            argument,
            members: &members,
            remap: &mut remap,
        };
        let applied = self.node_apply(r#return, &mut ctx);
        let result = self.evaluate_node(applied, Some(block));
        self.apply_depth -= 1;
        result
    }

    #[stacksafe]
    fn node_apply(&mut self, node: NodeId, ctx: &mut ApplyCtx<'_>) -> NodeId {
        if node == ctx.param {
            return ctx.argument;
        }
        if let Some(&clone) = ctx.remap.get(&node) {
            return clone;
        }
        if !ctx.members.contains(&node) {
            return node; // outside the template scope — reference as-is
        }
        // The body always exists, so only the parts that depend on the
        // parameter need fresh nodes: unevaluated operation nodes (their
        // operand edges must be rewritten) and nodes not proven concrete —
        // flagged parameterized nodes, plus nodes whose dependence was
        // never resolved (the Index/Apply arms read operands shallowly, so
        // those flags may still be `None`).  A node whose subtree evaluated
        // to a concrete value (`Some(false)`) is baked — reference it in
        // place.
        let (value, operation, parameterized_deep) = {
            let source = &self.nodes[node];
            (source.value, source.operation, source.parameterized_deep)
        };
        let depends_on_parameter =
            (value.is_none() && operation.is_some()) || parameterized_deep != Some(false);
        if !depends_on_parameter {
            return node;
        }
        // Reserve the clone id before recursing so diamonds resolve to one
        // clone and value cycles to the clone's own (still evaluating) id.
        let clone = self.add_node(ctx.target, None, None);
        ctx.remap.insert(node, clone);
        // A cached value on an operation node was computed against the
        // body's parameter and is stale once the argument is mapped in, so
        // such clones are left unevaluated — the kept operand chain
        // recomputes against the argument.  Constant nodes (no operation)
        // carry their remapped value.
        let value = if operation.is_some() {
            None
        } else {
            value.map(|value| self.value_apply(value, ctx))
        };
        let operation = operation.map(|operation| Operation {
            operand: operation
                .operand
                .map(|operand| self.node_apply(operand, ctx)),
            ..operation
        });
        self.nodes[clone].value = value;
        self.nodes[clone].operation = operation;
        clone
    }

    #[stacksafe]
    fn value_apply(&mut self, value: Value<P>, ctx: &mut ApplyCtx<'_>) -> Value<P> {
        match value {
            Value::Array(array) => {
                let nodes: Vec<NodeId> = unsafe { &*array }
                    .iter()
                    .map(|&id| self.node_apply(id, ctx))
                    .collect();
                Value::Array(self.copy_nodes(&nodes,ctx.target))
            }
            Value::Function(function) => {
                // A cloned function's scope is mapped like an array: every
                // member and both entry points are cloned into the target,
                // and the result is a fresh function homed on the target
                // block, so it is dropped with it.
                let (scope, r#return, parameter) = {
                    let function = &self.functions[function];
                    (
                        function.nodes.clone(),
                        function.r#return,
                        function.parameter,
                    )
                };
                let nodes: Vec<NodeId> = scope.iter().map(|&id| self.node_apply(id, ctx)).collect();
                let r#return = self.node_apply(r#return, ctx);
                let parameter = self.node_apply(parameter, ctx);
                let function = self.functions.insert(Function {
                    nodes,
                    r#return,
                    parameter,
                    block: ctx.target,
                });
                self.blocks[ctx.target].functions.push(function);
                Value::Function(function)
            }
            Value::Ext(ext) => Self::copy_ext(self, ext, ctx.target),
            value => value,
        }
    }
}

impl<P:Program> Module<P>{
    pub fn add_equality(&mut self, a: NodeId, b: NodeId) -> NodeId {
        disjoint::union(&mut self.nodes, a, b)
    }

    pub fn equality_representative(&mut self, node: NodeId) -> NodeId {
        disjoint::find(&mut self.nodes, node)
    }
}
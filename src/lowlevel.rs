use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use std::fmt;

use crate::utils::disjoint::{self};

mod equality;
mod evaluation;
mod function;
mod gc;
mod utils;

pub trait Program: Sized + Copy {
    type Value: ValueExt;
    type Operator: OperatorExt<Self>;
}

/// Extension values are program-specific; [`PartialEq`] is what decides
/// whether two of them unify.
pub trait ValueExt: Copy + PartialEq {
    fn is_ptr(&self) -> bool;
    /// Available if [`Self::is_ptr()`].
    fn ptr(&self) -> *const [u8] {
        unreachable!()
    }
    /// Available if [`Self::is_ptr()`].
    fn set_ptr(&mut self, _ptr: *const [u8]) {
        unreachable!()
    }
    /// Available if [`Self::is_ptr()`].
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

/// Equality is what decides whether two concrete values merge in
/// [`Module::unify`]; arrays are exempt there (they unify elementwise), so
/// they only compare by address here.
impl<P: Program> PartialEq for Value<P> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Ext(a), Value::Ext(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => std::ptr::eq(a, b),
            (Value::Function(a), Value::Function(b)) => a == b,
            (Value::USize(a), Value::USize(b)) => a == b,
            (Value::None, Value::None) | (Value::Parameterized, Value::Parameterized) => true,
            _ => false,
        }
    }
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

/// Garbage collection unit.
/// # Contract
/// - Only one node can be referenced from parent block
/// - Referencing a node whose block was released is a panic
pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
    /// Functions homed in this block, registered like nodes so
    /// [`Module::release_block`] drops them (and their scopes) with it.
    pub functions: Vec<FunctionId>,
}

/// # Contract:
/// Only [`Self::nodes`] can reference [`Self::parameter`].
#[derive(Clone, Debug)]
pub struct Function {
    /// including [`Self::r#return`] and [`Self::parameter`].
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
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Any node in self's reachable subtree has a [`Value::Parameterized`].
    /// Is [`Some`] only if having run by [`Module::evaluate_node_deep`].   
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

/// A failed unification between two value classes.
///
/// `a` and `b` are the representatives of the two classes that could not
/// be unified; [`Self::value_a`] and [`Self::value_b`] are the values that
/// conflicted.  The lowlevel only records facts — the high-level checker
/// reads these from [`Module::unify_errors`] to build diagnostics.
#[derive(Clone, Copy)]
pub struct UnifyError<P: Program> {
    pub a: NodeId,
    pub b: NodeId,
    pub value_a: Option<Value<P>>,
    pub value_b: Option<Value<P>>,
}

// --- Debug (dev aid) ---------------------------------------------------
//
// `Value` holds raw arena pointers, so these are hand-written: they read
// through the pointers (the arena outlives any debug print) and elide the
// program-specific `Ext` payload to a byte count.

impl<P: Program> fmt::Debug for Value<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Ext(ext) => {
                if ext.is_ptr() {
                    write!(f, "Ext({} bytes)", unsafe { &*ext.ptr() }.len())
                } else {
                    write!(f, "Ext(inline)")
                }
            }
            Value::Array(ptr) => write!(f, "Array({ptr:?})"),
            Value::Function(id) => write!(f, "Function({id:?})"),
            Value::USize(n) => write!(f, "USize({n})"),
            Value::None => write!(f, "None"),
            Value::Parameterized => write!(f, "Parameterized"),
        }
    }
}

impl<P: Program> fmt::Debug for Operator<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Operator::Ext(_) => write!(f, "Ext"),
            Operator::Index => write!(f, "Index"),
            Operator::Apply => write!(f, "Apply"),
        }
    }
}

impl<P: Program> fmt::Debug for Operation<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Operation")
            .field("operator", &self.operator)
            .field("operand", &self.operand)
            .finish()
    }
}

impl fmt::Debug for Block {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("parent", &self.parent)
            .field("children", &self.children)
            .field("nodes", &self.nodes)
            .field("functions", &self.functions)
            .finish()
    }
}

impl<P: Program> fmt::Debug for Node<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("value", &self.value)
            .field("operation", &self.operation)
            .field("block", &self.block)
            .field("visiting", &self.visiting)
            .field("parameterized_deep", &self.parameterized_deep)
            .field("equality", &self.equality)
            .finish()
    }
}

impl<P: Program> fmt::Debug for UnifyError<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnifyError")
            .field("a", &self.a)
            .field("b", &self.b)
            .field("value_a", &self.value_a)
            .field("value_b", &self.value_b)
            .finish()
    }
}

impl<P: Program> fmt::Debug for Module<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Module")
            .field("nodes", &self.nodes)
            .field("blocks", &self.blocks)
            .field("functions", &self.functions)
            .field("unify_errors", &self.unify_errors)
            .finish_non_exhaustive()
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
    pub evaluate_depth_limit: usize,
    /// Failed unifications collected by [`Self::unify`]; the checker drains
    /// this to build diagnostics.  A failed unify leaves the two classes
    /// unmerged.
    pub unify_errors: Vec<UnifyError<P>>,
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
            evaluate_depth_limit: Self::MAX_DEEP_DEPTH,
            unify_errors: Vec::new(),
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
}

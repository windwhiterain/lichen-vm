use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use std::collections::HashSet;
use std::fmt::Debug;

use lichen_utils::disjoint::{self};
use lichen_utils::extend::AsEnum;

pub use crate::assert::AssertError;
pub use crate::equality::UnifyError;
pub use crate::evaluation::EvalError;

mod assert;
mod equality;
mod evaluation;
mod function;
mod gc;
mod utils;

pub trait Program: Sized + Copy + Debug + PartialEq {
    /// The program's full value vocabulary: the structural [`LowValue`]
    /// spliced together with the program's own value variants.  The
    /// lowlevel reads and builds structural values through
    /// [`AsEnum::as_enum`] and [`From<LowValue>`]; the program's own
    /// variants are opaque to it.
    type Value: ValueExt + From<LowValue> + AsEnum<LowValue> + Clone;
    /// The program's full operator vocabulary: the structural [`LowOperator`]
    /// spliced together with the program's own operator variants.  The
    /// lowlevel dispatches structural operators through [`AsEnum::as_enum`];
    /// everything else falls through to [`OperatorExt::run`].
    type Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>;
    /// Program-global extension state, stored on [`Module`] and read or
    /// mutated by extension operators — the highlevel's fresh-type-id
    /// counter, for example.
    type GlobalExt: Debug + Copy + PartialEq + Default;
}

/// The structural values the lowlevel itself produces and consumes — the
/// non-extension subset of the former `Value<P>`.  A program's value type is
/// this enum extended with the program's own variants via the generated
/// `extend_LowValue!` carrier (see the `lichen-extend` crate), so the
/// lowlevel can always inspect a value through [`AsEnum::as_enum`] and
/// build one through [`From<LowValue>`] without naming the extension part.
/// A structural array value: the element ids plus an optional per-position
/// shallow mask.  `shallow` is null when no position is marked, otherwise a
/// `[bool]` in the same arena as `ids` — one entry per position, `true` =
/// the position's whole subtree is shallow.  The mask is inert metadata:
/// structure and unification ignore it, but it travels with the ids through
/// GC and apply clones, and [`Module::evaluate_node_deep`] skips the subtree
/// of a marked position, so the element stays lazy until a read forces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrayRef {
    pub ids: *const [NodeId],
    pub shallow: *const [bool],
}

impl ArrayRef {
    /// An unmasked array (`shallow` is null).
    pub fn new(ids: *const [NodeId]) -> Self {
        ArrayRef {
            ids,
            shallow: null_mask(),
        }
    }
    /// The element ids — valid for as long as the array's home block is
    /// alive (the caller's existing safety contract for arena payloads).
    pub fn ids(&self) -> &'static [NodeId] {
        unsafe { &*self.ids }
    }
    /// The shallow mask — empty when the array is unmasked.
    pub fn mask(&self) -> &'static [bool] {
        if self.shallow.is_null() {
            &[]
        } else {
            unsafe { &*self.shallow }
        }
    }
    /// Whether position `index` is marked shallow.
    pub fn is_shallow(&self, index: usize) -> bool {
        self.mask().get(index).copied() == Some(true)
    }
    /// Whether any position is marked shallow.
    pub fn has_shallow(&self) -> bool {
        self.mask().iter().any(|&marked| marked)
    }
}

/// The null form of a `*const [bool]` mask (a fat pointer — `ptr::null`
/// itself requires a thin type).
fn null_mask() -> *const [bool] {
    std::ptr::slice_from_raw_parts(std::ptr::null(), 0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[lichen_extend::enum_ext]
pub enum LowValue {
    USize(usize),
    Array(ArrayRef),
    Function(FunctionId),
    None,
    Parameterized,
}

/// The structural operators the lowlevel itself dispatches — the
/// non-extension subset of the former `Operator<P>`.  A program's operator
/// type is this enum extended with the program's own variants via the
/// generated `extend_LowOperator!` carrier, so the lowlevel can always pick
/// its own operators out of a value through [`AsEnum::as_enum`]; everything
/// `as_enum` doesn't recognise is a program operator and runs through
/// [`OperatorExt::run`].
#[derive(Debug, Clone, Copy, PartialEq)]
#[lichen_extend::enum_ext]
pub enum LowOperator {
    /// - `operand[0]`: array.
    /// - `operand[1]`: index.
    Index,
    /// - `operand[0]`: function.
    /// - `operand[1]`: argument.
    Apply,
}

/// [`PartialEq`] is what decides whether two of them unify.
pub trait ValueExt: Debug + Copy + PartialEq {
    fn is_handle(&self) -> bool;
    /// Available if [`Self::is_handle()`].
    fn handle(&self) -> Handle<[u8]> {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn set_handle(&mut self, _payload: Handle<[u8]>) {
        unreachable!()
    }
    /// Available if [`Self::is_handle()`].
    fn alignment() -> usize {
        unreachable!()
    }
}

pub trait OperatorExt<P: Program>: Debug + Copy {
    fn run(&self, operand: P::Value, block: BlockId, module: &mut Module<P>) -> P::Value;
}

#[derive(Debug, Clone, Copy)]
pub struct Operation<P: Program> {
    pub operator: P::Operator,
    pub operand: Option<NodeId>,
}

/// A class is unbound while it carries no value or only the lazy marker.
/// The highlevel checker uses the same rule for its diagnostics.
pub fn is_unbound(value: Option<impl AsEnum<LowValue>>) -> bool {
    value.is_none_or(|value| value.as_enum() == Some(LowValue::Parameterized))
}

/// Pointer into a [`Block::arena`].  
/// `PartialEq` compares pointing value if not `UNIQUE`.
#[derive(Debug)]
pub struct Handle<T: ?Sized, const UNIQUE: bool = false>(pub *const T);
impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for Handle<T> {}

impl<T: PartialEq + ?Sized, const UNIQUE: bool> PartialEq for Handle<T, UNIQUE> {
    fn eq(&self, other: &Self) -> bool {
        if std::ptr::eq(self.0, other.0) {
            return true;
        }
        if !UNIQUE {
            unsafe { *self.0 == *other.0 }
        } else {
            false
        }
    }
}

impl Handle<[u8]> {
    pub fn len(&self) -> usize {
        let slice = unsafe { &*self.0 };
        slice.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

new_key_type! {pub struct NodeId;}
new_key_type! {pub struct BlockId;}
new_key_type! {pub struct FunctionId;}

/// Garbage collection unit.
/// # Contract
/// - Only one node can be referenced from parent block
/// - Referencing a node whose block was released is a panic
#[derive(Debug)]
pub struct Block {
    pub arena: Bump,
    pub parent: Option<BlockId>,
    pub children: Vec<BlockId>,
    pub nodes: Vec<NodeId>,
    /// Functions homed in this block, registered like nodes so garbage
    /// collection ([`Module::garbage_collect`]) drops them (and their
    /// scopes) with it.
    pub functions: Vec<FunctionId>,
}

/// # Contract:
/// Only [`Self::nodes`] can reference [`Self::parameter`].
#[derive(Debug, Clone)]
pub struct Function {
    /// A membership set — the template scope — including the `r#return`
    /// and [`Self::parameter`] entry points.  Stored as a set because the
    /// clone pass only asks "is this node part of the scope"; no code reads
    /// the members positionally.
    pub nodes: HashSet<NodeId>,
    pub r#return: NodeId,
    pub parameter: NodeId,
    /// Owner.
    pub block: BlockId,
}

#[derive(Debug)]
pub struct Node<P: Program> {
    pub value: Option<P::Value>,
    pub operation: Option<Operation<P>>,
    /// The condition node of an assert, when this node is an assert point —
    /// an explicit constraint (the checker force-evaluates the condition,
    /// ignoring laziness, and requires `USize(1)`; see
    /// [`Module::check_asserts`]).  The point is a cell-shaped side node
    /// (no operation, the lazy marker as value) — nothing in the value flow
    /// references it, but it rides the apply clone and garbage collection
    /// like any scope member, and its condition is remapped through the
    /// clone so a function body's assert re-checks against each call's
    /// argument.
    pub assert: Option<NodeId>,
    /// Owner.
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Any node in self's reachable subtree has a [`LowValue::Parameterized`].
    /// Is [`Some`] only if having run by [`Module::evaluate_node_deep`].   
    pub parameterized_deep: Option<bool>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::equality_representative`].
    pub equality: disjoint::Meta<NodeId>,
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
    /// Total-application guard: a run panics when the *cumulative* number
    /// of function applications exceeds this — the lazy graph flattens most
    /// recursion (an apply returns its result pair and the outer deep pass
    /// descends into it, so nested depth stays 1 even for an infinite loop
    /// behind a lazy branch, and a wide recursion like fib is never deep at
    /// all), so nested depth alone cannot bound the work.  The total count
    /// bounds both.  Defaults to [`Self::MAX_APPLY_TOTAL`]; tests lower it
    /// to panic fast.
    pub apply_total_limit: usize,
    /// Deep-evaluation guard: a run panics when [`Self::evaluate_node_deep`]
    /// nests deeper than this (deep-evaluating an infinitely growing value,
    /// e.g. `f(x) = [x, f(x)]`).  Defaults to [`Self::MAX_DEEP_DEPTH`],
    /// which sits above the legitimately ~200k-deep block chains exercised
    /// by the `#[stacksafe]` tests; tests lower it to panic fast.
    pub evaluate_depth_limit: usize,
    pub unify_errors: Vec<UnifyError<P>>,
    /// Runtime evaluation failures (an out-of-bounds [`LowOperator::Index`]),
    /// recorded instead of panicking — same append-only, never-cleared
    /// contract as [`Self::unify_errors`].
    pub eval_errors: Vec<EvalError>,
    /// The module's assert points — the nodes with [`Node::assert`] set,
    /// originals and apply clones alike (the clone pass appends, so the
    /// check pass sees every instantiated assert).  Garbage collection
    /// prunes the entries of dropped blocks.  The checker walks this in
    /// [`Module::check_asserts`].
    pub asserts: Vec<NodeId>,
    /// Failed asserts: a condition that resolved to a concrete value other
    /// than `USize(1)`.  An assert whose condition stays lazy (an unbound
    /// parameter) is not triggered and records nothing.  Same append-only,
    /// never-cleared contract as [`Self::unify_errors`].
    pub assert_errors: Vec<AssertError<P>>,
    /// Program-global extension state — see [`Program::GlobalExt`].
    pub global_ext: P::GlobalExt,
    apply_depth: usize,
    apply_total: usize,
    deep_depth: usize,
}

impl<P: Program> Default for Module<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<P: Program> Module<P> {
    pub const MAX_APPLY_DEPTH: usize = 10_000;
    /// The default total-application budget.  Each application clones its
    /// function's body (~tens of nodes), so this also bounds the module's
    /// growth — an indeterminate recursion stops before it drains memory.
    pub const MAX_APPLY_TOTAL: usize = 100_000;
    pub const MAX_DEEP_DEPTH: usize = 300_000;

    pub fn new() -> Self {
        Module {
            nodes: SlotMap::with_key(),
            blocks: SlotMap::with_key(),
            functions: SlotMap::with_key(),
            apply_depth_limit: Self::MAX_APPLY_DEPTH,
            apply_total_limit: Self::MAX_APPLY_TOTAL,
            evaluate_depth_limit: Self::MAX_DEEP_DEPTH,
            unify_errors: Vec::new(),
            eval_errors: Vec::new(),
            asserts: Vec::new(),
            assert_errors: Vec::new(),
            global_ext: P::GlobalExt::default(),
            apply_depth: 0,
            apply_total: 0,
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
        value: Option<P::Value>,
    ) -> NodeId {
        let node = self.nodes.insert(Node {
            value,
            operation,
            assert: None,
            block,
            visiting: false,
            parameterized_deep: None,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        node
    }

    /// An assert point: a node naming `condition` as an explicit constraint
    /// — not a unification, so an unbound condition is *not* bound to `1`,
    /// it stays untriggered until an apply binds it.  The checker
    /// force-evaluates every registered point's condition (ignoring
    /// laziness) and requires `USize(1)`, see [`Self::check_asserts`].  The
    /// point is a cell-shaped side node registered in [`Self::asserts`]; the
    /// apply clone remaps its condition and re-registers the clone, so a
    /// function body's assert re-checks against each call's argument.
    pub fn add_assert(&mut self, block: BlockId, condition: NodeId) -> NodeId {
        let node = self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Parameterized)),
        );
        self.nodes[node].assert = Some(condition);
        self.asserts.push(node);
        node
    }

    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: impl IntoIterator<Item = NodeId>,
    ) -> NodeId {
        let function = self.functions.insert(Function {
            nodes: nodes.into_iter().collect(),
            r#return: ret,
            parameter: param,
            block,
        });
        self.blocks[block].functions.push(function);
        self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Function(function))),
        )
    }
}

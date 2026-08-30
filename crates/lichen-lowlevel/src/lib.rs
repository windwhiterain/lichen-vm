use bumpalo::Bump;
use slotmap::{SlotMap, new_key_type};
use std::fmt::Debug;
use std::marker::PhantomData;

use lichen_utils::disjoint::{self};
use lichen_utils::extend::AsEnum;

pub use crate::assert::AssertError;
pub use crate::equality::UnifyError;
pub use crate::evaluation::EvalError;
pub use crate::function::ApplyError;

mod assert;
mod equality;
mod evaluation;
mod function;
mod gc;
mod utils;

pub trait Program: Sized + Copy + Debug + PartialEq {
    /// The program's full value vocabulary: the program's own value union
    /// with the structural [`LowValue`] carried whole as one variant
    /// (composed by [`lichen_utils::enum_ext!`] — see [`LowValue`]).  The
    /// lowlevel reads and builds structural values through
    /// [`AsEnum::as_enum`] and [`From<LowValue>`]; the program's own
    /// variants are opaque to it.
    type Value: ValueExt + From<LowValue> + AsEnum<LowValue> + Clone;
    /// The program's full operator vocabulary: the program's own operator
    /// union with the structural [`LowOperator`] carried whole as one
    /// variant (composed the same way — see [`LowOperator`]).  The
    /// lowlevel dispatches structural operators through [`AsEnum::as_enum`];
    /// everything else falls through to [`OperatorExt::run`].
    type Operator: OperatorExt<Self> + From<LowOperator> + AsEnum<LowOperator>;
    /// Program-global extension state, stored on [`Module`] and read or
    /// mutated by extension operators — the highlevel's fresh-type-id
    /// counter, for example.  A concrete `GlobalExt` is a host struct
    /// composed of component states via [`lichen_utils::compose_ext!`], each
    /// component reached through [`lichen_utils::compose::AsField`] and its
    /// own inherent methods; the lowlevel only requires the marker
    /// [`GlobalExt`] trait.
    type GlobalExt: GlobalExt;
}

/// One element of a structural array value: the element's node plus its
/// shallow marker.  `shallow` is inert metadata — structure and unification
/// ignore it — but it travels with the node through GC and apply clones, and
/// [`Module::evaluate_node_deep`] skips the subtree of a marked position, so
/// the element stays lazy until a read forces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArrayItem {
    pub node: NodeId,
    pub shallow: bool,
}

impl ArrayItem {
    /// An unmarked element (`shallow` is false).
    pub fn new(node: NodeId) -> Self {
        ArrayItem {
            node,
            shallow: false,
        }
    }
}

/// Program-global extension state — the marker trait that stances the
/// `Program::GlobalExt` bound.
///
/// The lowlevel only ever *initialises* this state ([`Module::new`] calls
/// `P::GlobalExt::default()`); it never copies, compares, or formats it, so a
/// `GlobalExt` needs nothing beyond [`Default`] — `Debug`/`Copy`/`PartialEq`
/// are not required, so the concrete state's components need not be.  The
/// concrete state explicitly implements this marker (opt-in, no blanket impl):
/// it is composed downstream from component states with
/// [`lichen_utils::compose_ext!`] (which generates
/// [`lichen_utils::compose::AsField`] accessors per component; a component's
/// behaviour lives as its own inherent methods), then `impl GlobalExt for ..`.
pub trait GlobalExt: Default {}

/// The structural values the lowlevel itself produces and consumes — the
/// non-extension subset of the former `Value<P>`.  A program's value type
/// composes this enum with [`lichen_utils::enum_ext!`] — `+ LowValue;`
/// carries it whole as one variant named `LowValue` and bakes the
/// `From<LowValue>`/`AsEnum<LowValue>` pair the [`Program::Value`] contract
/// requires — so the lowlevel can always inspect a value through
/// [`AsEnum::as_enum`] and build one through [`From<LowValue>`] without
/// naming the program's part.  A chain layer further up (the highlevel's
/// vocabulary, a language crate's) lists its whole ancestry in one
/// invocation: `+ HighProgramValue as HighProgramValue; + LowValue;` — the
/// root glue generates through the carried layer.
///
/// A structural array value: the element [`ArrayItem`]s behind a
/// [`Handle`] into the array's home block's arena.  An element's `shallow`
/// flag is inert metadata: structure and unification ignore it, but it
/// travels with the element through GC and apply clones, and
/// [`Module::evaluate_node_deep`] skips the subtree of a marked element, so
/// the element stays lazy until a read forces it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LowValue {
    USize(usize),
    Array(Handle<[ArrayItem]>),
    Function(FunctionId),
    None,
    Parameterized,
}

impl Handle<[ArrayItem]> {
    /// The array's items — valid for as long as the array's home block is
    /// alive (the caller's existing safety contract for arena payloads).
    pub fn items(&self) -> &'static [ArrayItem] {
        unsafe { &*self.0 }
    }
}

/// The structural operators the lowlevel itself dispatches — the
/// non-extension subset of the former `Operator<P>`.  A program's operator
/// type composes this enum with [`lichen_utils::enum_ext!`] —
/// `+ LowOperator;` carries it whole as one variant named `LowOperator` and
/// bakes the `From<LowOperator>`/`AsEnum<LowOperator>` pair the
/// [`Program::Operator`] contract requires — so the lowlevel can always pick
/// its own operators out of a value through [`AsEnum::as_enum`]; everything
/// `as_enum` doesn't recognise is a program operator and runs through
/// [`OperatorExt::run`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LowOperator {
    /// - `operand[0]`: array.
    /// - `operand[1]`: index.
    Index,
    /// - `operand[0]`: function.
    /// - `operand[1]`: argument.
    Apply,
}

/// The cheap, structural equality a value vocabulary must provide —
/// marker/`USize` variants compare by their fields, handle payloads compare
/// by pointer identity ([`Handle`]'s [`PartialEq`]).  It decides the fast
/// checks (`is_unbound`, kind-marker lookups); the *full* equality
/// unification merges on is [`ValueExt::value_eq`], which compares handle
/// payloads by content.  Equality *through* arrays is not any `==`'s job —
/// unification recurses into them elementwise.
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
    /// Full equality of two values: handle payloads compare by content
    /// (same variant, byte-wise against the pointed-to allocation), every
    /// other pair is the derived [`PartialEq`].  This is the equality
    /// unification merges two concrete values on — [`PartialEq`] itself is
    /// only the cheap pointer-level form.  Not deep: an array is one
    /// allocation, so two arrays compare equal only when they share it;
    /// structural equality through arrays is unification's elementwise
    /// recursion.
    fn value_eq(&self, other: &Self) -> bool {
        if self.is_handle()
            && other.is_handle()
            && std::mem::discriminant(self) == std::mem::discriminant(other)
        {
            let (a, b) = (self.handle(), other.handle());
            return a.len() == b.len()
                && (std::ptr::eq(a.0, b.0)
                    || unsafe {
                        std::slice::from_raw_parts(a.0 as *const u8, a.len())
                            == std::slice::from_raw_parts(b.0 as *const u8, b.len())
                    });
        }
        self == other
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
/// `PartialEq` is pointer identity: two handles are equal iff they point at
/// the same allocation (same address and length) — never a dereference.
/// Content equality of two handle payloads is a value-level question,
/// answered by [`Module::value_eq`].
#[derive(Debug)]
pub struct Handle<T: ?Sized>(pub *const T);
pub struct StaticHandle<T: ?Sized> {
    pub module: usize,
    pub offset: *const T,
    _p: PhantomData<T>,
}

impl<T: ?Sized> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Clone for StaticHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T: ?Sized> Copy for Handle<T> {}
impl<T: ?Sized> Copy for StaticHandle<T> {}

impl<T: ?Sized> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0)
    }
}

impl<T: ?Sized> PartialEq for StaticHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.module == other.module && std::ptr::eq(self.offset, other.offset)
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

impl StaticHandle<[u8]> {
    pub fn len(&self) -> usize {
        let slice = unsafe { &*self.offset };
        slice.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

new_key_type! {pub struct NodeId;}
pub struct StaticNodeId {
    pub module: usize,
    pub index: usize,
}
new_key_type! {pub struct BlockId;}
new_key_type! {pub struct FunctionId;}
pub struct StaticFunctionId {
    pub module: usize,
    pub index: usize,
}

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
    /// The template scope — the nodes owned by this function's body
    /// (including the `r#return` and [`Self::parameter`] entry points),
    /// registered as they are compiled.  The clone pass's membership test
    /// does not consult this list directly: a node belongs to the template
    /// iff its [`Node::function`] chain (through [`Self::parent`]) reaches
    /// the applied function.  The list is the *starting set* of a closure
    /// clone and the garbage-collection root set, so it is iterated, never
    /// queried.
    pub nodes: Vec<NodeId>,
    pub r#return: NodeId,
    pub parameter: NodeId,
    /// The lexical parent — the function in whose body this function is
    /// nested (or [`None`] at top level).  The chain of these links makes
    /// the template membership test: a nested closure's nodes belong to an
    /// enclosing function's template because their owner's chain reaches
    /// it.  The link is *not* a keep-alive edge: garbage collection drops
    /// functions with their home block, never through this field.
    pub parent: Option<FunctionId>,
    /// The body's assert conditions — the function's own entries in
    /// [`Module::asserts`].  An apply clones every condition the deep pass
    /// did not prove concrete and registers the clones, so a body's assert
    /// that could not resolve at normalize re-checks the instantiated
    /// condition against each call's argument; a proven-concrete condition
    /// is per-call invariant (decided at normalize), so it is referenced in
    /// place and not re-registered.  Garbage collection moves the listed
    /// conditions with the function like any other edge.
    pub asserts: Vec<NodeId>,
    /// Owner.
    pub block: BlockId,
}

pub struct StaticFunction {
    pub parameter: StaticNodeId,
    pub r#return: StaticNodeId,
}

/// The outcome of the deep pass ([`Module::evaluate_node_deep`],
/// [`Module::evaluate_node_forced`]) on one node.  The deep pass records,
/// per node, whether it ran at all and, when it ran, whether the subtree it
/// covers is parameterized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvaluatedDeep {
    /// `true` when any node in self's reachable subtree has a
    /// [`LowValue::Parameterized`] — i.e. the deep pass could not prove the
    /// subtree concrete.
    pub parameterized: bool,
}

#[derive(Debug)]
pub struct Node<P: Program> {
    pub value: Option<P::Value>,
    pub operation: Option<Operation<P>>,
    /// The function whose body owns this node — the template membership
    /// back-pointer ([`None`] for top-level and runtime-created nodes whose
    /// owner is not a template).  The apply clone walk tests membership by
    /// walking this chain through [`Function::parent`]; clones carry the
    /// tag of the context that created them.
    pub function: Option<FunctionId>,
    /// Owner.
    pub block: BlockId,
    /// Detect circular recursion.
    pub visiting: bool,
    /// Whether the deep pass ([`Module::evaluate_node_deep`],
    /// [`Module::evaluate_node_forced`]) has run on this node, and what it
    /// proved.  [`Some`] means the deep pass ran and
    /// [`EvaluatedDeep::parameterized`] records whether any node in self's
    /// reachable subtree has a [`LowValue::Parameterized`].  [`None`] means
    /// it never ran, so the node's concreteness is unknown.
    pub evaluated_deep: Option<EvaluatedDeep>,
    /// Disjoint-set metadata for node equality classes, maintained by
    /// [`Module::add_equality`] and [`Module::equality_representative`].
    pub equality: disjoint::Meta<NodeId>,
}

pub struct StaticNode<P: Program> {
    pub value: Option<P::Value>,
    pub operation: Option<Operation<P>>,
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
    /// The module's assert worklist — the condition nodes registered by
    /// [`Self::add_assert`].  Spawn and every apply clone register here;
    /// [`Self::check_asserts`] drains it, consuming decided entries and
    /// leaving exactly the not-yet-triggered ones.  Garbage collection
    /// prunes the entries of dropped blocks.
    pub asserts: Vec<NodeId>,
    /// Failed asserts: a condition that resolved to a concrete value other
    /// than `USize(1)`.  An assert whose condition stays lazy (an unbound
    /// parameter) is not triggered and records nothing.  Same append-only,
    /// never-cleared contract as [`Self::unify_errors`].
    pub assert_errors: Vec<AssertError<P>>,
    /// Failed apply-time parameter checks: the context of each one (the
    /// declared parameter type, the argument type, and the apply node) so the
    /// highlevel can attribute the matching [`Self::unify_errors`] entries to
    /// the call site instead of the deep conflict leaves.  One entry per
    /// failed `function_apply`; the raw [`UnifyError`] entries it produced
    /// stay in [`Self::unify_errors`] alongside it.
    pub apply_errors: Vec<ApplyError>,
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
            apply_errors: Vec::new(),
            global_ext: P::GlobalExt::default(),
            apply_depth: 0,
            apply_total: 0,
            deep_depth: 0,
        }
    }

    /// Resets the per-run evaluation budgets ([`Self::apply_depth`],
    /// [`Self::apply_total`], [`Self::deep_depth`]) so a host can drive the
    /// module in a long-running loop (e.g. one kernel call per GUI frame)
    /// without the cumulative apply count exhausting
    /// [`Self::apply_total_limit`]. The budgets guard *one* run; a host that
    /// resets them per run keeps the guard while shedding lifetime
    /// accumulation. The limits themselves are unchanged.
    pub fn reset_apply_budget(&mut self) {
        self.apply_depth = 0;
        self.apply_total = 0;
        self.deep_depth = 0;
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
            function: None,
            block,
            visiting: false,
            evaluated_deep: None,
            equality: disjoint::Meta::default(),
        });
        disjoint::make_set(&mut self.nodes, node);
        self.blocks[block].nodes.push(node);
        node
    }

    /// Registers `condition` as an assert — an explicit constraint, not a
    /// unification, so an unbound condition is *not* bound to `1`, it stays
    /// untriggered until an apply binds it.  [`Self::check_asserts`]
    /// force-evaluates every registered condition (ignoring laziness) and
    /// requires `USize(1)`, see there.  The registry is a worklist; a
    /// condition owned by a function body ([`Function::asserts`]) is cloned
    /// and re-registered per apply, so a body's assert re-checks against
    /// each call's argument.
    pub fn add_assert(&mut self, condition: NodeId) -> NodeId {
        self.asserts.push(condition);
        condition
    }

    pub fn add_function(
        &mut self,
        block: BlockId,
        ret: NodeId,
        param: NodeId,
        nodes: impl IntoIterator<Item = NodeId>,
        asserts: impl IntoIterator<Item = NodeId>,
    ) -> NodeId {
        let nodes: Vec<NodeId> = nodes.into_iter().collect();
        let function = self.functions.insert(Function {
            nodes: Vec::new(),
            r#return: ret,
            parameter: param,
            parent: None,
            asserts: asserts.into_iter().collect(),
            block,
        });
        // The passed nodes are this function's template: tag each with its
        // owner, so the apply clone walk's chain membership test recognizes
        // them.  The function id must exist before the tags point at it.
        for &node in &nodes {
            self.nodes[node].function = Some(function);
        }
        self.functions[function].nodes = nodes;
        self.blocks[block].functions.push(function);
        // The value node is the function's own too — tagged with it, so an
        // enclosing template (a nested function's parent link) clones it and
        // instantiates a fresh closure per call instead of referencing the
        // template's function value in place.
        let func_node = self.add_node(
            block,
            None,
            Some(P::Value::from(LowValue::Function(function))),
        );
        self.nodes[func_node].function = Some(function);
        func_node
    }
}

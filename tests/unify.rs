//! Structural unification (`Module::unify`) over values, with failures
//! collected in `Module::unify_errors` instead of panicking.

use lichen_vm::{
    lowlevel::{
        Block, BlockId, Module, Node, NodeId, Operation, Operator, OperatorExt, Program, Value,
        ValueExt,
    },
    utils::disjoint,
};
use slotmap::SlotMap;

#[derive(Clone, Copy)]
struct TestProgram;

impl Program for TestProgram {
    type Value = TestValue;
    type Operator = TestOperator;
}

/// Extension values: a `u128` payload and a `char` slice, both arena-allocated.
#[derive(Clone, Copy)]
enum TestValue {
    U128(*const u128),
    String(*const [char]),
}

impl ValueExt for TestValue {
    fn is_ptr(&self) -> bool {
        true
    }
    fn alignment() -> usize {
        16
    }
    fn ptr(&self) -> *const [u8] {
        match self {
            TestValue::U128(p) => std::ptr::slice_from_raw_parts(*p as *const u8, 16),
            TestValue::String(p) => {
                let chars = unsafe { &**p };
                std::ptr::slice_from_raw_parts(chars.as_ptr() as *const u8, chars.len() * 4)
            }
        }
    }
    fn set_ptr(&mut self, ptr: *const [u8]) {
        match self {
            TestValue::U128(p) => *p = ptr as *const u128,
            TestValue::String(p) => {
                *p = std::ptr::slice_from_raw_parts(ptr as *const char, ptr.len() / 4)
            }
        }
    }
}

/// Value equality: same payload contents, not pointer identity.
impl PartialEq for TestValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TestValue::U128(a), TestValue::U128(b)) => unsafe { **a == **b },
            (TestValue::String(a), TestValue::String(b)) => unsafe { **a == **b },
            _ => false,
        }
    }
}

/// No operators are exercised; the type only exists to satisfy `Program`.
#[derive(Clone, Copy)]
enum TestOperator {}

impl OperatorExt<TestProgram> for TestOperator {
    fn run(
        &self,
        _operand: Value<TestProgram>,
        _block: &mut Block,
        _nodes: &SlotMap<NodeId, Node<TestProgram>>,
    ) -> Value<TestProgram> {
        match *self {}
    }
}

// --- builders ---------------------------------------------------------

fn u128_node(m: &mut Module<TestProgram>, block: BlockId, n: u128) -> NodeId {
    let p = m.blocks[block].arena.alloc(n);
    m.add_node(
        block,
        None,
        Some(Value::Ext(TestValue::U128(p as *const u128))),
    )
}

fn str_node(m: &mut Module<TestProgram>, block: BlockId, chars: &[char]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(chars);
    m.add_node(
        block,
        None,
        Some(Value::Ext(TestValue::String(
            std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
        ))),
    )
}

fn usize_node(m: &mut Module<TestProgram>, block: BlockId, n: usize) -> NodeId {
    m.add_node(block, None, Some(Value::USize(n)))
}

fn unit_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(Value::None))
}

fn array_node(m: &mut Module<TestProgram>, block: BlockId, ids: &[NodeId]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(ids);
    m.add_node(
        block,
        None,
        Some(Value::Array(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        ))),
    )
}

fn unbound_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(Value::Parameterized))
}

/// An `Apply` node with operand array `[function, argument]`.
fn apply_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    func: NodeId,
    arg: NodeId,
) -> NodeId {
    let operands = array_node(m, block, &[func, arg]);
    m.add_node(
        block,
        Some(Operation {
            operator: Operator::Apply,
            operand: Some(operands),
        }),
        None,
    )
}

fn u128_of(value: Value<TestProgram>) -> u128 {
    let Value::Ext(TestValue::U128(ptr)) = value else {
        panic!("expected U128")
    };
    unsafe { *ptr }
}

fn array_ids(value: Value<TestProgram>) -> Vec<NodeId> {
    let Value::Array(ptr) = value else {
        panic!("expected an array value")
    };
    unsafe { &*ptr }.to_vec()
}

// --- tests ------------------------------------------------------------

#[test]
fn unbound_binds_to_the_other_side() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let x = unbound_node(&mut m, block);
    let one = usize_node(&mut m, block, 1);
    let rep = m.unify(x, one);
    assert!(m.unify_errors.is_empty());
    assert!(matches!(m.nodes[rep].value, Some(Value::USize(1))));
    assert_eq!(m.equality_representative(x), m.equality_representative(one));
}

#[test]
fn two_unbound_nodes_unify_into_one_class() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unbound_node(&mut m, block);
    let b = unbound_node(&mut m, block);
    let t = unbound_node(&mut m, block);
    m.unify(a, t);
    m.unify(b, t);
    assert!(m.unify_errors.is_empty());
    let rep = m.equality_representative(a);
    assert_eq!(m.equality_representative(t), rep);
    assert_eq!(m.equality_representative(b), rep);
    // still unbound: the class carries no value yet
    assert!(is_unbound_value(m.nodes[rep].value));
}

#[test]
fn binding_one_member_binds_the_whole_class() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unbound_node(&mut m, block);
    let b = unbound_node(&mut m, block);
    let t = unbound_node(&mut m, block);
    m.unify(a, t);
    m.unify(b, t);
    // the `struct K<T>{a: T, b: T}; K<int, float>` shape: T binds to int...
    let int = usize_node(&mut m, block, 1);
    m.unify(t, int);
    assert!(m.unify_errors.is_empty());
    let rep = m.equality_representative(b);
    assert!(matches!(m.nodes[rep].value, Some(Value::USize(1))));
    // ...so the second field's instantiation with float conflicts
    let float = str_node(&mut m, block, &['f']);
    m.unify(b, float);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(b),
        m.equality_representative(float)
    );
    // an equal value still merges (the `K<int, int>` shape)
    let int_again = usize_node(&mut m, block, 1);
    m.unify(b, int_again);
    assert_eq!(m.unify_errors.len(), 1);
    assert_eq!(
        m.equality_representative(b),
        m.equality_representative(int_again)
    );
}

fn is_unbound_value(value: Option<Value<TestProgram>>) -> bool {
    matches!(value, None | Some(Value::Parameterized))
}

#[test]
fn equal_values_merge_and_unequal_values_conflict() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = usize_node(&mut m, block, 1);
    let same = usize_node(&mut m, block, 1);
    let different = usize_node(&mut m, block, 2);
    m.unify(a, same);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(a),
        m.equality_representative(same)
    );
    m.unify(a, different);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(a),
        m.equality_representative(different)
    );
}

#[test]
fn unit_values_merge() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unit_node(&mut m, block);
    let b = unit_node(&mut m, block);
    m.unify(a, b);
    assert!(m.unify_errors.is_empty());
    assert_eq!(m.equality_representative(a), m.equality_representative(b));
}

#[test]
fn conflicting_kinds_record_an_error_and_stay_separate() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = usize_node(&mut m, block, 1);
    let inner = usize_node(&mut m, block, 2);
    let b = array_node(&mut m, block, &[inner]);
    m.unify(a, b);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(m.equality_representative(a), m.equality_representative(b));
    let error = m.unify_errors[0];
    assert!(matches!(error.value_a, Some(Value::USize(1))));
    assert!(matches!(error.value_b, Some(Value::Array(_))));
}

#[test]
fn extension_values_merge_by_value_equality() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = u128_node(&mut m, block, 5);
    let same = u128_node(&mut m, block, 5);
    let other = u128_node(&mut m, block, 7);
    let s = str_node(&mut m, block, &['h', 'i']);
    m.unify(a, same);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(a),
        m.equality_representative(same)
    );
    m.unify(a, other);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(a),
        m.equality_representative(other)
    );
    m.unify(a, s);
    assert_eq!(m.unify_errors.len(), 2);
    assert_ne!(m.equality_representative(a), m.equality_representative(s));
}

#[test]
fn arrays_unify_elementwise() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let x = unbound_node(&mut m, block);
    let y = unbound_node(&mut m, block);
    let left = array_node(&mut m, block, &[x, y]);
    let one = usize_node(&mut m, block, 1);
    let two = usize_node(&mut m, block, 2);
    let right = array_node(&mut m, block, &[one, two]);
    m.unify(left, right);
    assert!(m.unify_errors.is_empty());
    let rep_x = m.equality_representative(x);
    let rep_y = m.equality_representative(y);
    assert!(matches!(m.nodes[rep_x].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[rep_y].value, Some(Value::USize(2))));
    assert_eq!(
        m.equality_representative(left),
        m.equality_representative(right)
    );
}

#[test]
fn array_length_mismatch_records_an_error() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = usize_node(&mut m, block, 1);
    let left = array_node(&mut m, block, &[a]);
    let b = usize_node(&mut m, block, 2);
    let c = usize_node(&mut m, block, 3);
    let right = array_node(&mut m, block, &[b, c]);
    m.unify(left, right);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(left),
        m.equality_representative(right)
    );
}

#[test]
fn array_element_conflict_records_an_error_without_merging_the_arrays() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let x = unbound_node(&mut m, block);
    let one = usize_node(&mut m, block, 1);
    let left = array_node(&mut m, block, &[x, one]);
    let two = usize_node(&mut m, block, 2);
    let s = str_node(&mut m, block, &['s']);
    let right = array_node(&mut m, block, &[two, s]);
    m.unify(left, right);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(left),
        m.equality_representative(right)
    );
    // the non-conflicting element still bound
    let rep_x = m.equality_representative(x);
    assert!(matches!(m.nodes[rep_x].value, Some(Value::USize(2))));
}

#[test]
fn same_function_value_merges() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let param = unbound_node(&mut m, block);
    let ret = usize_node(&mut m, block, 1);
    let f = m.add_function(block, ret, param, &[ret, param]);
    let fid = match m.nodes[f].value.unwrap() {
        Value::Function(fid) => fid,
        _ => unreachable!("function value node"),
    };
    let alias = m.add_node(block, None, Some(Value::Function(fid)));
    m.unify(f, alias);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(f),
        m.equality_representative(alias)
    );
}

#[test]
fn different_function_values_record_an_error() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let p1 = usize_node(&mut m, block, 1);
    let r1 = unbound_node(&mut m, block);
    let f1 = m.add_function(block, r1, p1, &[r1, p1]);
    let p2 = usize_node(&mut m, block, 2);
    let r2 = unbound_node(&mut m, block);
    let f2 = m.add_function(block, r2, p2, &[r2, p2]);
    m.unify(f1, f2);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(m.equality_representative(f1), m.equality_representative(f2));
}

#[test]
fn mutually_self_referential_arrays_record_an_error_instead_of_looping() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unbound_node(&mut m, block);
    let b = unbound_node(&mut m, block);
    let arr_a = array_node(&mut m, block, &[a]);
    let arr_b = array_node(&mut m, block, &[b]);
    let val_a = m.nodes[arr_a].value;
    let val_b = m.nodes[arr_b].value;
    m.nodes[a].value = val_a;
    m.nodes[b].value = val_b;
    m.unify(a, b);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(m.equality_representative(a), m.equality_representative(b));
}

#[test]
fn unifying_a_class_with_itself_is_a_noop() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = usize_node(&mut m, block, 1);
    let rep = m.unify(a, a);
    assert!(m.unify_errors.is_empty());
    assert_eq!(rep, a);
}

#[test]
fn multiple_conflicts_accumulate() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = usize_node(&mut m, block, 1);
    let s = str_node(&mut m, block, &['x']);
    m.unify(a, s);
    let u = unbound_node(&mut m, block);
    let arr = array_node(&mut m, block, &[u]);
    m.unify(a, arr);
    let two = usize_node(&mut m, block, 2);
    m.unify(a, two); // unequal concrete values: conflict
    assert_eq!(m.unify_errors.len(), 3);
}

// --- class value replication -------------------------------------------

#[test]
fn binding_reaches_every_member_of_the_class() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unbound_node(&mut m, block);
    let b = unbound_node(&mut m, block);
    let t = unbound_node(&mut m, block);
    m.unify(a, t);
    m.unify(b, t);
    let int = usize_node(&mut m, block, 1);
    m.unify(t, int);
    assert!(m.unify_errors.is_empty());
    // every member's own slot carries the binding, not just the rep's
    assert!(matches!(m.nodes[a].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[b].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[t].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[int].value, Some(Value::USize(1))));
}

#[test]
fn a_newcomer_joining_a_bound_class_carries_the_value() {
    let mut m = Module::new();
    let block = m.add_block(None);
    let a = unbound_node(&mut m, block);
    let int = usize_node(&mut m, block, 1);
    m.unify(a, int);
    assert!(m.unify_errors.is_empty());
    let b = unbound_node(&mut m, block);
    m.unify(a, b); // an unbound node joins the bound class
    assert!(m.unify_errors.is_empty());
    assert!(matches!(m.nodes[a].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[b].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[int].value, Some(Value::USize(1))));
}

// --- garbage collection interplay --------------------------------------

/// Collect a block whose member of a class is unreachable from the block
/// root: the member dies, the class's survivor keeps the binding, and the
/// member list stays clean (walkable, no stale ids).
#[test]
fn garbage_collecting_a_block_splices_its_members_out_of_the_class() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let x = unbound_node(&mut m, root);
    let y = unbound_node(&mut m, child); // not reachable from child_root
    let child_root = usize_node(&mut m, child, 0);
    m.unify(x, y);
    let int = usize_node(&mut m, root, 7);
    m.unify(x, int);
    assert!(m.unify_errors.is_empty());
    assert!(matches!(m.nodes[y].value, Some(Value::USize(7))));

    m.garbage_collect(child_root);

    assert!(!m.blocks.contains_key(child));
    let rep = m.equality_representative(x);
    assert_eq!(m.equality_representative(int), rep);
    assert!(matches!(m.nodes[rep].value, Some(Value::USize(7))));
    // the member list holds exactly the two survivors, in join order
    let members: Vec<_> = disjoint::members(&m.nodes, rep).collect();
    assert_eq!(members.len(), 2);
    assert!(members.contains(&x) && members.contains(&int));
}

/// The class representative dies with the block: a survivor is re-elected,
/// all survivors' parents re-point at it, and the binding is still readable.
#[test]
fn garbage_collect_re_elects_a_representative_when_the_old_one_dies() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let y1 = unbound_node(&mut m, child);
    let y2 = unbound_node(&mut m, child);
    let child_root = usize_node(&mut m, child, 0);
    let x = unbound_node(&mut m, root);
    // union by size keeps the representative in the child block
    m.unify(y1, y2);
    m.unify(y1, x);
    let int = usize_node(&mut m, root, 7);
    m.unify(x, int);
    assert!(m.unify_errors.is_empty());
    assert_eq!(m.equality_representative(x), y1);

    m.garbage_collect(child_root);

    assert!(!m.blocks.contains_key(child));
    let rep = m.equality_representative(x);
    assert_eq!(m.equality_representative(int), rep);
    assert_ne!(rep, y1);
    assert!(matches!(m.nodes[rep].value, Some(Value::USize(7))));
    let members: Vec<_> = disjoint::members(&m.nodes, rep).collect();
    assert_eq!(members.len(), 2);
    assert!(members.contains(&x) && members.contains(&int));
}

// --- application-time unification --------------------------------------
//
// The functions below have body `f(x) = x` — the return node IS the
// parameter — so no operator is needed to exercise the apply-time unify.

#[test]
fn apply_unifies_the_cloned_parameter_with_the_argument() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = unbound_node(&mut m, root);
    let f = m.add_function(root, param, param, &[param]);
    let arg = u128_node(&mut m, root, 42);
    let call = apply_node(&mut m, root, f, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
    assert!(m.unify_errors.is_empty());
    // the parameter's clone merged with the argument: one class of two
    let rep = m.equality_representative(arg);
    assert_eq!(disjoint::members(&m.nodes, rep).count(), 2);
}

#[test]
fn apply_with_an_unbound_argument_stays_lazy() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = unbound_node(&mut m, root);
    let f = m.add_function(root, param, param, &[param]);
    let arg = unbound_node(&mut m, root);
    let call = apply_node(&mut m, root, f, arg);
    assert!(matches!(
        m.evaluate_node_deep(call, None),
        Value::Parameterized
    ));
    assert!(m.unify_errors.is_empty());
    // two unbound nodes unify into one class, still unbound
    let rep = m.equality_representative(arg);
    assert_eq!(disjoint::members(&m.nodes, rep).count(), 2);
}

#[test]
fn apply_unifies_array_parameters_elementwise() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = x with x = [x0, x1]: the parameter's structure is an array,
    // so the apply unifies it elementwise against the argument.
    let x0 = unbound_node(&mut m, root);
    let x1 = unbound_node(&mut m, root);
    let param = array_node(&mut m, root, &[x0, x1]);
    let f = m.add_function(root, param, param, &[param, x0, x1]);
    let one = usize_node(&mut m, root, 1);
    let two = usize_node(&mut m, root, 2);
    let arg = array_node(&mut m, root, &[one, two]);
    let call = apply_node(&mut m, root, f, arg);
    let value = m.evaluate_node_deep(call, None);
    assert!(m.unify_errors.is_empty());
    // the cloned pattern's elements are bound to the argument's elements
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert!(matches!(m.nodes[ids[0]].value, Some(Value::USize(1))));
    assert!(matches!(m.nodes[ids[1]].value, Some(Value::USize(2))));
}

#[test]
fn apply_time_conflict_records_an_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The parameter is already bound to a concrete value — a defined
    // signature like `x: int`.  Applying a conflicting argument is a
    // runtime type error at the application site.
    let param = unbound_node(&mut m, root);
    let f = m.add_function(root, param, param, &[param]);
    let one = usize_node(&mut m, root, 1);
    m.unify(param, one);
    assert!(m.unify_errors.is_empty());
    let float = str_node(&mut m, root, &['f']);
    let call = apply_node(&mut m, root, f, float);
    m.evaluate_node_deep(call, None);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(param),
        m.equality_representative(float)
    );
    let error = m.unify_errors[0];
    assert!(matches!(error.value_a, Some(Value::USize(1))));
    assert!(matches!(error.value_b, Some(Value::Ext(_))));
}

#[test]
fn apply_unify_binds_an_unbound_argument_into_the_param_class() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = unbound_node(&mut m, root);
    let f = m.add_function(root, param, param, &[param]);
    let one = usize_node(&mut m, root, 1);
    m.unify(param, one);
    assert!(m.unify_errors.is_empty());
    let unbound = unbound_node(&mut m, root);
    let call = apply_node(&mut m, root, f, unbound);
    assert!(matches!(
        m.evaluate_node_deep(call, None),
        Value::USize(1)
    ));
    assert!(m.unify_errors.is_empty());
    // the argument node itself now carries the parameter's value
    assert!(matches!(m.nodes[unbound].value, Some(Value::USize(1))));
}

#[test]
fn apply_reestablishes_the_parameter_patterns_internal_classes() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = x with x = [x0, x1], where x0 ~ x1 are unified in the
    // template — a homogeneous pattern: both elements must unify to the
    // same value.  The apply re-establishes the class among the cloned
    // elements, so the argument is forced to satisfy it.
    let x0 = unbound_node(&mut m, root);
    let x1 = unbound_node(&mut m, root);
    let param = array_node(&mut m, root, &[x0, x1]);
    let f = m.add_function(root, param, param, &[param, x0, x1]);
    m.unify(x0, x1);
    assert!(m.unify_errors.is_empty());

    // [1, 2]: the second element conflicts with the first through the
    // re-established pattern class.
    let one = usize_node(&mut m, root, 1);
    let two = usize_node(&mut m, root, 2);
    let arg = array_node(&mut m, root, &[one, two]);
    let call = apply_node(&mut m, root, f, arg);
    m.evaluate_node_deep(call, None);
    assert_eq!(m.unify_errors.len(), 1);

    // [1, 1] merges.
    let one_a = usize_node(&mut m, root, 1);
    let one_b = usize_node(&mut m, root, 1);
    let arg2 = array_node(&mut m, root, &[one_a, one_b]);
    let call2 = apply_node(&mut m, root, f, arg2);
    m.evaluate_node_deep(call2, None);
    assert_eq!(m.unify_errors.len(), 1);
}

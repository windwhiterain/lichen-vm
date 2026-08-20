use lichen_vm::lowlevel::{
    Block, BlockId, Module, Node, NodeId, Operation, Operator, OperatorExt, Program, Value,
    ValueExt,
};
use slotmap::SlotMap;

#[derive(Clone, Copy)]
struct TestProgram;

impl Program for TestProgram {
    type Value = TestValue;
    type Operator = TestOperator;
}

/// Extension values carrying typed pointers into a block's arena.  The
/// arena machinery only deals in byte views, so `ValueExt` converts
/// between the typed pointer and a byte slice on the fly.
#[derive(Clone, Copy)]
enum TestValue {
    /// A `u128` payload (16 bytes).
    U128(*const u128),
    /// A `char` payload, four bytes per char.
    String(*const [char]),
}
impl ValueExt for TestValue {
    fn is_ptr(&self) -> bool {
        true
    }
    // `U128` requires 16-byte alignment; returning the strictest
    // alignment keeps the `String` copies over-aligned, which is safe.
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

#[derive(Clone, Copy)]
enum TestOperator {
    /// Pass the operand through untouched.
    Id,
    Add,
    Concat,
}

impl OperatorExt<TestProgram> for TestOperator {
    fn run(
        &self,
        operand: Value<TestProgram>,
        block: &mut Block,
        nodes: &SlotMap<NodeId, Node<TestProgram>>,
    ) -> Value<TestProgram> {
        match self {
            TestOperator::Id => operand,
            // Binary ops receive their operands as an array of two node
            // ids; the elements are already evaluated.
            TestOperator::Add | TestOperator::Concat => {
                let Value::Array(operands) = operand else {
                    unreachable!("Add/Concat expect an array of two node ids")
                };
                let operands = unsafe { &*operands };
                let left = nodes[operands[0]].value.unwrap();
                let right = nodes[operands[1]].value.unwrap();
                match self {
                    TestOperator::Add => {
                        let sum = u128_of(left).wrapping_add(u128_of(right));
                        let p = block.arena.alloc(sum);
                        Value::Ext(TestValue::U128(p as *const u128))
                    }
                    _ => {
                        let mut result = string_of(left);
                        result.extend(string_of(right));
                        let slice = block.arena.alloc_slice_copy(&result);
                        Value::Ext(TestValue::String(std::ptr::slice_from_raw_parts(
                            slice.as_ptr(),
                            slice.len(),
                        )))
                    }
                }
            }
        }
    }
}

fn u128_of(value: Value<TestProgram>) -> u128 {
    let Value::Ext(TestValue::U128(ptr)) = value else {
        panic!("expected U128")
    };
    u128::from_ne_bytes(
        unsafe { std::slice::from_raw_parts(ptr as *const u8, 16) }
            .try_into()
            .unwrap(),
    )
}

fn string_of(value: Value<TestProgram>) -> Vec<char> {
    let Value::Ext(TestValue::String(ptr)) = value else {
        panic!("expected String")
    };
    unsafe { &*ptr }.to_vec()
}

// --- builders ---------------------------------------------------------

fn u128_node(m: &mut Module<TestProgram>, block: BlockId, n: u128) -> NodeId {
    let p = m.blocks[block].arena.alloc(n);
    m.add_node(block, None, Some(Value::Ext(TestValue::U128(p as *const u128))))
}

fn str_node(m: &mut Module<TestProgram>, block: BlockId, chars: &[char]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(chars);
    m.add_node(
        block,
        None,
        Some(Value::Ext(TestValue::String(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))),
    )
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

fn usize_node(m: &mut Module<TestProgram>, block: BlockId, n: usize) -> NodeId {
    m.add_node(block, None, Some(Value::USize(n)))
}

fn op_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    operator: Operator<TestProgram>,
    operand: Option<NodeId>,
) -> NodeId {
    m.add_node(block, Some(Operation { operator, operand }), None)
}

/// Assert `value` is an array whose elements hold the given `u128`s.
fn assert_u128_array(m: &Module<TestProgram>, value: Value<TestProgram>, expected: &[u128]) {
    let Value::Array(ptr) = value else {
        panic!("expected array")
    };
    let ids = unsafe { &*ptr };
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected) {
        assert_eq!(u128_of(m.nodes[id].value.unwrap()), n);
    }
}

// --- tests ------------------------------------------------------------

#[test]
fn redundant_nodes_are_not_compacted() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 5);
    let y = u128_node(&mut m, child, 7); // redundant: never referenced
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(x));

    let value = m.evaluate_node_deep(root_node,None);

    assert_eq!(u128_of(value), 5);
    assert_eq!(m.nodes.len(), 2); // root_node + child's kept return x
    assert!(!m.nodes.contains_key(y));
}

#[test]
fn u128_payload_is_relocated_into_parent_and_block_releasable() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // Marker allocated in root's arena before the child runs.  Bumpalo
    // allocates downward within a chunk, so later allocations sit at
    // lower addresses.
    let marker = m.blocks[root].arena.alloc_slice_copy(b"marker");
    let marker_start = marker.as_ptr() as usize;

    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 42);
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(x));

    let value = m.evaluate_node_deep(root_node,None);
    let Value::Ext(TestValue::U128(ptr)) = value else {
        panic!("expected U128")
    };
    assert_eq!(u128_of(value), 42);
    // Relocated into root's arena: the copy was made after the marker,
    // so it sits below it in the same chunk.
    assert!(ptr as *const u8 as usize + 16 <= marker_start);

    // The child block was released: gone from the block table, yet the
    // value still points into root's arena.
    assert!(!m.blocks.contains_key(child));
    assert_eq!(u128_of(value), 42);
}

#[test]
fn add_sums_u128_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
    let add = op_node(&mut m, root, Operator::Ext(TestOperator::Add), Some(operands));

    let value = m.evaluate_node_deep(add,None);

    assert_eq!(u128_of(value), 7);
}

#[test]
fn concat_joins_string_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = str_node(&mut m, root, &['a', 'b']);
    let b = str_node(&mut m, root, &['c', 'd']);
    let operands = array_node(&mut m, root, &[a, b]);
    let concat = op_node(&mut m, root, Operator::Ext(TestOperator::Concat), Some(operands));

    let value = m.evaluate_node_deep(concat,None);

    assert_eq!(string_of(value), vec!['a', 'b', 'c', 'd']);
}

#[test]
fn index_selects_array_element() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b]);
    let idx = usize_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[arr, idx]);
    let index = op_node(&mut m, root, Operator::Index, Some(operands));

    let value = m.evaluate_node_deep(index,None);

    assert_eq!(u128_of(value), 20);
}

#[test]
fn array_return_compacts_elements_into_parent() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let a = u128_node(&mut m, child, 10);
    let b = u128_node(&mut m, child, 20);
    let c = u128_node(&mut m, child, 30);
    let ret = array_node(&mut m, child, &[a, b, c]);
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(ret));

    let value = m.evaluate_node_deep(root_node,None);

    // The element nodes keep their ids: their data was relocated into the
    // root's arena, so they stay readable after the child was released.
    assert_u128_array(&m, value, &[10, 20, 30]);
    assert_eq!(m.nodes.len(), 5); // root_node + ret + a, b, c
    assert!(m.nodes.contains_key(a));
    assert!(m.nodes.contains_key(b));
    assert!(m.nodes.contains_key(c));
}

#[test]
fn nested_scalar_return_compacts_into_grandparent() {
    let mut m = Module::new();
    let grandparent = m.add_block(None);
    let outer = m.add_block(Some(grandparent));
    let inner = m.add_block(Some(outer));
    let x = u128_node(&mut m, inner, 9);
    let ret = array_node(&mut m, outer, &[x]); // outer's return references inner's return x
    let root_node = op_node(
        &mut m,
        grandparent,
        Operator::Ext(TestOperator::Id),
        Some(ret),
    );

    let value = m.evaluate_node_deep(root_node,None);

    assert_u128_array(&m, value, &[9]);
    assert_eq!(m.nodes.len(), 3); // root_node + outer's kept return + inner's kept return
    assert!(m.nodes.contains_key(x));
}

#[test]
fn nested_array_return_relocates_data_twice() {
    let mut m = Module::new();
    let grandparent = m.add_block(None);
    let outer = m.add_block(Some(grandparent));
    let inner = m.add_block(Some(outer));
    let c = u128_node(&mut m, inner, 7);
    let inner_ret = array_node(&mut m, inner, &[c]);
    let outer_ret = array_node(&mut m, outer, &[inner_ret]);
    let root_node = op_node(
        &mut m,
        grandparent,
        Operator::Ext(TestOperator::Id),
        Some(outer_ret),
    );

    let value = m.evaluate_node_deep(root_node,None);

    // inner's data was relocated into outer first, then into grandparent;
    // all node ids survive unchanged.
    let Value::Array(ptr) = value else {
        panic!("expected array")
    };
    let ids = unsafe { &*ptr };
    assert_eq!(ids.len(), 1);
    let Value::Array(ptr) = m.nodes[ids[0]].value.unwrap() else {
        panic!("expected array")
    };
    let ids = unsafe { &*ptr };
    assert_eq!(ids.len(), 1);
    assert_eq!(u128_of(m.nodes[ids[0]].value.unwrap()), 7);
    assert_eq!(m.nodes.len(), 4); // root_node + outer_ret + inner_ret + c
    assert!(m.nodes.contains_key(c));
}

#[test]
fn unreferenced_child_blocks_are_released() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let x = u128_node(&mut m, child, 5);
    let grandchild = m.add_block(Some(child));
    let orphan = u128_node(&mut m, grandchild, 9); // never referenced
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(x));

    let value = m.evaluate_node_deep(root_node,None);

    assert_eq!(u128_of(value), 5);
    assert_eq!(m.nodes.len(), 2); // root_node + child's kept return x
    assert!(!m.nodes.contains_key(orphan));
}

#[test]
fn block_run_pulls_outer_and_sibling_blocks() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_block(Some(root));
    let c = m.add_block(Some(p));
    let s = m.add_block(Some(p)); // sibling of c
    let z = u128_node(&mut m, s, 11); // s's return
    let y = op_node(&mut m, p, Operator::Ext(TestOperator::Id), Some(z)); // p's node uses sibling s
    let c_ret = op_node(&mut m, c, Operator::Ext(TestOperator::Id), Some(y)); // c references outer y
    let p_ret = op_node(&mut m, p, Operator::Ext(TestOperator::Id), Some(c_ret)); // p's return is c's result
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(p_ret));

    let value = m.evaluate_node_deep(root_node,None);

    // Running p ran c, whose resolution pulled in p's outer node y,
    // which ran sibling s; the result is compacted up to the root.
    assert_eq!(u128_of(value), 11);
    assert_eq!(m.nodes.len(), 2); // root_node + p's kept return
    assert!(!m.nodes.contains_key(z));
    assert!(!m.blocks.contains_key(s)); // sibling s was released
    assert!(!m.blocks.contains_key(c));
    assert!(!m.blocks.contains_key(p));
}

#[test]
#[should_panic(expected = "cycle")]
fn cyclic_operations_panic_instead_of_looping() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = op_node(&mut m, root, Operator::Ext(TestOperator::Id), None);
    let b = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(a));
    // Close the loop a -> b -> a through the public operation fields; the
    // evaluating state marks b as in-progress on re-entry and panics.
    m.nodes[a].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(b),
    });
    m.evaluate_node_deep(b, None);
}

#[test]
fn deep_never_run_block_chain_releases_stack_safely() {
    let mut m = Module::new();
    let top = m.add_block(None);
    let mut prev = top;
    for _ in 0..100_000 {
        prev = m.add_block(Some(prev));
    }
    let first = m.blocks[top].children[0];
    let x = u128_node(&mut m, first, 1);

    // Running first releases its whole never-run subtree, 100_000 blocks deep.
    m.evaluate_node_deep(x, Some(top));

    assert_eq!(u128_of(m.nodes[x].value.unwrap()), 1);
    assert_eq!(m.nodes.len(), 1); // only x survived, moved to top
    assert!(m.blocks.contains_key(top));
    assert!(!m.blocks.contains_key(first));
    assert_eq!(m.blocks.len(), 1); // top is all that remains
}

#[test]
fn visiting_markers_are_cleared_after_evaluation() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
    let add = op_node(&mut m, root, Operator::Ext(TestOperator::Add), Some(operands));

    m.evaluate_node_deep(add, None);

    assert!(m.nodes.values().all(|n| !n.visiting));
}

#[test]
fn deep_block_chain_evaluates_stack_safely() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let mut chain = vec![root];
    for _ in 0..100_000 {
        let id = m.add_block(Some(*chain.last().unwrap()));
        chain.push(id);
    }
    // Deepest block holds the constant; each block's return op references the
    // child block's return node, so evaluation nests 100_000 block runs deep.
    let mut ret = u128_node(&mut m, *chain.last().unwrap(), 7);
    for i in (1..chain.len() - 1).rev() {
        ret = op_node(&mut m, chain[i], Operator::Ext(TestOperator::Id), Some(ret));
    }
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(ret));

    let value = m.evaluate_node_deep(root_node, None);

    assert_eq!(u128_of(value), 7);
    assert_eq!(m.blocks.len(), 1); // only root remains, chain compacted into it
    assert!(m.nodes.values().all(|n| !n.visiting));
}

#[test]
fn parameterized_deep_marks_subtrees_with_parameters() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_node(root, None, Some(Value::Parameterized));
    let x = u128_node(&mut m, root, 5);
    let arr = array_node(&mut m, root, &[x, p]);
    let id_arr = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(arr));
    let id_p = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(p));

    m.evaluate_node_deep(id_arr, None);

    // The parameter node itself and everything reachable from it is flagged;
    // plain constants are not.
    assert_eq!(m.nodes[p].parameterized_deep, Some(true));
    assert_eq!(m.nodes[arr].parameterized_deep, Some(true));
    assert_eq!(m.nodes[id_arr].parameterized_deep, Some(true));
    assert_eq!(m.nodes[x].parameterized_deep, Some(false));
    assert_eq!(m.nodes[id_p].parameterized_deep, None); // not yet evaluated

    m.evaluate_node_deep(id_p, None);
    assert_eq!(m.nodes[id_p].parameterized_deep, Some(true));
}

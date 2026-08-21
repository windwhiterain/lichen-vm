use lichen_vm::lowlevel::{
    Block, BlockId, Function, FunctionId, Module, Node, NodeId, Operation, Operator, OperatorExt,
    Program, Value, ValueExt,
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
            // ids; the elements are already evaluated.  A parameterized
            // operand means the body is still a template being defined —
            // stay lazy so the definition pass can flag it.
            TestOperator::Add | TestOperator::Concat => {
                if matches!(operand, Value::Parameterized) {
                    return Value::Parameterized;
                }
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

/// The node ids inside `value` if it's an array.
fn array_ids(value: Value<TestProgram>) -> &'static [NodeId] {
    let Value::Array(ptr) = value else {
        panic!("expected array")
    };
    unsafe { &*ptr }
}

/// Assert `value` is an array whose elements hold the given `u128`s.
fn assert_u128_array(m: &Module<TestProgram>, value: Value<TestProgram>, expected: &[u128]) {
    let ids = array_ids(value);
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected) {
        assert_eq!(u128_of(m.nodes[id].value.unwrap()), n);
    }
}

/// Wrap `ret`/`param` — and every node built so far in `block` — into a
/// function value node, returning the node and the function id.
fn wrap_function(
    m: &mut Module<TestProgram>,
    block: BlockId,
    ret: NodeId,
    param: NodeId,
) -> (NodeId, FunctionId) {
    let nodes = m.blocks[block].nodes.clone();
    let func_node = m.add_function(block, ret, param, &nodes);
    let Value::Function(func) = m.nodes[func_node].value.unwrap() else {
        unreachable!("add_function always wraps a function value")
    };
    (func_node, func)
}

/// Create a function value in a fresh body block: the return node at
/// `Block::RETURN_IDX` and the parameter at index 1 of the scope, wrapped
/// by [`Module::add_function`]. `wire` fills in the return node given the
/// parameter id.  Returns the function value node plus the return and
/// parameter node ids.
fn function(
    m: &mut Module<TestProgram>,
    wire: impl FnOnce(&mut Module<TestProgram>, NodeId, NodeId),
) -> (NodeId, NodeId, NodeId) {
    let block = m.add_block(None);
    let ret = m.add_node(block, None, None);
    let param = m.add_node(block, None, Some(Value::Parameterized));
    wire(m, ret, param);
    let (func_node, _) = wrap_function(m, block, ret, param);
    (func_node, ret, param)
}

/// A call node applying `func_node` to `arg` (operand array
/// `[function, argument]`).
fn call_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    func_node: NodeId,
    arg: NodeId,
) -> NodeId {
    let operands = array_node(m, block, &[func_node, arg]);
    op_node(m, block, Operator::Apply, Some(operands))
}

/// Build a self-referential function `f(x) = [x, f(x)]` in its own body
/// block: the Apply operand array references the function's own value node,
/// so each application of `f` produces one recursion level.  Returns the
/// function value node and id.
fn recursive_function(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    // Placeholder for the function's own value node: the operand array must
    // reference it before the function exists, so `add_function` (which
    // creates the value node last) cannot be used here.
    let func_node = m.add_node(body, None, None);
    let operands = array_node(m, body, &[func_node, param]);
    let apply = op_node(m, body, Operator::Apply, Some(operands));
    let ret = array_node(m, body, &[param, apply]);
    let function = m.functions.insert(Function {
        nodes: vec![param, func_node, operands, apply, ret],
        r#return: ret,
        parameter: param,
        block: body,
    });
    m.blocks[body].functions.push(function);
    m.nodes[func_node].value = Some(Value::Function(function));
    (func_node, function)
}

/// Build two functions calling each other: `f(x) = [x, g(x)]` and
/// `g(x) = [x, f(x)]`, sharing one body block.  Returns the two function
/// value nodes.
fn mutually_recursive_functions(m: &mut Module<TestProgram>) -> (NodeId, NodeId) {
    let body = m.add_block(None);
    let f_param = m.add_node(body, None, Some(Value::Parameterized));
    let g_param = m.add_node(body, None, Some(Value::Parameterized));
    // Both value nodes are placeholders: f's body references g before g
    // exists, and vice versa.
    let f_func = m.add_node(body, None, None);
    let g_func = m.add_node(body, None, None);
    // f(x) = [x, g(x)]
    let f_ops = array_node(m, body, &[g_func, f_param]);
    let f_apply = op_node(m, body, Operator::Apply, Some(f_ops));
    let f_ret = array_node(m, body, &[f_param, f_apply]);
    // g(x) = [x, f(x)]
    let g_ops = array_node(m, body, &[f_func, g_param]);
    let g_apply = op_node(m, body, Operator::Apply, Some(g_ops));
    let g_ret = array_node(m, body, &[g_param, g_apply]);
    let f = m.functions.insert(Function {
        nodes: vec![f_param, f_func, f_ops, f_apply, f_ret],
        r#return: f_ret,
        parameter: f_param,
        block: body,
    });
    let g = m.functions.insert(Function {
        nodes: vec![g_param, g_func, g_ops, g_apply, g_ret],
        r#return: g_ret,
        parameter: g_param,
        block: body,
    });
    m.blocks[body].functions.extend([f, g]);
    m.nodes[f_func].value = Some(Value::Function(f));
    m.nodes[g_func].value = Some(Value::Function(g));
    (f_func, g_func)
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
    let ids = array_ids(value);
    assert_eq!(ids.len(), 1);
    let ids = array_ids(m.nodes[ids[0]].value.unwrap());
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

#[test]
fn function_call_operator_clones_body_and_maps_parameter() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });

    // The template scope is exactly the body nodes, return at index 0 and
    // parameter at index 1.
    let Value::Function(func) = m.nodes[func_node].value.unwrap() else {
        unreachable!("expected a function value")
    };
    assert_eq!(m.functions[func].nodes.as_slice(), &[ret, param]);

    // The body is untouched and still callable: its parameter is still the
    // marker and its return node still references it.
    let body = m.nodes[ret].block;
    assert!(m.blocks.contains_key(body));
    assert_eq!(m.nodes[ret].operation.unwrap().operand, Some(param));
    assert!(matches!(m.nodes[param].value, Some(Value::Parameterized)));
    assert_eq!(m.nodes[ret].block, body);
    assert_eq!(m.nodes[param].block, body);

    // The call operator resolves through the argument and caches the
    // result on the call node in its own block.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
    assert_eq!(m.nodes[call].block, root);
    assert_eq!(u128_of(m.nodes[call].value.unwrap()), 42);
    assert_eq!(m.nodes.len(), 7); // ret + param + func + arg + operands + call + clone_ret
}

#[test]
fn function_call_operator_clones_array_body() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, 7]
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, Some(Value::Parameterized)); // PARAMETER_IDX
    let seven = u128_node(&mut m, f, 7);
    let slice = m.blocks[f].arena.alloc_slice_copy(&[param, seven]);
    m.nodes[ret].value = Some(Value::Array(std::ptr::slice_from_raw_parts(
        slice.as_ptr(),
        slice.len(),
    )));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The array embeds the parameter, so the definition pass (evaluating
    // the body with the marker parameter) flags it parameterized.
    m.evaluate_node_deep(ret, None);
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 10);
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);

    assert_u128_array(&m, value, &[10, 7]);
    // The clone's array holds the argument node and references the body's
    // constant in place; the body's own array still references the parameter.
    assert_eq!(array_ids(m.nodes[call].value.unwrap()), &[arg, seven]);
    assert_eq!(m.nodes[seven].block, f); // referenced in place, not cloned
    assert_eq!(array_ids(m.nodes[ret].value.unwrap()), &[param, seven]); // body unchanged
}

#[test]
fn function_call_operator_preserves_parameterized_operand_chain() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(Id(x))
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, Some(Value::Parameterized)); // PARAMETER_IDX
    let mid = op_node(&mut m, f, Operator::Ext(TestOperator::Id), Some(param));
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(mid),
    });
    let (func_node, _) = wrap_function(&mut m, f, ret, param);

    // The argument is itself parameterized, so the call result stays a
    // marker until the argument resolves.
    let arg = m.add_node(root, None, Some(Value::Parameterized));
    let call = call_node(&mut m, root, func_node, arg);
    let value = m.evaluate_node_deep(call, None);
    assert!(matches!(value, Value::Parameterized));
    assert_eq!(m.nodes[call].parameterized_deep, Some(true));

    // The body is untouched.
    assert!(m.blocks.contains_key(m.nodes[ret].block));
    assert_eq!(m.nodes[mid].operation.unwrap().operand, Some(param));

    // Re-bind the argument node and re-evaluate the call: the cloned chain
    // resolves through it.
    let p = m.blocks[root].arena.alloc(99u128);
    m.nodes[arg].value = Some(Value::Ext(TestValue::U128(p as *const u128)));
    m.nodes[call].value = None; // drop the cached marker
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 99);
}

#[test]
fn function_call_operator_recomputes_stale_definition_markers() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });

    // The definition pass evaluates the body with the parameter as a
    // marker, caching a marker value on the return node and flagging it
    // parameterized.
    m.evaluate_node_deep(ret, None);
    assert!(matches!(m.nodes[ret].value, Some(Value::Parameterized)));
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    // The call clones the parameterized node unevaluated — the stale
    // marker is recomputed against the concrete argument.
    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The body is untouched and stays callable.
    assert_eq!(m.nodes[ret].operation.unwrap().operand, Some(param));
    assert!(m.nodes[param].value.is_some());
}

#[test]
fn function_call_operator_references_concrete_body_nodes_in_place() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Id(7): no path depends on the parameter.
    let (func_node, ret, param) = function(&mut m, |m, ret, _param| {
        let seven = u128_node(m, m.nodes[ret].block, 7);
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(seven),
        });
    });

    // The definition pass resolves the body to a concrete constant.
    m.evaluate_node_deep(ret, None);
    assert_eq!(m.nodes[ret].parameterized_deep, Some(false));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 7);
    assert!(matches!(m.nodes[param].value, Some(Value::Parameterized))); // body untouched
}

#[test]
fn function_in_local_block_survives_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), built inside a local block that is compacted away.
    let ret = m.add_node(child, None, None); // RETURN_IDX
    let param = m.add_node(child, None, Some(Value::Parameterized)); // PARAMETER_IDX
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(param),
    });
    let (func_node, _) = wrap_function(&mut m, child, ret, param);

    // Calling the function while it still lives in the local block works.
    let arg = u128_node(&mut m, child, 42);
    let call = call_node(&mut m, child, func_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // Running the block compacts the function value into the root and maps
    // the template nodes along with it.
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(func_node));
    let Value::Function(mapped) = m.evaluate_node_deep(root_node, None) else {
        panic!("expected function")
    };
    assert!(!m.blocks.contains_key(child));
    assert_eq!(m.nodes[ret].block, root); // template mapped into the root
    assert_eq!(m.nodes[param].block, root);
    assert_eq!(m.functions[mapped].nodes.as_slice(), &[ret, param]);
    assert!(m.functions.contains_key(mapped)); // the function outlives the block

    // The outer block can still call the mapped function.
    let arg2 = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, func_node, arg2);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 7);
}

#[test]
fn function_scope_is_dropped_with_its_block() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // f(x) = Id(x), homed in the child but not reachable from the block's
    // return, so it must not survive the compaction.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, Some(Value::Parameterized));
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(param),
    });
    let (func_node, func) = wrap_function(&mut m, child, ret, param);
    assert_eq!(m.functions.len(), 1);

    // Evaluate a *different* node of the child: the block compacts only the
    // return-reachable tree, then releases the rest — the function's home
    // node included, dropping the function and its scope.
    let x = u128_node(&mut m, child, 5);
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(x));
    assert_eq!(u128_of(m.evaluate_node_deep(root_node, None)), 5);

    assert!(!m.blocks.contains_key(child));
    assert!(!m.nodes.contains_key(func_node));
    assert!(!m.functions.contains_key(func)); // scope dropped with the block
}

// --- nested functions, higher-order functions, mixed blocks+functions ---

#[test]
fn nested_function_is_called_by_the_outer_body() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g(y) = Id(y) is defined inside f's template block, and f(x) = g(x)
    // calls it with f's own parameter.
    let f = m.add_block(None);
    let gret = m.add_node(f, None, None);
    let gparam = m.add_node(f, None, Some(Value::Parameterized));
    m.nodes[gret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(gparam),
    });
    let (g_node, _) = wrap_function(&mut m, f, gret, gparam);
    let ret = m.add_node(f, None, None);
    let param = m.add_node(f, None, Some(Value::Parameterized));
    let operands = array_node(&mut m, f, &[g_node, param]);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Apply,
        operand: Some(operands),
    });
    let (f_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: the nested g is concrete

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The nested g is untouched in the template and still callable directly.
    assert_eq!(m.nodes[g_node].block, f);
    let g_arg = u128_node(&mut m, root, 5);
    let g_call = call_node(&mut m, root, g_node, g_arg);
    assert_eq!(u128_of(m.evaluate_node_deep(g_call, None)), 5);
}

#[test]
fn outer_call_returns_a_nested_function_value() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // g(y) = Id(y) lives inside f's body; f(x) = g returns it.
    let f = m.add_block(None);
    let gret = m.add_node(f, None, None);
    let gparam = m.add_node(f, None, Some(Value::Parameterized));
    m.nodes[gret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(gparam),
    });
    let (g_node, g_id) = wrap_function(&mut m, f, gret, gparam);
    let ret = m.add_node(f, None, None);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Id),
        operand: Some(g_node),
    });
    let param = m.add_node(f, None, Some(Value::Parameterized));
    let (f_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: Id(g) is concrete

    let one = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, f_node, one);
    let Value::Function(got) = m.evaluate_node_deep(call, None) else {
        panic!("expected the nested function value");
    };
    assert_eq!(got, g_id); // the same nested function, referenced in place

    // The returned function is callable from the outer block.
    let arg = u128_node(&mut m, root, 7);
    let call2 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 7);
}

#[test]
fn higher_order_function_passes_a_function_argument_through() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(g) = g: the parameter is the return node's operand, so calling
    // apply with a function argument hands that function back.
    let (apply_node, ret, _param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });
    m.evaluate_node_deep(ret, None); // definition pass: Id(marker) stays a marker

    // g(x) = Id(x).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });
    let Value::Function(g_id) = m.nodes[g_node].value.unwrap() else {
        unreachable!("expected a function value")
    };

    let call = call_node(&mut m, root, apply_node, g_node);
    let Value::Function(got) = m.evaluate_node_deep(call, None) else {
        panic!("expected the function argument back");
    };
    assert_eq!(got, g_id);

    // The passed-through function is still callable.
    let arg = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, g_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 9);
}

#[test]
fn higher_order_function_calls_its_function_argument() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // apply(f) = f(42): the parameter is the target of an Apply node.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    let forty_two = u128_node(&mut m, body, 42);
    let operands = array_node(&mut m, body, &[param, forty_two]);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Apply,
        operand: Some(operands),
    });
    let (apply_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: a marker target stays lazy

    // g(x) = Id(x): passing g as the argument makes apply evaluate g(42).
    let (g_node, _, _) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });
    let call = call_node(&mut m, root, apply_node, g_node);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);
}

#[test]
fn function_can_index_into_parameterized_array() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, 7][0]: the array embeds the parameter, so the Index arm
    // sees a marker element and stays lazy during the definition pass.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    let seven = u128_node(&mut m, body, 7);
    let array = array_node(&mut m, body, &[param, seven]);
    let zero = usize_node(&mut m, body, 0);
    let operands = array_node(&mut m, body, &[array, zero]);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Index,
        operand: Some(operands),
    });
    let (f_node, _) = wrap_function(&mut m, body, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass: index of a marker stays a marker
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, f_node, arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 42);

    // The body is untouched and still parameterized.
    assert!(matches!(m.nodes[param].value, Some(Value::Parameterized)));
}

#[test]
fn manually_partially_evaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(Add(x, 1), 2).  The partial definition is built by hand:
    // evaluate_node_deep is called on exactly the two constants, and
    // nothing else — the parameter-dependent chain stays unevaluated.
    let body = m.add_block(None);
    let ret = m.add_node(body, None, None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    let one = u128_node(&mut m, body, 1);
    let inner_ops = array_node(&mut m, body, &[param, one]);
    let inner = op_node(&mut m, body, Operator::Ext(TestOperator::Add), Some(inner_ops));
    let two = u128_node(&mut m, body, 2);
    let ret_ops = array_node(&mut m, body, &[inner, two]);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Add),
        operand: Some(ret_ops),
    });
    let (f_node, _) = wrap_function(&mut m, body, ret, param);

    // Manually define exactly the constants; the parameter-dependent nodes
    // keep parameterized_deep = None.
    m.evaluate_node_deep(one, None);
    m.evaluate_node_deep(two, None);
    assert_eq!(m.nodes[one].parameterized_deep, Some(false));
    assert_eq!(m.nodes[two].parameterized_deep, Some(false));
    assert_eq!(m.nodes[ret].parameterized_deep, None);
    assert_eq!(m.nodes[inner].parameterized_deep, None);
    assert_eq!(m.nodes[inner_ops].parameterized_deep, None);
    assert_eq!(m.nodes[ret_ops].parameterized_deep, None);

    // The apply reuses the proven constants in place and clones + remaps
    // the unevaluated chain: f(5) = (5 + 1) + 2 = 8, f(9) = 12.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 8);

    // The clone of the inner operand array maps the parameter onto the
    // argument while keeping the proven constant in place.
    let cloned_inner_ops = m.blocks[root]
        .nodes
        .iter()
        .copied()
        .find(|&id| {
            matches!(m.nodes[id].value, Some(Value::Array(_)))
                && array_ids(m.nodes[id].value.unwrap()) == [five, one]
        })
        .expect("the cloned inner operand array references the argument");
    assert_eq!(m.nodes[cloned_inner_ops].block, root);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 12);
}

#[test]
fn unevaluated_function_applies_correctly() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = Add(x, 1), with no evaluate_node / evaluate_node_deep call
    // before applying: every body node keeps parameterized_deep = None.
    let (f_node, ret, param) = function(&mut m, |m, ret, param| {
        let one = u128_node(m, m.nodes[ret].block, 1);
        let operands = array_node(m, m.nodes[ret].block, &[param, one]);
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Add),
            operand: Some(operands),
        });
    });
    assert_eq!(m.nodes[ret].parameterized_deep, None);
    assert_eq!(m.nodes[param].parameterized_deep, None);

    // The apply clones the whole unevaluated body and resolves it against
    // the argument: f(5) = 6, f(9) = 10.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 6);

    let nine = u128_node(&mut m, root, 9);
    let call2 = call_node(&mut m, root, f_node, nine);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 10);
}

#[test]
fn recursive_function_applies_itself_lazily() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (f_node, f_id) = recursive_function(&mut m);
    // The function's own value node is concrete (it never depends on the
    // parameter), so the clone keeps the self-reference in place instead of
    // copying the function per level.
    m.evaluate_node_deep(f_node, None);
    assert_eq!(m.nodes[f_node].parameterized_deep, Some(false));

    // f(5) = [5, f(5)]: each forced application produces exactly one new
    // level — a fresh, still-unevaluated apply clone referencing the same
    // function value and the same argument.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], five);
    let c1 = ids[1];
    assert!(m.nodes[c1].value.is_none()); // unevaluated until forced
    assert!(matches!(
        m.nodes[c1].operation,
        Some(Operation { operator: Operator::Apply, .. })
    ));
    let ops = m.nodes[c1].operation.unwrap().operand.unwrap();
    assert_eq!(array_ids(m.nodes[ops].value.unwrap()), [f_node, five]);

    // Forcing that level runs the same function against the same argument.
    let level1 = m.evaluate_node(c1, None);
    let ids1 = array_ids(level1);
    assert_eq!(ids1.len(), 2);
    assert_eq!(ids1[0], five);
    let c2 = ids1[1];
    assert_ne!(c2, c1);
    let ops = m.nodes[c2].operation.unwrap().operand.unwrap();
    assert_eq!(array_ids(m.nodes[ops].value.unwrap()), [f_node, five]);

    let level2 = m.evaluate_node(c2, None);
    assert_eq!(array_ids(level2)[0], five);
    assert!(m.nodes[array_ids(level2)[1]].value.is_none());

    // The recursion never cloned the function: the same template recursed
    // three times, referenced in place.
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.functions[f_id].block, m.nodes[f_node].block);
    assert!(matches!(m.nodes[f_node].value, Some(Value::Function(_))));
}

#[test]
fn undefined_recursive_function_clones_a_function_per_level() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // With no evaluation at all, the function value node's
    // parameterized_deep stays None, so the clone rule copies it: each
    // recursion level carries its own fresh function clone homed on the
    // calling block.
    let (f_node, f_id) = recursive_function(&mut m);
    assert_eq!(m.nodes[f_node].parameterized_deep, None);

    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], five);
    let c1 = ids[1];

    let ops = m.nodes[c1].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    let Value::Function(cloned) = m.nodes[operand_ids[0]].value.unwrap() else {
        panic!("expected a cloned function value")
    };
    assert_ne!(cloned, f_id);
    assert_eq!(m.functions[cloned].block, root);
    assert_eq!(m.functions[cloned].nodes.len(), 5);

    let level1 = m.evaluate_node(c1, None);
    let ids1 = array_ids(level1);
    assert_eq!(ids1.len(), 2);
    assert_eq!(ids1[0], five);
    let c2 = ids1[1];
    assert_ne!(c2, c1);
    let ops = m.nodes[c2].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    let Value::Function(cloned2) = m.nodes[operand_ids[0]].value.unwrap() else {
        panic!("expected a cloned function value")
    };
    assert_ne!(cloned2, cloned);
    assert_eq!(m.functions.len(), 3); // the original plus one clone per level
}

#[test]
fn mutually_recursive_functions_call_each_other() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (f_node, g_node) = mutually_recursive_functions(&mut m);

    // f(5) = [5, g(5)]: g's value node is outside f's template scope, so
    // the clone references it in place.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], five);
    let g_app = ids[1];
    let ops = m.nodes[g_app].operation.unwrap().operand.unwrap();
    assert_eq!(array_ids(m.nodes[ops].value.unwrap()), [g_node, five]);

    // Forcing that level runs g's body: g(5) = [5, f(5)].
    let level1 = m.evaluate_node(g_app, None);
    let ids = array_ids(level1);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], five);
    let f_app = ids[1];
    assert_ne!(f_app, g_app);
    let ops = m.nodes[f_app].operation.unwrap().operand.unwrap();
    assert_eq!(array_ids(m.nodes[ops].value.unwrap()), [f_node, five]);
    assert_eq!(m.functions.len(), 2); // cross-references stay in place
}

#[test]
fn mixed_blocks_and_functions_survive_compaction() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // g(x) = Add(x, 7) is built inside the child block; the constant 7
    // lives in the child and is part of g's scope.
    let ret = m.add_node(child, None, None);
    let param = m.add_node(child, None, Some(Value::Parameterized));
    let seven = u128_node(&mut m, child, 7);
    let operands = array_node(&mut m, child, &[param, seven]);
    m.nodes[ret].operation = Some(Operation {
        operator: Operator::Ext(TestOperator::Add),
        operand: Some(operands),
    });
    let (g_node, g_id) = wrap_function(&mut m, child, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass (Add is marker-aware)

    // Call g from inside the child: 10 + 7.
    let ten = u128_node(&mut m, child, 10);
    let call = call_node(&mut m, child, g_node, ten);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 17);

    // The root pulls g out of the child: compaction re-homes the function
    // and moves its whole scope, then releases the rest of the block.
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(g_node));
    let Value::Function(mapped) = m.evaluate_node_deep(root_node, None) else {
        panic!("expected function")
    };
    assert_eq!(mapped, g_id);
    assert_eq!(m.functions[g_id].block, root); // re-homed to the root
    assert_eq!(m.nodes[seven].block, root); // scope constant moved too
    assert!(!m.blocks.contains_key(child));

    // Still callable after compaction: 3 + 7.
    let three = u128_node(&mut m, root, 3);
    let call2 = call_node(&mut m, root, g_node, three);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 10);
}

#[test]
fn call_clones_are_compacted_with_the_calling_block() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    // g(x) = Add(x, 1) lives at the root level.
    let (g_node, g_ret, _g_param) = function(&mut m, |m, ret, param| {
        let one = u128_node(m, m.nodes[ret].block, 1);
        let operands = array_node(m, m.nodes[ret].block, &[param, one]);
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Add),
            operand: Some(operands),
        });
    });
    m.evaluate_node_deep(g_ret, None); // definition pass

    // The call clones g's body into the child block.
    let five = u128_node(&mut m, child, 5);
    let call = call_node(&mut m, child, g_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 6);
    assert_eq!(m.nodes[call].block, child);

    // Compacting the child moves the call node (with its cached result)
    // into the root; the clone nodes it used are released with the block.
    let root_node = op_node(&mut m, root, Operator::Ext(TestOperator::Id), Some(call));
    assert_eq!(u128_of(m.evaluate_node_deep(root_node, None)), 6);
    assert_eq!(m.nodes[call].block, root);
    assert!(!m.blocks.contains_key(child));

    // The root-level function is untouched and still callable.
    let two = u128_node(&mut m, root, 2);
    let call2 = call_node(&mut m, root, g_node, two);
    assert_eq!(u128_of(m.evaluate_node_deep(call2, None)), 3);
}

#[test]
fn call_return_is_shallow_for_container_bodies() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, Id(x)] — the second element depends on the parameter, so
    // the definition pass leaves it a marker and the call clones it
    // unevaluated rather than forcing it.
    let f = m.add_block(None);
    let ret = m.add_node(f, None, None); // RETURN_IDX
    let param = m.add_node(f, None, Some(Value::Parameterized)); // PARAMETER_IDX
    let id2 = op_node(&mut m, f, Operator::Ext(TestOperator::Id), Some(param));
    let slice = m.blocks[f].arena.alloc_slice_copy(&[param, id2]);
    m.nodes[ret].value = Some(Value::Array(std::ptr::slice_from_raw_parts(
        slice.as_ptr(),
        slice.len(),
    )));
    let (func_node, _) = wrap_function(&mut m, f, ret, param);
    m.evaluate_node_deep(ret, None); // definition pass flags the array parameterized
    assert_eq!(m.nodes[ret].parameterized_deep, Some(true));

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);

    // Shallow evaluation of the call node returns the array without
    // forcing the elements: the Id(x) clone stays unevaluated.
    let value = m.evaluate_node(call, None);
    let ids = array_ids(value);
    assert_eq!(ids.len(), 2);
    assert_eq!(ids[0], arg);
    assert!(m.nodes[ids[1]].value.is_none()); // the Id clone is still lazy

    // Deep evaluation forces them.
    let deep = m.evaluate_node_deep(call, None);
    assert_u128_array(&m, deep, &[42, 42]);
}

#[test]
fn add_equality_merges_equivalence_classes() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 1);
    let b = u128_node(&mut m, root, 2);
    let c = u128_node(&mut m, root, 3);

    // Fresh nodes are their own representatives.
    assert_eq!(m.equality_representative(a), a);
    assert_eq!(m.equality_representative(b), b);

    let rep = m.add_equality(a, b);
    assert_eq!(m.equality_representative(a), rep);
    assert_eq!(m.equality_representative(b), rep);
    assert_ne!(m.equality_representative(c), rep);

    // Equality is transitive: merging b's class with c pulls a in too.
    m.add_equality(b, c);
    assert_eq!(m.equality_representative(a), rep);
    assert_eq!(m.equality_representative(c), rep);
}

#[test]
fn root_node_compresses_deep_paths() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let nodes: Vec<_> = (0..5).map(|i| u128_node(&mut m, root, i as u128)).collect();
    // Build a deliberately deep parent chain 0 <- 1 <- 2 <- 3 <- 4 by hand
    // (union by size would attach every new node directly under the root).
    for pair in nodes.windows(2) {
        m.nodes[pair[1]].equality.parent = Some(pair[0]);
    }

    let rep = m.equality_representative(nodes[4]);

    assert_eq!(rep, nodes[0]);
    // The whole path was flattened onto the representative.
    for &n in &nodes {
        assert_eq!(m.nodes[n].equality.parent, (n != nodes[0]).then_some(nodes[0]));
    }
}

#[test]
fn cloned_function_nodes_start_in_their_own_equality_class() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, ret, _param) = function(&mut m, |m, ret, param| {
        m.nodes[ret].operation = Some(Operation {
            operator: Operator::Ext(TestOperator::Id),
            operand: Some(param),
        });
    });

    let arg = u128_node(&mut m, root, 42);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);

    // A clone is a fresh node, so it starts as a singleton class unrelated
    // to the body's nodes: find the call's clone of ret (the Id op whose
    // operand is the argument) in the call node's block.
    let clone_ret = m.blocks[root]
        .nodes
        .iter()
        .copied()
        .find(|&id| m.nodes[id].operation.is_some_and(|op| op.operand == Some(arg)))
        .expect("the call clone of ret");
    assert_eq!(m.equality_representative(clone_ret), clone_ret);
    assert_ne!(m.equality_representative(clone_ret), m.equality_representative(ret));
}

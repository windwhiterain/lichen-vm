//! The `lowlevel` layer of the `basic` test crate.  The category modules
//! live in the same-name directory `tests/basic/lowlevel/` next to this
//! file, so plain `mod` declarations resolve them without `#[path]`:
//!
//! - `compaction` — block compaction / GC: relocation, hoisting, release
//! - `evaluation` — operators, the cycle guard, visiting/parameterized markers
//! - `function` — `Apply` semantics, nested and higher-order functions
//! - `recursion` — lazy recursion, definition passes, depth guards
//! - `equality` — `unify` and the DSU equivalence classes it binds through
//!
//! The shared harness (the test `Program`/`Value`/`Operator` and the node
//! and function builders) lives here; each category module pulls it in with
//! `use super::*;`.

mod compaction;
mod equality;
mod evaluation;
mod function;
mod recursion;

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

/// Value equality: same payload contents, not pointer identity — a cloned
/// (copied) `Ext` payload must merge with its original in `unify`.
impl PartialEq for TestValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (TestValue::U128(a), TestValue::U128(b)) => unsafe { **a == **b },
            (TestValue::String(a), TestValue::String(b)) => unsafe { **a == **b },
            _ => false,
        }
    }
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
    Sub,
    Concat,
    /// `a == b`, returning `Value::USize(1/0)` so it can drive `Index`.
    Eq,
    /// `a < b`, returning `Value::USize(1/0)` so it can drive `Index`.
    Lt,
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
            TestOperator::Add
            | TestOperator::Sub
            | TestOperator::Concat
            | TestOperator::Eq
            | TestOperator::Lt => {
                if matches!(operand, Value::Parameterized) {
                    return Value::Parameterized;
                }
                let Value::Array(operands) = operand else {
                    unreachable!("binary ops expect an array of two node ids")
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
                    TestOperator::Sub => {
                        let difference = u128_of(left).wrapping_sub(u128_of(right));
                        let p = block.arena.alloc(difference);
                        Value::Ext(TestValue::U128(p as *const u128))
                    }
                    TestOperator::Eq => Value::USize((u128_of(left) == u128_of(right)) as usize),
                    TestOperator::Lt => Value::USize((u128_of(left) < u128_of(right)) as usize),
                    TestOperator::Concat => {
                        let mut result = string_of(left);
                        result.extend(string_of(right));
                        let slice = block.arena.alloc_slice_copy(&result);
                        Value::Ext(TestValue::String(std::ptr::slice_from_raw_parts(
                            slice.as_ptr(),
                            slice.len(),
                        )))
                    }
                    _ => unreachable!("all binary ops are handled above"),
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

/// An unbound cell — `Value::Parameterized`, so deep evaluation stays lazy
/// instead of panicking on a missing operation.
fn unbound_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(Value::Parameterized))
}

fn unit_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(Value::None))
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

/// Register a function whose body lives in `block`: insert the `Function`
/// (scope = the block's node list), register it on the block, and fill the
/// placeholder `func_node` with its value.  Returns the function id.
fn finish_function(
    m: &mut Module<TestProgram>,
    block: BlockId,
    ret: NodeId,
    param: NodeId,
    func_node: NodeId,
) -> FunctionId {
    let nodes = m.blocks[block].nodes.clone();
    let function = m.functions.insert(Function {
        nodes,
        r#return: ret,
        parameter: param,
        block,
    });
    m.blocks[block].functions.push(function);
    m.nodes[func_node].value = Some(Value::Function(function));
    function
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

/// Build a Fibonacci function in its own body block:
/// `fib(x) = if x < 2 then x else fib(x-1) + fib(x-2)`, with the
/// `if/else` expressed as a lazy `Index` branch — `Index([else, then], c)`
/// with `c` a `USize(0/1)` — so the untaken recursive branch is never
/// forced.  Returns the function value node and id.
fn fibonacci(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    // Placeholder for the function's own value node (the operand arrays
    // reference it before the function exists).
    let fib_func = m.add_node(body, None, None);
    let one = u128_node(m, body, 1);
    let two = u128_node(m, body, 2);
    // fib(x-1)
    let sub1_ops = array_node(m, body, &[param, one]);
    let sub1 = op_node(m, body, Operator::Ext(TestOperator::Sub), Some(sub1_ops));
    let fib1_ops = array_node(m, body, &[fib_func, sub1]);
    let fib1 = op_node(m, body, Operator::Apply, Some(fib1_ops));
    // fib(x-2)
    let sub2_ops = array_node(m, body, &[param, two]);
    let sub2 = op_node(m, body, Operator::Ext(TestOperator::Sub), Some(sub2_ops));
    let fib2_ops = array_node(m, body, &[fib_func, sub2]);
    let fib2 = op_node(m, body, Operator::Apply, Some(fib2_ops));
    // rec = fib(x-1) + fib(x-2)
    let rec_ops = array_node(m, body, &[fib1, fib2]);
    let rec = op_node(m, body, Operator::Ext(TestOperator::Add), Some(rec_ops));
    // ret = if x < 2 then x else rec
    let lt_ops = array_node(m, body, &[param, two]);
    let lt = op_node(m, body, Operator::Ext(TestOperator::Lt), Some(lt_ops));
    let branch = array_node(m, body, &[rec, param]);
    let index_ops = array_node(m, body, &[branch, lt]);
    let ret = op_node(m, body, Operator::Index, Some(index_ops));
    let function = finish_function(m, body, ret, param, fib_func);
    (fib_func, function)
}

/// Build `f(x) = Apply(f, x)` — an unconditional self-application with no
/// base case, so any evaluation of a call never returns.
fn unconditional_self_apply(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(Value::Parameterized));
    let func_node = m.add_node(body, None, None); // placeholder self-ref
    let operands = array_node(m, body, &[func_node, param]);
    let ret = op_node(m, body, Operator::Apply, Some(operands));
    let function = finish_function(m, body, ret, param, func_node);
    (func_node, function)
}

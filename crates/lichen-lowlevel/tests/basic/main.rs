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

use lichen_lowlevel::{
    ArrayRef, BlockId, Function, FunctionId, Handle, LowValue, Module, NodeId, Operation,
    OperatorExt, Program, ValueExt,
};
use lichen_utils::extend::AsEnum;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TestProgram;

/// The test harness's global extension state — empty; the lowlevel's
/// extension operators here (arithmetic, string ops) keep no state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TestGlobalExt;

impl Program for TestProgram {
    type Value = TestValue;
    type Operator = TestOperator;
    type GlobalExt = TestGlobalExt;
}

// The test value vocabulary: the lowlevel structural values spliced in
// from [`LowValue`], plus the handle-carrying extension values below.  The
// arena machinery only deals in byte views, so `ValueExt` converts
// between the typed pointer and a byte slice on the fly.
lichen_lowlevel::extend_LowValue! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum TestValue {
        /// A `u128` payload (16 bytes).
        U128(Handle<u128>),
        /// A `char` payload, four bytes per char.
        String(Handle<[char]>),
    }
}

impl ValueExt for TestValue {
    fn is_handle(&self) -> bool {
        matches!(self, TestValue::U128(_) | TestValue::String(_))
    }
    // `U128` requires 16-byte alignment; returning the strictest
    // alignment keeps the `String` copies over-aligned, which is safe.
    fn alignment() -> usize {
        16
    }
    fn handle(&self) -> Handle<[u8]> {
        match self {
            TestValue::U128(p) => Handle(std::ptr::slice_from_raw_parts(p.0 as *const u8, 16)),
            TestValue::String(p) => {
                let chars = unsafe { &*p.0 };
                Handle(std::ptr::slice_from_raw_parts(
                    chars.as_ptr() as *const u8,
                    chars.len() * 4,
                ))
            }
            _ => unreachable!("only the handle variants carry a payload"),
        }
    }
    fn set_handle(&mut self, payload: Handle<[u8]>) {
        match self {
            TestValue::U128(p) => p.0 = payload.0 as *const u128,
            TestValue::String(p) => {
                p.0 = std::ptr::slice_from_raw_parts(payload.0 as *const char, payload.len() / 4)
            }
            _ => unreachable!("only the handle variants carry a payload"),
        }
    }
}

// The test operator vocabulary: the lowlevel structural operators spliced
// in from [`LowOperator`], plus the test operators below.
lichen_lowlevel::extend_LowOperator! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum TestOperator {
        /// Pass the operand through untouched.
        Id,
        Add,
        Sub,
        Concat,
        /// `a == b`, returning `TestValue::USize(1/0)` so it can drive `Index`.
        Eq,
        /// `a < b`, returning `TestValue::USize(1/0)` so it can drive `Index`.
        Lt,
    }
}

impl OperatorExt<TestProgram> for TestOperator {
    fn run(
        &self,
        operand: TestValue,
        block: BlockId,
        module: &mut Module<TestProgram>,
    ) -> TestValue {
        match self {
            // The structural operators never reach `run`: the VM dispatches
            // them through `AsEnum` before falling through.
            TestOperator::Index | TestOperator::Apply => {
                unreachable!("structural operators are dispatched by the VM")
            }
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
                if matches!(operand.as_enum(), Some(LowValue::Parameterized)) {
                    return TestValue::Parameterized;
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary ops expect an array of two node ids")
                };
                let operands = operands.ids();
                let left = module.nodes[operands[0]].value.unwrap();
                let right = module.nodes[operands[1]].value.unwrap();
                match self {
                    TestOperator::Add => {
                        let sum = u128_of(left).wrapping_add(u128_of(right));
                        let p = module.blocks[block].arena.alloc(sum);
                        TestValue::U128(Handle(p as *const u128))
                    }
                    TestOperator::Sub => {
                        let difference = u128_of(left).wrapping_sub(u128_of(right));
                        let p = module.blocks[block].arena.alloc(difference);
                        TestValue::U128(Handle(p as *const u128))
                    }
                    TestOperator::Eq => {
                        TestValue::USize((u128_of(left) == u128_of(right)) as usize)
                    }
                    TestOperator::Lt => TestValue::USize((u128_of(left) < u128_of(right)) as usize),
                    TestOperator::Concat => {
                        let mut result = string_of(left);
                        result.extend(string_of(right));
                        let slice = module.blocks[block].arena.alloc_slice_copy(&result);
                        TestValue::String(Handle(std::ptr::slice_from_raw_parts(
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

fn u128_of(value: TestValue) -> u128 {
    let TestValue::U128(payload) = value else {
        panic!("expected U128")
    };
    u128::from_ne_bytes(
        unsafe { std::slice::from_raw_parts(payload.0 as *const u8, 16) }
            .try_into()
            .unwrap(),
    )
}

fn string_of(value: TestValue) -> Vec<char> {
    let TestValue::String(payload) = value else {
        panic!("expected String")
    };
    unsafe { &*payload.0 }.to_vec()
}

// --- builders ---------------------------------------------------------

fn u128_node(m: &mut Module<TestProgram>, block: BlockId, n: u128) -> NodeId {
    let p = m.blocks[block].arena.alloc(n);
    m.add_node(block, None, Some(TestValue::U128(Handle(p as *const u128))))
}

fn str_node(m: &mut Module<TestProgram>, block: BlockId, chars: &[char]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(chars);
    m.add_node(
        block,
        None,
        Some(TestValue::String(Handle(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        )))),
    )
}

fn array_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    ids: &[NodeId],
    mask: Option<&[bool]>,
) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(ids);
    let ids_ptr = std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len());
    let shallow = match mask {
        Some(mask) if mask.iter().any(|&marked| marked) => {
            let slice = m.blocks[block].arena.alloc_slice_copy(mask);
            std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len())
        }
        _ => std::ptr::slice_from_raw_parts(std::ptr::null(), 0),
    };
    m.add_node(
        block,
        None,
        Some(TestValue::Array(ArrayRef { ids: ids_ptr, shallow })),
    )
}

fn usize_node(m: &mut Module<TestProgram>, block: BlockId, n: usize) -> NodeId {
    m.add_node(block, None, Some(TestValue::USize(n)))
}

/// An unbound cell — `TestValue::Parameterized`, so deep evaluation stays
/// lazy instead of panicking on a missing operation.
fn unbound_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(TestValue::Parameterized))
}

fn unit_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(TestValue::None))
}

fn op_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    operator: TestOperator,
    operand: Option<NodeId>,
) -> NodeId {
    m.add_node(block, Some(Operation { operator, operand }), None)
}

/// The node ids inside `value` if it's an array.
fn array_ids(value: TestValue) -> &'static [NodeId] {
    let TestValue::Array(array) = value else {
        panic!("expected array")
    };
    array.ids()
}

/// Assert `value` is an array whose elements hold the given `u128`s.
fn assert_u128_array(m: &Module<TestProgram>, value: TestValue, expected: &[u128]) {
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
    let func_node = m.add_function(block, ret, param, nodes);
    let TestValue::Function(func) = m.nodes[func_node].value.unwrap() else {
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
    let param = m.add_node(block, None, Some(TestValue::Parameterized));
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
    let operands = array_node(m, block, &[func_node, arg], None);
    op_node(m, block, TestOperator::Apply, Some(operands))
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
    let nodes: std::collections::HashSet<NodeId> = m.blocks[block].nodes.iter().copied().collect();
    let function = m.functions.insert(Function {
        nodes,
        r#return: ret,
        parameter: param,
        block,
    });
    m.blocks[block].functions.push(function);
    m.nodes[func_node].value = Some(TestValue::Function(function));
    function
}

/// Build a self-referential function `f(x) = [x, f(x)]` in its own body
/// block: the Apply operand array references the function's own value node,
/// so each application of `f` produces one recursion level.  Returns the
/// function value node and id.
fn recursive_function(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    // Placeholder for the function's own value node: the operand array must
    // reference it before the function exists, so `add_function` (which
    // creates the value node last) cannot be used here.
    let func_node = m.add_node(body, None, None);
    let operands = array_node(m, body, &[func_node, param], None);
    let apply = op_node(m, body, TestOperator::Apply, Some(operands));
    let ret = array_node(m, body, &[param, apply], None);
    let function = m.functions.insert(Function {
        nodes: HashSet::from([param, func_node, operands, apply, ret]),
        r#return: ret,
        parameter: param,
        block: body,
    });
    m.blocks[body].functions.push(function);
    m.nodes[func_node].value = Some(TestValue::Function(function));
    (func_node, function)
}

/// Build two functions calling each other: `f(x) = [x, g(x)]` and
/// `g(x) = [x, f(x)]`, sharing one body block.  Returns the two function
/// value nodes.
fn mutually_recursive_functions(m: &mut Module<TestProgram>) -> (NodeId, NodeId) {
    let body = m.add_block(None);
    let f_param = m.add_node(body, None, Some(TestValue::Parameterized));
    let g_param = m.add_node(body, None, Some(TestValue::Parameterized));
    // Both value nodes are placeholders: f's body references g before g
    // exists, and vice versa.
    let f_func = m.add_node(body, None, None);
    let g_func = m.add_node(body, None, None);
    // f(x) = [x, g(x)]
    let f_ops = array_node(m, body, &[g_func, f_param], None);
    let f_apply = op_node(m, body, TestOperator::Apply, Some(f_ops));
    let f_ret = array_node(m, body, &[f_param, f_apply], None);
    // g(x) = [x, f(x)]
    let g_ops = array_node(m, body, &[f_func, g_param], None);
    let g_apply = op_node(m, body, TestOperator::Apply, Some(g_ops));
    let g_ret = array_node(m, body, &[g_param, g_apply], None);
    let f = m.functions.insert(Function {
        nodes: HashSet::from([f_param, f_func, f_ops, f_apply, f_ret]),
        r#return: f_ret,
        parameter: f_param,
        block: body,
    });
    let g = m.functions.insert(Function {
        nodes: HashSet::from([g_param, g_func, g_ops, g_apply, g_ret]),
        r#return: g_ret,
        parameter: g_param,
        block: body,
    });
    m.blocks[body].functions.extend([f, g]);
    m.nodes[f_func].value = Some(TestValue::Function(f));
    m.nodes[g_func].value = Some(TestValue::Function(g));
    (f_func, g_func)
}

/// Build a Fibonacci function in its own body block:
/// `fib(x) = if x < 2 then x else fib(x-1) + fib(x-2)`, with the
/// `if/else` expressed as a lazy `Index` branch — `Index([else, then], c)`
/// with `c` a `USize(0/1)` — so the untaken recursive branch is never
/// forced.  Returns the function value node and id.
fn fibonacci(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    // Placeholder for the function's own value node (the operand arrays
    // reference it before the function exists).
    let fib_func = m.add_node(body, None, None);
    let one = u128_node(m, body, 1);
    let two = u128_node(m, body, 2);
    // fib(x-1)
    let sub1_ops = array_node(m, body, &[param, one], None);
    let sub1 = op_node(m, body, TestOperator::Sub, Some(sub1_ops));
    let fib1_ops = array_node(m, body, &[fib_func, sub1], None);
    let fib1 = op_node(m, body, TestOperator::Apply, Some(fib1_ops));
    // fib(x-2)
    let sub2_ops = array_node(m, body, &[param, two], None);
    let sub2 = op_node(m, body, TestOperator::Sub, Some(sub2_ops));
    let fib2_ops = array_node(m, body, &[fib_func, sub2], None);
    let fib2 = op_node(m, body, TestOperator::Apply, Some(fib2_ops));
    // rec = fib(x-1) + fib(x-2)
    let rec_ops = array_node(m, body, &[fib1, fib2], None);
    let rec = op_node(m, body, TestOperator::Add, Some(rec_ops));
    // ret = if x < 2 then x else rec
    let lt_ops = array_node(m, body, &[param, two], None);
    let lt = op_node(m, body, TestOperator::Lt, Some(lt_ops));
    let branch = array_node(m, body, &[rec, param], None);
    let index_ops = array_node(m, body, &[branch, lt], None);
    let ret = op_node(m, body, TestOperator::Index, Some(index_ops));
    let function = finish_function(m, body, ret, param, fib_func);
    (fib_func, function)
}

/// Build `f(x) = Apply(f, x)` — an unconditional self-application with no
/// base case, so any evaluation of a call never returns.
fn unconditional_self_apply(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(TestValue::Parameterized));
    let func_node = m.add_node(body, None, None); // placeholder self-ref
    let operands = array_node(m, body, &[func_node, param], None);
    let ret = op_node(m, body, TestOperator::Apply, Some(operands));
    let function = finish_function(m, body, ret, param, func_node);
    (func_node, function)
}

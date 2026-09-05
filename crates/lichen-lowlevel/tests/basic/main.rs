//! The `lowlevel` layer of the `basic` test crate.  The category modules
//! live in the same-name directory `tests/basic/lowlevel/` next to this
//! file, so plain `mod` declarations resolve them without `#[path]`:
//!
//! - `compaction` — block compaction / GC: relocation, hoisting, release
//! - `evaluation` — operators, the cycle guard, visiting/evaluated_deep markers
//! - `function` — `Apply` semantics, nested and higher-order functions
//! - `recursion` — lazy recursion, definition passes, depth guards
//! - `equality` — `unify` and the DSU equivalence classes it binds through
//! - `assert` — assert points, forced evaluation, clone-on-apply
//! - `table` — constant table values, deep-content keys, `TableGet` reads
//!
//! The shared harness (the test `Program`/`Value`/`Operator` and the node
//! and function builders) lives here; each category module pulls it in with
//! `use super::*;`.

mod assert;
mod compaction;
mod equality;
mod evaluation;
mod function;
mod recursion;
mod static_module;
mod table;

use lichen_lowlevel::{
    AnyFunctionId, AnyHandle, AnyNodeId, ArrayItem, BlockId, EvalError, EvaluatedDeep, Function,
    FunctionId, GlobalExt, Handle, LowOperator, LowValue, Module, NodeId, Operation, OperatorExt,
    Program, StaticHandle, ValueExt,
};
use lichen_utils::extend::AsEnum;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq)]
struct TestProgram;

/// The test harness's global extension state — empty; the lowlevel's
/// extension operators here (arithmetic, string ops) keep no state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct TestGlobalExt;

impl GlobalExt for TestGlobalExt {}

impl Program for TestProgram {
    type Value = TestValue;
    type Operator = TestOperator;
    type GlobalExt = TestGlobalExt;
    type PackageMeta = ();
}

// The test value vocabulary: the handle-carrying extension values below,
// with the lowlevel structural values carried whole as one variant.  The
// arena machinery only deals in byte views, so `ValueExt` converts
// between the typed pointer and a byte slice on the fly.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum TestValue {
        /// A `u128` payload (16 bytes).
        U128(AnyHandle<u128>),
        /// A `char` payload, four bytes per char.
        String(AnyHandle<[char]>),
    }
    + LowValue;
}

/// A dynamic (block-arena) handle — the kind every hand-built test value
/// uses; static payloads appear only inside `StaticModule`s.
fn dyn_handle<T: ?Sized>(ptr: *const T) -> AnyHandle<T> {
    AnyHandle::Dynamic(Handle(ptr))
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
    fn handle(&self) -> AnyHandle<[u8]> {
        match self {
            TestValue::U128(p) => match p {
                AnyHandle::Dynamic(h) => {
                    AnyHandle::Dynamic(Handle(std::ptr::slice_from_raw_parts(h.0 as *const u8, 16)))
                }
                AnyHandle::Static(h) => AnyHandle::Static(StaticHandle {
                    module: h.module,
                    offset: std::ptr::slice_from_raw_parts(h.offset as *const u8, 16),
                }),
            },
            TestValue::String(p) => match p {
                AnyHandle::Dynamic(h) => {
                    let chars = unsafe { &*h.0 };
                    AnyHandle::Dynamic(Handle(std::ptr::slice_from_raw_parts(
                        chars.as_ptr() as *const u8,
                        chars.len() * 4,
                    )))
                }
                AnyHandle::Static(h) => {
                    let chars = unsafe { &*h.offset };
                    AnyHandle::Static(StaticHandle {
                        module: h.module,
                        offset: std::ptr::slice_from_raw_parts(
                            chars.as_ptr() as *const u8,
                            chars.len() * 4,
                        ),
                    })
                }
            },
            _ => unreachable!("only the handle variants carry a payload"),
        }
    }
    fn set_handle(&mut self, payload: AnyHandle<[u8]>) {
        match self {
            TestValue::U128(p) => {
                *p = match payload {
                    AnyHandle::Dynamic(h) => AnyHandle::Dynamic(Handle(h.0 as *const u128)),
                    AnyHandle::Static(h) => AnyHandle::Static(StaticHandle {
                        module: h.module,
                        offset: h.offset as *const u128,
                    }),
                }
            }
            TestValue::String(p) => {
                *p = match payload {
                    AnyHandle::Dynamic(h) => AnyHandle::Dynamic(Handle(
                        std::ptr::slice_from_raw_parts(h.0 as *const char, h.len() / 4),
                    )),
                    AnyHandle::Static(h) => AnyHandle::Static(StaticHandle {
                        module: h.module,
                        offset: std::ptr::slice_from_raw_parts(
                            h.offset as *const char,
                            h.len() / 4,
                        ),
                    }),
                }
            }
            _ => unreachable!("only the handle variants carry a payload"),
        }
    }
}

// The test operator vocabulary: the test operators below, with the lowlevel
// structural operators carried whole as one variant.
lichen_utils::enum_ext! {
    #[derive(Debug, Clone, Copy, PartialEq)]
    enum TestOperator {
        /// Pass the operand through untouched.
        Id,
        Add,
        Sub,
        Concat,
        /// `a == b`, returning `TestValue::LowValue(LowValue::USize(1/0))` so it can drive `Index`.
        Eq,
        /// `a < b`, returning `TestValue::LowValue(LowValue::USize(1/0))` so it can drive `Index`.
        Lt,
    }
    + LowOperator;
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
            TestOperator::LowOperator(LowOperator::Index)
            | TestOperator::LowOperator(LowOperator::Apply)
            | TestOperator::LowOperator(LowOperator::TableGet) => {
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
                    return TestValue::LowValue(LowValue::Parameterized);
                }
                let Some(LowValue::Array(operands)) = operand.as_enum() else {
                    unreachable!("binary ops expect an array of two node ids")
                };
                let operands = operands.items();
                // Operands may be baked static refs (a constant operand of a
                // materialized static function) — resolve through the module
                // API, which reads a dynamic node's value or a static node's
                // solved value.
                let left = module.node_value(operands[0].node).unwrap();
                let right = module.node_value(operands[1].node).unwrap();
                match self {
                    TestOperator::Add => {
                        let sum = u128_of(left).wrapping_add(u128_of(right));
                        let p = module.blocks[block].arena.alloc(sum);
                        TestValue::U128(dyn_handle(p as *const u128))
                    }
                    TestOperator::Sub => {
                        let difference = u128_of(left).wrapping_sub(u128_of(right));
                        let p = module.blocks[block].arena.alloc(difference);
                        TestValue::U128(dyn_handle(p as *const u128))
                    }
                    TestOperator::Eq => TestValue::LowValue(LowValue::USize(
                        (u128_of(left) == u128_of(right)) as usize,
                    )),
                    TestOperator::Lt => TestValue::LowValue(LowValue::USize(
                        (u128_of(left) < u128_of(right)) as usize,
                    )),
                    TestOperator::Concat => {
                        let mut result = string_of(left);
                        result.extend(string_of(right));
                        let slice = module.blocks[block].arena.alloc_slice_copy(&result);
                        TestValue::String(dyn_handle(std::ptr::slice_from_raw_parts(
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
    let ptr = match payload {
        AnyHandle::Dynamic(h) => h.0 as *const u8,
        AnyHandle::Static(h) => h.offset as *const u8,
    };
    u128::from_ne_bytes(
        unsafe { std::slice::from_raw_parts(ptr, 16) }
            .try_into()
            .unwrap(),
    )
}

fn string_of(value: TestValue) -> Vec<char> {
    let TestValue::String(payload) = value else {
        panic!("expected String")
    };
    match payload {
        AnyHandle::Dynamic(h) => unsafe { &*h.0 }.to_vec(),
        AnyHandle::Static(h) => unsafe { &*h.offset }.to_vec(),
    }
}

// --- builders ---------------------------------------------------------

/// Unwrap a function value known to be dynamic (all test-built values).
fn dyn_function(value: TestValue) -> FunctionId {
    let TestValue::LowValue(LowValue::Function(func)) = value else {
        panic!("expected a function value")
    };
    let AnyFunctionId::Dynamic(func) = func else {
        panic!("expected a dynamic function")
    };
    func
}

/// An unmarked array item referencing a dynamic node.
fn item(node: NodeId) -> ArrayItem {
    ArrayItem::new(AnyNodeId::Dynamic(node))
}

fn u128_node(m: &mut Module<TestProgram>, block: BlockId, n: u128) -> NodeId {
    let p = m.blocks[block].arena.alloc(n);
    m.add_node(
        block,
        None,
        Some(TestValue::U128(dyn_handle(p as *const u128))),
    )
}

fn str_node(m: &mut Module<TestProgram>, block: BlockId, chars: &[char]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(chars);
    m.add_node(
        block,
        None,
        Some(TestValue::String(dyn_handle(
            std::ptr::slice_from_raw_parts(slice.as_ptr(), slice.len()),
        ))),
    )
}

fn array_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    ids: &[NodeId],
    mask: Option<&[bool]>,
) -> NodeId {
    let items: Vec<ArrayItem> = ids
        .iter()
        .enumerate()
        .map(|(i, &node)| ArrayItem {
            node: lichen_lowlevel::AnyNodeId::Dynamic(node),
            shallow: mask.is_some_and(|mask| mask[i]),
        })
        .collect();
    m.add_node(
        block,
        None,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, block),
        ))),
    )
}

fn usize_node(m: &mut Module<TestProgram>, block: BlockId, n: usize) -> NodeId {
    m.add_node(block, None, Some(TestValue::LowValue(LowValue::USize(n))))
}

/// An unbound cell — `TestValue::LowValue(LowValue::Parameterized)`, so deep evaluation stays
/// lazy instead of panicking on a missing operation.
fn unbound_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(
        block,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    )
}

fn unit_node(m: &mut Module<TestProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(TestValue::LowValue(LowValue::None)))
}

fn op_node(
    m: &mut Module<TestProgram>,
    block: BlockId,
    operator: TestOperator,
    operand: Option<NodeId>,
) -> NodeId {
    m.add_node(block, Some(Operation { operator, operand }), None)
}

/// The node ids inside `value` if it's an array (test arrays are dynamic).
fn array_ids(value: TestValue) -> Vec<NodeId> {
    let TestValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected array")
    };
    array
        .items()
        .iter()
        .map(|item| match item.node {
            AnyNodeId::Dynamic(node) => node,
            AnyNodeId::Static(_) => panic!("expected a dynamic element"),
        })
        .collect()
}

/// The shallow flags inside `value` if it's an array.
fn array_mask(value: TestValue) -> Vec<bool> {
    let TestValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected array")
    };
    array.items().iter().map(|item| item.shallow).collect()
}

/// Assert `value` is an array whose elements hold the given `u128`s.
fn assert_u128_array(m: &Module<TestProgram>, value: TestValue, expected: &[u128]) {
    let ids = array_ids(value);
    assert_eq!(ids.len(), expected.len());
    for (&id, &n) in ids.iter().zip(expected) {
        assert_eq!(u128_of(m.node_value(AnyNodeId::Dynamic(id)).unwrap()), n);
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
    wrap_function_asserts(m, block, ret, param, [])
}

/// [`wrap_function`] with the function's own assert conditions (`Function::asserts`).
fn wrap_function_asserts(
    m: &mut Module<TestProgram>,
    block: BlockId,
    ret: NodeId,
    param: NodeId,
    asserts: impl IntoIterator<Item = NodeId>,
) -> (NodeId, FunctionId) {
    let nodes = m.blocks[block].nodes.clone();
    let func_node = m.add_function(block, ret, param, nodes, asserts);
    let TestValue::LowValue(LowValue::Function(func)) =
        m.node_value(AnyNodeId::Dynamic(func_node)).unwrap()
    else {
        unreachable!("add_function always wraps a function value")
    };
    let AnyFunctionId::Dynamic(func) = func else {
        unreachable!("add_function always wraps a dynamic function")
    };
    (func_node, func)
}

/// The manual-insert mirror of [`Module::add_function`]'s tagging: stamp
/// `nodes` as owned by `function` and record them as its scope.  A function
/// built by hand (the placeholder-value helpers below) must tag its body,
/// or the apply clone walk's chain membership test reads the body as
/// outside the template and references it in place.
fn tag_scope(m: &mut Module<TestProgram>, function: FunctionId, nodes: Vec<NodeId>) {
    for &node in &nodes {
        m.nodes[node].function = Some(function);
    }
    m.functions[function].nodes = nodes;
}

/// Create a function value in a fresh body block: the return node at
/// `Block::RETURN_IDX` and the parameter at index 1 of the scope, wrapped
/// by [`Module::add_function`]. `wire` fills in the return node given the
/// parameter id.  Asserts registered during `wire` become the function's
/// own (`Function::asserts`), so an apply re-checks them per call.  Returns
/// the function value node plus the return and parameter node ids.
fn function(
    m: &mut Module<TestProgram>,
    wire: impl FnOnce(&mut Module<TestProgram>, NodeId, NodeId),
) -> (NodeId, NodeId, NodeId) {
    let block = m.add_block(None);
    let ret = m.add_node(block, None, None);
    let param = m.add_node(
        block,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let asserts_before = m.asserts.len();
    wire(m, ret, param);
    let asserts = m.asserts[asserts_before..].to_vec();
    let (func_node, _) = wrap_function_asserts(m, block, ret, param, asserts);
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
    op_node(
        m,
        block,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(operands),
    )
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
    let nodes: Vec<NodeId> = m.blocks[block].nodes.iter().copied().collect();
    let function = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return: ret,
        parameter: param,
        asserts: Vec::new(),
        parent: None,
        block,
    });
    tag_scope(m, function, nodes);
    m.blocks[block].functions.push(function);
    m.write_node_value(
        func_node,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(function),
        ))),
    );
    function
}

/// Build a self-referential function `f(x) = [x, f(x)]` in its own body
/// block: the Apply operand array references the function's own value node,
/// so each application of `f` produces one recursion level.  Returns the
/// function value node and id.
fn recursive_function(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    // Placeholder for the function's own value node: the operand array must
    // reference it before the function exists, so `add_function` (which
    // creates the value node last) cannot be used here.
    let func_node = m.add_node(body, None, None);
    let operands = array_node(m, body, &[func_node, param], None);
    let apply = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(operands),
    );
    let ret = array_node(m, body, &[param, apply], None);
    let function = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return: ret,
        parameter: param,
        asserts: Vec::new(),
        parent: None,
        block: body,
    });
    tag_scope(m, function, vec![param, func_node, operands, apply, ret]);
    m.blocks[body].functions.push(function);
    m.write_node_value(
        func_node,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(function),
        ))),
    );
    (func_node, function)
}

/// Build two functions calling each other: `f(x) = [x, g(x)]` and
/// `g(x) = [x, f(x)]`, sharing one body block.  Returns the two function
/// value nodes.
fn mutually_recursive_functions(m: &mut Module<TestProgram>) -> (NodeId, NodeId) {
    let body = m.add_block(None);
    let f_param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let g_param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    // Both value nodes are placeholders: f's body references g before g
    // exists, and vice versa.
    let f_func = m.add_node(body, None, None);
    let g_func = m.add_node(body, None, None);
    // f(x) = [x, g(x)]
    let f_ops = array_node(m, body, &[g_func, f_param], None);
    let f_apply = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(f_ops),
    );
    let f_ret = array_node(m, body, &[f_param, f_apply], None);
    // g(x) = [x, f(x)]
    let g_ops = array_node(m, body, &[f_func, g_param], None);
    let g_apply = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(g_ops),
    );
    let g_ret = array_node(m, body, &[g_param, g_apply], None);
    let f = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return: f_ret,
        parameter: f_param,
        asserts: Vec::new(),
        parent: None,
        block: body,
    });
    tag_scope(m, f, vec![f_param, f_func, f_ops, f_apply, f_ret]);
    let g = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return: g_ret,
        parameter: g_param,
        asserts: Vec::new(),
        parent: None,
        block: body,
    });
    tag_scope(m, g, vec![g_param, g_func, g_ops, g_apply, g_ret]);
    m.blocks[body].functions.extend([f, g]);
    m.write_node_value(
        f_func,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(f),
        ))),
    );
    m.write_node_value(
        g_func,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(g),
        ))),
    );
    (f_func, g_func)
}

/// Build a Fibonacci function in its own body block:
/// `fib(x) = if x < 2 then x else fib(x-1) + fib(x-2)`, with the
/// `if/else` expressed as a lazy `Index` branch — `Index([else, then], c)`
/// with `c` a `USize(0/1)` — so the untaken recursive branch is never
/// forced.  Returns the function value node and id.
fn fibonacci(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    // Placeholder for the function's own value node (the operand arrays
    // reference it before the function exists).
    let fib_func = m.add_node(body, None, None);
    let one = u128_node(m, body, 1);
    let two = u128_node(m, body, 2);
    // fib(x-1)
    let sub1_ops = array_node(m, body, &[param, one], None);
    let sub1 = op_node(m, body, TestOperator::Sub, Some(sub1_ops));
    let fib1_ops = array_node(m, body, &[fib_func, sub1], None);
    let fib1 = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(fib1_ops),
    );
    // fib(x-2)
    let sub2_ops = array_node(m, body, &[param, two], None);
    let sub2 = op_node(m, body, TestOperator::Sub, Some(sub2_ops));
    let fib2_ops = array_node(m, body, &[fib_func, sub2], None);
    let fib2 = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(fib2_ops),
    );
    // rec = fib(x-1) + fib(x-2)
    let rec_ops = array_node(m, body, &[fib1, fib2], None);
    let rec = op_node(m, body, TestOperator::Add, Some(rec_ops));
    // ret = if x < 2 then x else rec
    let lt_ops = array_node(m, body, &[param, two], None);
    let lt = op_node(m, body, TestOperator::Lt, Some(lt_ops));
    let branch = array_node(m, body, &[rec, param], None);
    let index_ops = array_node(m, body, &[branch, lt], None);
    let ret = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Index),
        Some(index_ops),
    );
    let function = finish_function(m, body, ret, param, fib_func);
    (fib_func, function)
}

/// Build `f(x) = Apply(f, x)` — an unconditional self-application with no
/// base case, so any evaluation of a call never returns.
fn unconditional_self_apply(m: &mut Module<TestProgram>) -> (NodeId, FunctionId) {
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let func_node = m.add_node(body, None, None); // placeholder self-ref
    let operands = array_node(m, body, &[func_node, param], None);
    let ret = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(operands),
    );
    let function = finish_function(m, body, ret, param, func_node);
    (func_node, function)
}

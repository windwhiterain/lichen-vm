//! The `@loop` conversion's verdicts and the facts a convertible recursion
//! yields (`lichen_lowlevel::loop_conversion`). The shapes here are hand-built
//! so the verdict is the only thing under test; the checker-composed templates
//! (and their tuple states) are pinned in `lichen-highlevel`'s `loop_marker`.

use super::*;
use lichen_lowlevel::{LoopArm, LoopRefusal};

/// `count n = if n < 1 then n else count (n - 1)`, marked `@loop`: the
/// convertible shape — one tail self-application, one base, a scalar state.
/// Returns the value node, the id, and the nodes the conversion must name.
fn marked_countdown(m: &mut Module<TestProgram>) -> (NodeId, FunctionId, NodeId, NodeId, NodeId) {
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let func_node = m.add_node(body, None, None);
    let one = u128_node(m, body, 1);
    let decrement_ops = array_node(m, body, &[param, one], None);
    let decrement = op_node(m, body, TestOperator::Sub, Some(decrement_ops));
    let call_ops = array_node(m, body, &[func_node, decrement], None);
    let call = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(call_ops),
    );
    let condition_ops = array_node(m, body, &[param, one], None);
    let condition = op_node(m, body, TestOperator::Lt, Some(condition_ops));
    // `[count (n - 1), n][n < 1]` — element 1 answers `1`, which is the base.
    let branches = array_node(m, body, &[call, param], None);
    let index_ops = array_node(m, body, &[branches, condition], None);
    let ret = op_node(
        m,
        body,
        TestOperator::LowOperator(LowOperator::Index),
        Some(index_ops),
    );
    let function = finish_function(m, body, ret, param, func_node);
    m.mark_looping(function);
    (func_node, function, condition, decrement, param)
}

#[test]
fn a_marked_tail_recursion_converts() {
    let mut m = Module::new();
    let (_func_node, function, condition, decrement, param) = marked_countdown(&mut m);
    let conversion = m
        .loop_conversion(function)
        .expect("a tail self-application with a base converts");
    // `count`'s parameter is a bare cell (no `[value, type]` pair): the whole
    // value, one slot.
    assert_eq!(conversion.state, vec![Vec::<usize>::new()]);
    assert_eq!(conversion.tests.len(), 1);
    assert_eq!(conversion.tests[0].condition, condition);
    assert_eq!(conversion.tests[0].on_one, LoopArm::Exit(0));
    assert_eq!(conversion.tests[0].on_zero, LoopArm::Step(0));
    assert_eq!(conversion.steps.len(), 1);
    assert_eq!(conversion.steps[0].next, vec![decrement]);
    assert_eq!(conversion.exits.len(), 1);
    assert_eq!(conversion.exits[0].values, vec![param]);
}

#[test]
fn a_recursion_whose_call_is_not_in_tail_position_refuses() {
    // `fib(x) = if x < 2 then x else fib(x-1) + fib(x-2)`: both calls are
    // operands of the `Add`, so neither is a branch of the spine.
    let mut m = Module::new();
    let (_func_node, function) = fibonacci(&mut m);
    m.mark_looping(function);
    assert_eq!(m.loop_conversion(function), Err(LoopRefusal::NonTailCall));
}

#[test]
fn a_recursion_in_a_tuple_element_refuses() {
    // `f(x) = [x, f(x)]`: the self-application is one element of a
    // tuple-bodied return, not a branch of it. The return states its
    // `[value, type]` pair the way every compiled function does, so the
    // tuple — not the pair's first element — is what the spine reads.
    let mut m: Module<TestProgram> = Module::new();
    let body = m.add_block(None);
    let param = m.add_node(
        body,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let func_node = m.add_node(body, None, None);
    let call_ops = array_node(&mut m, body, &[func_node, param], None);
    let call = op_node(
        &mut m,
        body,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(call_ops),
    );
    let tuple = array_node(&mut m, body, &[param, call], None);
    let type_node = m.add_node(body, None, None);
    let ret = array_node(&mut m, body, &[tuple, type_node], None);
    let function = finish_function(&mut m, body, ret, param, func_node);
    m.mark_looping(function);
    assert_eq!(m.loop_conversion(function), Err(LoopRefusal::NonTailCall));
}

#[test]
fn a_recursion_without_a_base_case_refuses() {
    // `f(x) = Apply(f, x)`: a step and no exit — the loop would never return.
    let mut m = Module::new();
    let (_func_node, function) = unconditional_self_apply(&mut m);
    m.mark_looping(function);
    assert_eq!(m.loop_conversion(function), Err(LoopRefusal::NoBaseCase));
}

#[test]
fn a_mutual_recursion_refuses_at_the_component_cap() {
    // `f(x) = [x, g(x)]` and `g(x) = [x, f(x)]`: a two-member marked
    // component, which the conversion does not cover.
    let mut m = Module::new();
    let (_f_func, _g_func) = mutually_recursive_functions(&mut m);
    let functions: Vec<FunctionId> = m.functions.keys().collect();
    for &function in &functions {
        m.mark_looping(function);
    }
    assert_eq!(
        m.loop_conversion(functions[0]),
        Err(LoopRefusal::MutualComponent { size: 2 })
    );
}

#[test]
fn a_non_recursive_function_refuses() {
    let mut m = Module::new();
    let (func_node, _ret, _param) = function(&mut m, |_m, _ret, _param| {});
    let function = dyn_function(m.node_value(AnyNodeId::Dynamic(func_node)).unwrap());
    m.mark_looping(function);
    assert_eq!(m.loop_conversion(function), Err(LoopRefusal::NotRecursive));
}

#[test]
fn a_parameter_read_resolves_to_its_path() {
    let mut m = Module::new();
    let (_func_node, function, condition, decrement, param) = marked_countdown(&mut m);
    // The bare parameter is the whole value; a computation over it is not a
    // read at all.
    assert_eq!(m.parameter_value_path(function, param), Some(Vec::new()));
    assert_eq!(m.parameter_value_path(function, condition), None);
    assert_eq!(m.parameter_value_path(function, decrement), None);
}

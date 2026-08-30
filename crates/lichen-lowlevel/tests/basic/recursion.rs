//! Lazy recursion: self-applying and mutually recursive functions, the
//! branch-lazy definition pass, and the recursion depth guards.

use super::*;

#[test]
fn recursive_function_applies_itself_lazily() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (f_node, f_id) = recursive_function(&mut m);
    // The function's own value node is concrete (it never depends on the
    // parameter), so the clone keeps the self-reference in place instead of
    // copying the function per level.
    m.evaluate_node_deep(f_node, None);
    assert_eq!(m.nodes[f_node].evaluated_deep, Some(EvaluatedDeep { parameterized: false }));

    // f(5) = [5, f(5)]: each forced application produces exactly one new
    // level — a fresh, still-unevaluated apply clone referencing the same
    // function value, while the parameter is a clone unified with the
    // argument (so it carries the argument's value).
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    let rep_five = m.equality_representative(five);
    assert_eq!(ids.len(), 2);
    assert_eq!(m.equality_representative(ids[0]), rep_five);
    assert!(matches!(m.nodes[ids[0]].value, Some(TestValue::U128(_))));
    let c1 = ids[1];
    assert!(m.nodes[c1].value.is_none()); // unevaluated until forced
    assert!(matches!(
        m.nodes[c1].operation,
        Some(Operation {
            operator: TestOperator::LowOperator(LowOperator::Apply),
            ..
        })
    ));
    let ops = m.nodes[c1].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    assert_eq!(operand_ids[0], f_node);
    assert_eq!(m.equality_representative(operand_ids[1]), rep_five);

    // Forcing that level runs the same function against the same argument.
    let level1 = m.evaluate_node(c1, None);
    let ids1 = array_ids(level1);
    assert_eq!(ids1.len(), 2);
    assert_eq!(m.equality_representative(ids1[0]), rep_five);
    let c2 = ids1[1];
    assert_ne!(c2, c1);
    let ops = m.nodes[c2].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    assert_eq!(operand_ids[0], f_node);
    assert_eq!(m.equality_representative(operand_ids[1]), rep_five);

    let level2 = m.evaluate_node(c2, None);
    let ids2 = array_ids(level2);
    assert_eq!(ids2.len(), 2);
    assert_eq!(m.equality_representative(ids2[0]), rep_five);
    assert!(m.nodes[ids2[1]].value.is_none());

    // The recursion never cloned the function: the same template recursed
    // three times, referenced in place.
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.functions[f_id].block, m.nodes[f_node].block);
    assert!(matches!(
        m.nodes[f_node].value,
        Some(TestValue::LowValue(LowValue::Function(_)))
    ));
}
#[test]
fn undefined_recursive_function_clones_a_function_per_level() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // With no evaluation at all, the function value node's
    // evaluated_deep stays None, so the clone rule copies it: each
    // recursion level carries its own fresh function clone homed on the
    // calling block.
    let (f_node, f_id) = recursive_function(&mut m);
    assert_eq!(m.nodes[f_node].evaluated_deep, None);

    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    let rep_five = m.equality_representative(five);
    assert_eq!(ids.len(), 2);
    assert_eq!(m.equality_representative(ids[0]), rep_five);
    let c1 = ids[1];

    let ops = m.nodes[c1].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    let TestValue::LowValue(LowValue::Function(cloned)) = m.nodes[operand_ids[0]].value.unwrap() else {
        panic!("expected a cloned function value")
    };
    assert_ne!(cloned, f_id);
    assert_eq!(m.functions[cloned].block, root);
    assert_eq!(m.functions[cloned].nodes.len(), 5);

    let level1 = m.evaluate_node(c1, None);
    let ids1 = array_ids(level1);
    assert_eq!(ids1.len(), 2);
    assert_eq!(m.equality_representative(ids1[0]), rep_five);
    let c2 = ids1[1];
    assert_ne!(c2, c1);
    let ops = m.nodes[c2].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    let TestValue::LowValue(LowValue::Function(cloned2)) = m.nodes[operand_ids[0]].value.unwrap() else {
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
    // the clone references it in place; the parameter is a clone unified
    // with the argument.
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, f_node, five);
    let level0 = m.evaluate_node(call, None);
    let ids = array_ids(level0);
    let rep_five = m.equality_representative(five);
    assert_eq!(ids.len(), 2);
    assert_eq!(m.equality_representative(ids[0]), rep_five);
    let g_app = ids[1];
    let ops = m.nodes[g_app].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    assert_eq!(operand_ids[0], g_node);
    assert_eq!(m.equality_representative(operand_ids[1]), rep_five);

    // Forcing that level runs g's body: g(5) = [5, f(5)].
    let level1 = m.evaluate_node(g_app, None);
    let ids = array_ids(level1);
    assert_eq!(ids.len(), 2);
    assert_eq!(m.equality_representative(ids[0]), rep_five);
    let f_app = ids[1];
    assert_ne!(f_app, g_app);
    let ops = m.nodes[f_app].operation.unwrap().operand.unwrap();
    let operand_ids = array_ids(m.nodes[ops].value.unwrap());
    assert_eq!(operand_ids[0], f_node);
    assert_eq!(m.equality_representative(operand_ids[1]), rep_five);
    assert_eq!(m.functions.len(), 2); // cross-references stay in place
}
#[test]
fn fibonacci_recurses_through_index_branches() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (fib_node, fib_id) = fibonacci(&mut m);

    // The function's own value node is concrete, so the recursion reuses
    // one FunctionId instead of cloning the function per level.
    m.evaluate_node_deep(fib_node, None);
    assert_eq!(m.nodes[fib_node].evaluated_deep, Some(EvaluatedDeep { parameterized: false }));

    // The definition pass terminates: with a marker condition the Index
    // arm stays lazy and never forces the recursive branch, so the body is
    // definable even though it applies itself.
    m.evaluate_node_deep(m.functions[fib_id].r#return, None);
    assert_eq!(
        m.nodes[m.functions[fib_id].r#return].evaluated_deep,
        Some(EvaluatedDeep { parameterized: true })
    );

    for (n, expected) in [(0, 0), (1, 1), (2, 1), (3, 2), (5, 5), (10, 55)] {
        let arg = u128_node(&mut m, root, n);
        let call = call_node(&mut m, root, fib_node, arg);
        assert_eq!(
            u128_of(m.evaluate_node_deep(call, None)),
            expected,
            "fib({n})"
        );
    }

    assert_eq!(m.functions.len(), 1); // recursion referenced the function in place
}
#[test]
fn countdown_definition_pass_terminates() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = if x == 0 then 0 else Add(f(x-1), 1) — the recursion sits
    // behind a lazy branch, so the definition pass completes even though
    // the body applies itself.
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(TestValue::LowValue(LowValue::Parameterized)));
    let func_node = m.add_node(body, None, None); // placeholder self-ref
    let zero = u128_node(&mut m, body, 0);
    let one = u128_node(&mut m, body, 1);
    // f(x-1)
    let sub_ops = array_node(&mut m, body, &[param, one], None);
    let sub = op_node(&mut m, body, TestOperator::Sub, Some(sub_ops));
    let call_ops = array_node(&mut m, body, &[func_node, sub], None);
    let call = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Apply), Some(call_ops));
    // Add(f(x-1), 1)
    let rec_ops = array_node(&mut m, body, &[call, one], None);
    let rec = op_node(&mut m, body, TestOperator::Add, Some(rec_ops));
    // if x == 0 then 0 else rec
    let cond_ops = array_node(&mut m, body, &[param, zero], None);
    let cond = op_node(&mut m, body, TestOperator::Eq, Some(cond_ops));
    let branch = array_node(&mut m, body, &[rec, zero], None);
    let index_ops = array_node(&mut m, body, &[branch, cond], None);
    let ret = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Index), Some(index_ops));
    let function = finish_function(&mut m, body, ret, param, func_node);
    m.evaluate_node_deep(func_node, None); // self-ref stays in place
    m.evaluate_node_deep(ret, None); // definition pass: completes, flagged
    assert_eq!(m.nodes[ret].evaluated_deep, Some(EvaluatedDeep { parameterized: true }));

    let zero_arg = u128_node(&mut m, root, 0);
    let call = call_node(&mut m, root, func_node, zero_arg);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 0);
    let five = u128_node(&mut m, root, 5);
    let call = call_node(&mut m, root, func_node, five);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 5);
    assert_eq!(m.functions.len(), 1);
    assert_eq!(m.functions[function].block, body);
}
#[test]
fn mutual_recursion_with_branches_definition_pass_terminates() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // even(x) = if x == 0 then 1 else odd(x-1)
    // odd(x)  = if x == 0 then 0 else even(x-1)
    let body = m.add_block(None);
    let e_param = m.add_node(body, None, Some(TestValue::LowValue(LowValue::Parameterized)));
    let o_param = m.add_node(body, None, Some(TestValue::LowValue(LowValue::Parameterized)));
    let e_func = m.add_node(body, None, None); // placeholders
    let o_func = m.add_node(body, None, None);
    let zero = u128_node(&mut m, body, 0);
    let one = u128_node(&mut m, body, 1);
    // even: if x == 0 then 1 else odd(x-1)
    let e_cond_ops = array_node(&mut m, body, &[e_param, zero], None);
    let e_cond = op_node(&mut m, body, TestOperator::Eq, Some(e_cond_ops));
    let e_sub_ops = array_node(&mut m, body, &[e_param, one], None);
    let e_sub = op_node(&mut m, body, TestOperator::Sub, Some(e_sub_ops));
    let e_call_ops = array_node(&mut m, body, &[o_func, e_sub], None);
    let e_call = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Apply), Some(e_call_ops));
    let e_branch = array_node(&mut m, body, &[e_call, one], None);
    let e_index_ops = array_node(&mut m, body, &[e_branch, e_cond], None);
    let e_ret = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Index), Some(e_index_ops));
    let even = m.functions.insert(Function {
        nodes: HashSet::from([
            e_param,
            e_func,
            e_cond_ops,
            e_cond,
            e_sub_ops,
            e_sub,
            e_call_ops,
            e_call,
            e_branch,
            e_index_ops,
            e_ret,
        ]),
        r#return: e_ret,
        parameter: e_param,
        block: body,
    });
    // odd: if x == 0 then 0 else even(x-1)
    let o_cond_ops = array_node(&mut m, body, &[o_param, zero], None);
    let o_cond = op_node(&mut m, body, TestOperator::Eq, Some(o_cond_ops));
    let o_sub_ops = array_node(&mut m, body, &[o_param, one], None);
    let o_sub = op_node(&mut m, body, TestOperator::Sub, Some(o_sub_ops));
    let o_call_ops = array_node(&mut m, body, &[e_func, o_sub], None);
    let o_call = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Apply), Some(o_call_ops));
    let o_branch = array_node(&mut m, body, &[o_call, zero], None);
    let o_index_ops = array_node(&mut m, body, &[o_branch, o_cond], None);
    let o_ret = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Index), Some(o_index_ops));
    let odd = m.functions.insert(Function {
        nodes: HashSet::from([
            o_param,
            o_func,
            o_cond_ops,
            o_cond,
            o_sub_ops,
            o_sub,
            o_call_ops,
            o_call,
            o_branch,
            o_index_ops,
            o_ret,
        ]),
        r#return: o_ret,
        parameter: o_param,
        block: body,
    });
    m.blocks[body].functions.extend([even, odd]);
    m.nodes[e_func].value = Some(TestValue::LowValue(LowValue::Function(even)));
    m.nodes[o_func].value = Some(TestValue::LowValue(LowValue::Function(odd)));

    // Both bodies are definable: a marker condition keeps the Index arm
    // lazy, so the cross-call is never forced.
    m.evaluate_node_deep(e_ret, None);
    m.evaluate_node_deep(o_ret, None);
    assert_eq!(m.nodes[e_ret].evaluated_deep, Some(EvaluatedDeep { parameterized: true }));
    assert_eq!(m.nodes[o_ret].evaluated_deep, Some(EvaluatedDeep { parameterized: true }));

    let six = u128_node(&mut m, root, 6);
    let call = call_node(&mut m, root, e_func, six);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 1);
    let seven = u128_node(&mut m, root, 7);
    let call = call_node(&mut m, root, e_func, seven);
    assert_eq!(u128_of(m.evaluate_node_deep(call, None)), 0);
    assert_eq!(m.functions.len(), 2); // cross-references stay in place
}
#[test]
#[should_panic(expected = "recursion depth exceeded")]
fn non_terminating_apply_panics_at_depth_limit() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, _) = unconditional_self_apply(&mut m);
    m.apply_depth_limit = 4;
    let arg = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);
}
#[test]
#[should_panic(expected = "recursion depth exceeded")]
fn definition_pass_on_non_terminating_body_panics() {
    let mut m = Module::new();
    let (_, function) = unconditional_self_apply(&mut m);
    m.apply_depth_limit = 4;
    // The unconditional self-apply is in the direct value path, so the
    // definition pass nests applications forever instead of staying lazy.
    m.evaluate_node_deep(m.functions[function].r#return, None);
}
#[test]
#[should_panic(expected = "recursion depth exceeded")]
fn deep_evaluating_an_infinite_stream_panics() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [x, f(x)]: each apply level terminates, but deep evaluation
    // walks the infinitely growing value — the deep guard catches it.
    let (func_node, _) = recursive_function(&mut m);
    m.evaluate_depth_limit = 8;
    let arg = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);
}
#[test]
#[should_panic(expected = "too many function applications")]
fn flattened_recursion_panics_at_the_total_apply_budget() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // f(x) = [f(x), 0]: the return is a *cached pair* whose element is the
    // recursion.  Each apply evaluates the pair to its cached value and
    // returns, so the recursion is driven by the outer deep pass descending
    // into the pair — the applies stay at depth 1, invisible to the
    // nested-depth guard.  The total-application budget is the work bound
    // that catches it.
    let body = m.add_block(None);
    let param = m.add_node(body, None, Some(TestValue::LowValue(LowValue::Parameterized)));
    let func_node = m.add_node(body, None, None);
    let ops = array_node(&mut m, body, &[func_node, param], None);
    let call = op_node(&mut m, body, TestOperator::LowOperator(LowOperator::Apply), Some(ops));
    let zero = u128_node(&mut m, body, 0);
    let ret = array_node(&mut m, body, &[call, zero], None);
    let _function = finish_function(&mut m, body, ret, param, func_node);
    m.apply_total_limit = 4;
    let arg = u128_node(&mut m, root, 1);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);
}

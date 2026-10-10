//! Node evaluation: operator execution, the cycle guard, and the `evaluated_deep` markers.

use super::*;

#[test]
fn add_sums_u128_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(operands));

    let value = m.evaluate_node_deep(add, None).unwrap();

    assert_eq!(u128_of(value), 7);
}
#[test]
fn concat_joins_string_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = str_node(&mut m, root, &['a', 'b']);
    let b = str_node(&mut m, root, &['c', 'd']);
    let operands = array_node(&mut m, root, &[a, b], None);
    let concat = op_node(&mut m, root, TestOperator::Concat, Some(operands));

    let value = m.evaluate_node_deep(concat, None).unwrap();

    assert_eq!(string_of(value), vec!['a', 'b', 'c', 'd']);
}
#[test]
fn index_selects_array_element() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b], None);
    let idx = usize_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[arr, idx], None);
    let index = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(operands),
    );

    let value = m.evaluate_node_deep(index, None).unwrap();

    assert_eq!(u128_of(value), 20);
}
#[test]
fn index_out_of_bounds_records_an_eval_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b], None);
    let idx = usize_node(&mut m, root, 5);
    let operands = array_node(&mut m, root, &[arr, idx], None);
    let index = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(operands),
    );

    let value = m.evaluate_node_deep(index, None).unwrap();

    // No panic, no element: the failure is recorded as facts instead, and
    // the read yields the computed-nothing value.
    assert!(matches!(value, TestValue::LowValue(LowValue::Error)));
    assert_eq!(m.eval_errors.len(), 1);
    let EvalError::Index {
        index,
        index_value,
        length,
    } = m.eval_errors[0]
    else {
        panic!("an out-of-bounds read records an Index failure")
    };
    assert_eq!(index, lichen_lowlevel::AnyNodeId::Dynamic(idx));
    assert_eq!(index_value, 5);
    assert_eq!(length, 2);
    assert!(m.unify_errors.is_empty());
}
#[test]
fn out_of_bounds_index_is_recorded_once_and_in_bounds_still_selects() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b], None);
    // One past the end: the bound is exclusive.
    let idx = usize_node(&mut m, root, 2);
    let operands = array_node(&mut m, root, &[arr, idx], None);
    let index = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(operands),
    );

    assert!(matches!(
        m.evaluate_node_deep(index, None).unwrap(),
        TestValue::LowValue(LowValue::Error)
    ));
    assert_eq!(m.eval_errors.len(), 1);
    // Re-evaluating the same node reads the cached error result — no
    // duplicate record.
    m.evaluate_node_deep(index, None);
    assert_eq!(m.eval_errors.len(), 1);

    // The last element is still selectable.
    let idx = usize_node(&mut m, root, 1);
    let last_ops = array_node(&mut m, root, &[arr, idx], None);
    let last = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(last_ops),
    );
    assert_eq!(u128_of(m.evaluate_node_deep(last, None).unwrap()), 20);
    assert_eq!(m.eval_errors.len(), 1);
}
#[test]
fn out_of_bounds_index_in_a_function_body_records_without_panicking() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = m.add_node(root, None, None);
    // The out-of-bounds index sits in the return pair, so the definition pass hits it.
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b], None);
    let idx = usize_node(&mut m, root, 5);
    let ops = array_node(&mut m, root, &[arr, idx], None);
    let oob = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(ops),
    );
    let ret = array_node(&mut m, root, &[param, oob], None);
    wrap_function(&mut m, root, ret, param);

    m.evaluate_node_deep(ret, None);

    assert_eq!(m.eval_errors.len(), 1);
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(oob)),
        Some(TestValue::LowValue(LowValue::Error))
    ));
    assert!(
        m.node_value(AnyNodeId::Dynamic(param)).is_none(),
        "the parameter stays an empty slot"
    );
}
#[test]
fn applying_a_non_function_records_an_eval_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // A structural scalar target makes the apply a user error, not a lazy deferral.
    let callee = usize_node(&mut m, root, 5);
    let argument = usize_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[callee, argument], None);
    let apply = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Apply),
        Some(operands),
    );

    let value = m.evaluate_node_deep(apply, None).unwrap();

    // No panic, no call: the failure is recorded as a fact, and the apply
    // yields the computed-nothing value.
    assert!(matches!(value, TestValue::LowValue(LowValue::Error)));
    assert_eq!(m.eval_errors.len(), 1);
    let EvalError::ApplyTarget { function } = m.eval_errors[0] else {
        panic!("applying a non-function records an ApplyTarget failure")
    };
    assert_eq!(function, AnyNodeId::Dynamic(callee));
    assert!(m.unify_errors.is_empty());
}
#[test]
#[should_panic(expected = "cycle")]
fn cyclic_operations_panic_instead_of_looping() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // `a` starts operation-free because its operand `b` does not exist yet.

    // The cycle guard panics when `b` is re-entered while already evaluating.
    let a = m.add_node(root, None, None);
    let b = op_node(&mut m, root, TestOperator::Id, Some(a));
    m.close_operation_cycle(
        a,
        Operation {
            operator: TestOperator::Id,
            operand: Some(b),
        },
    );
    m.evaluate_node_deep(b, None);
}

#[test]
fn deep_eval_cuts_a_self_referential_value_cycle() {
    // A value cycle is cut by re-reading the cached value; an operation cycle panics.
    let mut m = Module::new();
    let root = m.add_block(None);
    let marker = u128_node(&mut m, root, 7);
    let k = m.add_node(root, None, None);
    let items = [
        ArrayItem::new(AnyNodeId::Dynamic(marker)),
        ArrayItem::new(AnyNodeId::Dynamic(k)),
    ];
    m.write_node_value(
        k,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, root),
        ))),
    );

    let value = m.evaluate_node_deep(k, None).unwrap();

    assert!(matches!(value, TestValue::LowValue(LowValue::Array(_))));
    assert!(m.nodes.keys().all(|id| !m.node_visiting(id)));
}
#[test]
fn visiting_markers_are_cleared_after_evaluation() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(operands));

    m.evaluate_node_deep(add, None);

    assert!(m.nodes.keys().all(|id| !m.node_visiting(id)));
}
#[test]
fn evaluated_deep_marks_subtrees_with_parameters() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_node(root, None, None);
    let x = u128_node(&mut m, root, 5);
    let arr = array_node(&mut m, root, &[x, p], None);
    let id_arr = op_node(&mut m, root, TestOperator::Id, Some(arr));
    let id_p = op_node(&mut m, root, TestOperator::Id, Some(p));

    m.evaluate_node_deep(id_arr, None);

    // The parameter node itself and everything reachable from it is flagged;
    // plain constants are not.
    assert_eq!(
        m.node_evaluated_deep(p),
        Some(EvaluatedDeep { undecided: true })
    );
    assert_eq!(
        m.node_evaluated_deep(arr),
        Some(EvaluatedDeep { undecided: true })
    );
    assert_eq!(
        m.node_evaluated_deep(id_arr),
        Some(EvaluatedDeep { undecided: true })
    );
    assert_eq!(
        m.node_evaluated_deep(x),
        Some(EvaluatedDeep { undecided: false })
    );
    assert_eq!(m.node_evaluated_deep(id_p), None); // not yet evaluated

    m.evaluate_node_deep(id_p, None);
    assert_eq!(
        m.node_evaluated_deep(id_p),
        Some(EvaluatedDeep { undecided: true })
    );
}
#[test]
fn deep_eval_skips_shallow_positions_until_an_index_read() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The deep pass skips the masked element entirely; position 0 is walked normally.
    let three = u128_node(&mut m, root, 3);
    let four = u128_node(&mut m, root, 4);
    let five = u128_node(&mut m, root, 5);
    let add_ops = array_node(&mut m, root, &[four, five], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_ops));
    let arr = array_node(&mut m, root, &[three, add], Some(&[false, true]));

    let value = m.evaluate_node_deep(arr, None).unwrap();
    assert!(matches!(value, TestValue::LowValue(LowValue::Array(_))));

    assert!(
        m.node_value(AnyNodeId::Dynamic(add)).is_none(),
        "shallow position stays lazy"
    );
    assert_eq!(m.node_evaluated_deep(add), None, "never walked");
    assert_eq!(
        m.node_evaluated_deep(three),
        Some(EvaluatedDeep { undecided: false })
    );
    assert_eq!(
        m.node_evaluated_deep(arr),
        Some(EvaluatedDeep { undecided: true }),
        "a shallow-marked array is never proven concrete"
    );
    // A read forces the single element on demand.
    let idx = usize_node(&mut m, root, 1);
    let ops = array_node(&mut m, root, &[arr, idx], None);
    let read = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(ops),
    );
    assert_eq!(u128_of(m.evaluate_node_deep(read, None).unwrap()), 9);
}
#[test]
fn sub_eq_lt_operators_compute_concrete_results() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let two = u128_node(&mut m, root, 2);
    let three = u128_node(&mut m, root, 3);
    // Sub: 3 - 2 = 1
    let sub_ops = array_node(&mut m, root, &[three, two], None);
    let sub = op_node(&mut m, root, TestOperator::Sub, Some(sub_ops));
    assert_eq!(u128_of(m.evaluate_node_deep(sub, None).unwrap()), 1);
    // Eq: 3 == 3
    let eq_ops = array_node(&mut m, root, &[three, three], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(eq_ops));
    assert!(matches!(
        m.evaluate_node_deep(eq, None).unwrap(),
        TestValue::LowValue(LowValue::USize(1))
    ));
    // Lt: 3 < 2 is false
    let lt_ops = array_node(&mut m, root, &[three, two], None);
    let lt = op_node(&mut m, root, TestOperator::Lt, Some(lt_ops));
    assert!(matches!(
        m.evaluate_node_deep(lt, None).unwrap(),
        TestValue::LowValue(LowValue::USize(0))
    ));
}
#[test]
fn deep_budget_refusal_under_an_extension_operator_records_without_panicking() {
    // The extension-operator arm reads its operand's `evaluated_deep` after the deep pass,

    // which returns before writing that flag when it refuses on depth.

    // Three Id frames put the innermost operand one frame past the limit.
    let mut m = Module::new();
    let root = m.add_block(None);
    let leaf = u128_node(&mut m, root, 7);
    let first = op_node(&mut m, root, TestOperator::Id, Some(leaf));
    let second = op_node(&mut m, root, TestOperator::Id, Some(first));
    let top = op_node(&mut m, root, TestOperator::Id, Some(second));
    m.evaluate_depth_limit = 2;

    let value = m.evaluate_node_deep(top, None);

    assert_eq!(
        m.budget_exhausted,
        Some(BudgetExhausted::EvaluateDepth { limit: 2 }),
        "the guard's verdict is the outcome, not a panic"
    );
    assert!(value.is_none(), "the refused frame stays undecided");
    assert_eq!(
        m.node_evaluated_deep(first),
        None,
        "the refused frame wrote no flag"
    );
}
#[test]
fn a_block_root_the_budget_refuses_yields_an_empty_value() {
    // The child block's delegation runs a fresh pass whose first frame is the block root.

    // At the limit it refuses before evaluating the root, so the root caches no value.
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let leaf = u128_node(&mut m, child, 7);
    let child_op = op_node(&mut m, child, TestOperator::Id, Some(leaf));
    let read = op_node(&mut m, root, TestOperator::Id, Some(child_op));
    m.evaluate_depth_limit = 2;

    let value = m.evaluate_node_deep(read, None).unwrap();

    assert_eq!(
        m.budget_exhausted,
        Some(BudgetExhausted::EvaluateDepth { limit: 2 }),
        "the guard's verdict is the outcome, not a panic"
    );
    assert!(matches!(value, TestValue::LowValue(LowValue::Error)));
}
#[test]
fn a_block_root_that_stays_lazy_is_not_an_internal_error() {
    // An undecided answer is never cached, so a still-lazy block root leaves compaction nothing to move.
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let undecided = undecided_node(&mut m, child);
    let add = op_node(&mut m, child, TestOperator::Add, Some(undecided));
    let read = op_node(&mut m, root, TestOperator::Id, Some(add));

    let value = m.evaluate_node_deep(read, None);

    assert!(value.is_none(), "the block root stays undecided");
}

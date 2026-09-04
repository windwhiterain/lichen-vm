//! Node evaluation: operator execution, the cycle guard, and the
//! visiting / `evaluated_deep` markers left behind by `evaluate_node_deep`.

use super::*;

#[test]
fn add_sums_u128_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(operands));

    let value = m.evaluate_node_deep(add, None);

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

    let value = m.evaluate_node_deep(concat, None);

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

    let value = m.evaluate_node_deep(index, None);

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

    let value = m.evaluate_node_deep(index, None);

    // No panic, no element: the failure is recorded as facts instead.
    assert!(matches!(value, TestValue::LowValue(LowValue::None)));
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
}#[test]
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
        m.evaluate_node_deep(index, None),
        TestValue::LowValue(LowValue::None)
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
    assert_eq!(u128_of(m.evaluate_node_deep(last, None)), 20);
    assert_eq!(m.eval_errors.len(), 1);
}
#[test]
fn out_of_bounds_index_in_a_function_body_records_without_panicking() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    // f(x) = [x, [10, 20][5]]: the OOB index sits in the return pair, so a
    // deep evaluation of the return (the definition pass) hits it.
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
        Some(TestValue::LowValue(LowValue::None))
    ));
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(param)),
        Some(TestValue::LowValue(LowValue::Parameterized))
    ));
}
#[test]
#[should_panic(expected = "cycle")]
fn cyclic_operations_panic_instead_of_looping() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = op_node(&mut m, root, TestOperator::Id, None);
    let b = op_node(&mut m, root, TestOperator::Id, Some(a));
    // Close the loop a -> b -> a through the public operation fields; the
    // evaluating state marks b as in-progress on re-entry and panics.
    m.nodes[a].operation = Some(Operation {
        operator: TestOperator::Id,
        operand: Some(b),
    });
    m.evaluate_node_deep(b, None);
}

#[test]
fn deep_eval_cuts_a_self_referential_value_cycle() {
    // A node whose array value contains itself (the `Type : Type` universe
    // `K = [Type, K]` shape, which every type spine in the recursive-pair
    // encoding reaches).  A value cycle is cut by the deep-evaluation guard
    // — the cached value is re-read, not recomputed — while an *operation*
    // cycle still panics (see above).
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

    let value = m.evaluate_node_deep(k, None);

    assert!(matches!(value, TestValue::LowValue(LowValue::Array(_))));
    assert!(m.nodes.values().all(|n| !n.visiting));
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

    assert!(m.nodes.values().all(|n| !n.visiting));
}
#[test]
fn evaluated_deep_marks_subtrees_with_parameters() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_node(
        root,
        None,
        Some(TestValue::LowValue(LowValue::Parameterized)),
    );
    let x = u128_node(&mut m, root, 5);
    let arr = array_node(&mut m, root, &[x, p], None);
    let id_arr = op_node(&mut m, root, TestOperator::Id, Some(arr));
    let id_p = op_node(&mut m, root, TestOperator::Id, Some(p));

    m.evaluate_node_deep(id_arr, None);

    // The parameter node itself and everything reachable from it is flagged;
    // plain constants are not.
    assert_eq!(
        m.nodes[p].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: true
        })
    );
    assert_eq!(
        m.nodes[arr].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: true
        })
    );
    assert_eq!(
        m.nodes[id_arr].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: true
        })
    );
    assert_eq!(
        m.nodes[x].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: false
        })
    );
    assert_eq!(m.nodes[id_p].evaluated_deep, None); // not yet evaluated

    m.evaluate_node_deep(id_p, None);
    assert_eq!(
        m.nodes[id_p].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: true
        })
    );
}
#[test]
fn deep_eval_skips_shallow_positions_until_an_index_read() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // [3, Add(4, 5)] with position 1 marked shallow: the deep pass must
    // skip the Add entirely (it stays unevaluated) while position 0 is
    // walked normally.
    let three = u128_node(&mut m, root, 3);
    let four = u128_node(&mut m, root, 4);
    let five = u128_node(&mut m, root, 5);
    let add_ops = array_node(&mut m, root, &[four, five], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_ops));
    let arr = array_node(&mut m, root, &[three, add], Some(&[false, true]));

    let value = m.evaluate_node_deep(arr, None);
    assert!(matches!(value, TestValue::LowValue(LowValue::Array(_))));

    assert!(m.node_value(AnyNodeId::Dynamic(add)).is_none(), "shallow position stays lazy");
    assert_eq!(m.nodes[add].evaluated_deep, None, "never walked");
    assert_eq!(
        m.nodes[three].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: false
        })
    );
    assert_eq!(
        m.nodes[arr].evaluated_deep,
        Some(EvaluatedDeep {
            parameterized: true
        }),
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
    assert_eq!(u128_of(m.evaluate_node_deep(read, None)), 9);
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
    assert_eq!(u128_of(m.evaluate_node_deep(sub, None)), 1);
    // Eq: 3 == 3
    let eq_ops = array_node(&mut m, root, &[three, three], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(eq_ops));
    assert!(matches!(
        m.evaluate_node_deep(eq, None),
        TestValue::LowValue(LowValue::USize(1))
    ));
    // Lt: 3 < 2 is false
    let lt_ops = array_node(&mut m, root, &[three, two], None);
    let lt = op_node(&mut m, root, TestOperator::Lt, Some(lt_ops));
    assert!(matches!(
        m.evaluate_node_deep(lt, None),
        TestValue::LowValue(LowValue::USize(0))
    ));
}

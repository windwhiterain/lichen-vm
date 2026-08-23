//! Node evaluation: operator execution, the cycle guard, and the
//! visiting / parameterized markers left behind by `evaluate_node_deep`.

use super::*;

#[test]
fn add_sums_u128_operands() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
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
    let operands = array_node(&mut m, root, &[a, b]);
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
    let arr = array_node(&mut m, root, &[a, b]);
    let idx = usize_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[arr, idx]);
    let index = op_node(&mut m, root, TestOperator::Index, Some(operands));

    let value = m.evaluate_node_deep(index, None);

    assert_eq!(u128_of(value), 20);
}
#[test]
fn index_out_of_bounds_records_an_eval_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b]);
    let idx = usize_node(&mut m, root, 5);
    let operands = array_node(&mut m, root, &[arr, idx]);
    let index = op_node(&mut m, root, TestOperator::Index, Some(operands));

    let value = m.evaluate_node_deep(index, None);

    // No panic, no element: the failure is recorded as facts instead.
    assert!(matches!(value, TestValue::None));
    assert_eq!(m.eval_errors.len(), 1);
    let err = m.eval_errors[0];
    assert_eq!(err.index, idx);
    assert_eq!(err.index_value, 5);
    assert_eq!(err.length, 2);
    assert!(m.unify_errors.is_empty());
}
#[test]
fn out_of_bounds_index_is_recorded_once_and_in_bounds_still_selects() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b]);
    // One past the end: the bound is exclusive.
    let idx = usize_node(&mut m, root, 2);
    let operands = array_node(&mut m, root, &[arr, idx]);
    let index = op_node(&mut m, root, TestOperator::Index, Some(operands));

    assert!(matches!(m.evaluate_node_deep(index, None), TestValue::None));
    assert_eq!(m.eval_errors.len(), 1);
    // Re-evaluating the same node reads the cached error result — no
    // duplicate record.
    m.evaluate_node_deep(index, None);
    assert_eq!(m.eval_errors.len(), 1);

    // The last element is still selectable.
    let idx = usize_node(&mut m, root, 1);
    let last_ops = array_node(&mut m, root, &[arr, idx]);
    let last = op_node(&mut m, root, TestOperator::Index, Some(last_ops));
    assert_eq!(u128_of(m.evaluate_node_deep(last, None)), 20);
    assert_eq!(m.eval_errors.len(), 1);
}
#[test]
fn out_of_bounds_index_in_a_function_body_records_without_panicking() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let param = m.add_node(root, None, Some(TestValue::Parameterized));
    // f(x) = [x, [10, 20][5]]: the OOB index sits in the return pair, so a
    // deep evaluation of the return (the definition pass) hits it.
    let a = u128_node(&mut m, root, 10);
    let b = u128_node(&mut m, root, 20);
    let arr = array_node(&mut m, root, &[a, b]);
    let idx = usize_node(&mut m, root, 5);
    let ops = array_node(&mut m, root, &[arr, idx]);
    let oob = op_node(&mut m, root, TestOperator::Index, Some(ops));
    let ret = array_node(&mut m, root, &[param, oob]);
    wrap_function(&mut m, root, ret, param);

    m.evaluate_node_deep(ret, None);

    assert_eq!(m.eval_errors.len(), 1);
    assert!(matches!(m.nodes[oob].value, Some(TestValue::None)));
    assert!(matches!(
        m.nodes[param].value,
        Some(TestValue::Parameterized)
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
    let slice = m.blocks[root].arena.alloc_slice_copy(&[marker, k]);
    m.nodes[k].value = Some(TestValue::Array(std::ptr::slice_from_raw_parts(
        slice.as_ptr(),
        slice.len(),
    )));

    let value = m.evaluate_node_deep(k, None);

    assert!(matches!(value, TestValue::Array(_)));
    assert!(m.nodes.values().all(|n| !n.visiting));
}
#[test]
fn visiting_markers_are_cleared_after_evaluation() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 3);
    let b = u128_node(&mut m, root, 4);
    let operands = array_node(&mut m, root, &[a, b]);
    let add = op_node(&mut m, root, TestOperator::Add, Some(operands));

    m.evaluate_node_deep(add, None);

    assert!(m.nodes.values().all(|n| !n.visiting));
}
#[test]
fn parameterized_deep_marks_subtrees_with_parameters() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let p = m.add_node(root, None, Some(TestValue::Parameterized));
    let x = u128_node(&mut m, root, 5);
    let arr = array_node(&mut m, root, &[x, p]);
    let id_arr = op_node(&mut m, root, TestOperator::Id, Some(arr));
    let id_p = op_node(&mut m, root, TestOperator::Id, Some(p));

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
fn sub_eq_lt_operators_compute_concrete_results() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let two = u128_node(&mut m, root, 2);
    let three = u128_node(&mut m, root, 3);
    // Sub: 3 - 2 = 1
    let sub_ops = array_node(&mut m, root, &[three, two]);
    let sub = op_node(&mut m, root, TestOperator::Sub, Some(sub_ops));
    assert_eq!(u128_of(m.evaluate_node_deep(sub, None)), 1);
    // Eq: 3 == 3
    let eq_ops = array_node(&mut m, root, &[three, three]);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(eq_ops));
    assert!(matches!(
        m.evaluate_node_deep(eq, None),
        TestValue::USize(1)
    ));
    // Lt: 3 < 2 is false
    let lt_ops = array_node(&mut m, root, &[three, two]);
    let lt = op_node(&mut m, root, TestOperator::Lt, Some(lt_ops));
    assert!(matches!(
        m.evaluate_node_deep(lt, None),
        TestValue::USize(0)
    ));
}

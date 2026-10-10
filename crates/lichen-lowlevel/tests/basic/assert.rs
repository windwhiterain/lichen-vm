//! Asserts: an explicit constraint a condition node must reach `USize(1)` for.
//! Design: `docs/notes/lowlevel-vm.md`.

use super::*;

#[test]
fn passing_assert_records_no_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let one = usize_node(&mut m, root, 1);
    let condition = m.add_assert(one);
    assert_eq!(
        m.asserts.iter().map(|e| e.condition).collect::<Vec<_>>(),
        vec![condition],
        "the condition is registered"
    );

    m.check_asserts();

    assert!(m.assert_errors.is_empty());
}

#[test]
fn failing_assert_records_the_value() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let three = usize_node(&mut m, root, 3);
    m.add_assert(three);

    m.check_asserts();

    assert_eq!(m.assert_errors.len(), 1);
    let err = m.assert_errors[0];
    assert_eq!(
        err.value,
        TestValue::LowValue(LowValue::USize(3)),
        "the resolved value is recorded"
    );
}

#[test]
fn assert_resolves_through_a_computation() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let zero = u128_node(&mut m, root, 0);
    let one = u128_node(&mut m, root, 1);
    let add_operands = array_node(&mut m, root, &[zero, one], None);
    let add = op_node(&mut m, root, TestOperator::Add, Some(add_operands));
    let eq_operands = array_node(&mut m, root, &[add, one], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(eq_operands));
    m.add_assert(eq);

    m.check_asserts();

    assert!(m.assert_errors.is_empty(), "0 + 1 == 1 holds");
}

#[test]
fn assert_on_an_empty_value_fails() {
    // A failed read resolves to the decided `Error`, so the assert FAILS rather than staying untriggered.
    let mut m = Module::new();
    let root = m.add_block(None);
    let a = u128_node(&mut m, root, 10);
    let arr = array_node(&mut m, root, &[a], None);
    let idx = usize_node(&mut m, root, 5);
    let operands = array_node(&mut m, root, &[arr, idx], None);
    let oob = op_node(
        &mut m,
        root,
        TestOperator::LowOperator(LowOperator::Index),
        Some(operands),
    );
    m.add_assert(oob);

    m.check_asserts();

    assert_eq!(m.assert_errors.len(), 1, "an `Error` condition fails");
    assert_eq!(
        m.assert_errors[0].value,
        TestValue::LowValue(LowValue::Error),
        "the failed read's residue is recorded"
    );
    assert!(
        m.asserts.is_empty(),
        "a decided condition is consumed, not deferred"
    );
}

#[test]
fn assert_on_an_undecided_condition_is_not_triggered() {
    // Unlike a unification the assert never binds its undecided condition node.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = undecided_node(&mut m, root);
    let one = u128_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[x, one], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    m.add_assert(eq);

    m.check_asserts();

    assert!(
        m.assert_errors.is_empty(),
        "an untriggered assert is no failure"
    );
    assert!(
        m.node_value(AnyNodeId::Dynamic(x)).is_none(),
        "the undecided cell was not bound by the assert"
    );
}

/// `f(x) = assert(x == 1)` applied to `arg`; the clone re-checks the condition.
fn applied_equality_assert(arg: usize) -> Module<TestProgram> {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, _, _) = function(&mut m, |m, ret, param| {
        let block = m.node_block(ret);
        let one = u128_node(m, block, 1);
        let operands = array_node(m, block, &[param, one], None);
        let eq = op_node(m, block, TestOperator::Eq, Some(operands));
        m.add_assert(eq);
        let pair = array_node(m, block, &[eq, one], None);
        m.write_node_value(ret, Some(m.node_value(AnyNodeId::Dynamic(pair)).unwrap()));
    });
    let arg = u128_node(&mut m, root, arg as u128);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);
    m.check_asserts();
    m
}

#[test]
fn apply_clones_the_untriggered_assert_and_checks_the_call() {
    // The template's own assert stays untriggered on the worklist.
    let m = applied_equality_assert(1);
    assert_eq!(
        m.asserts.len(),
        1,
        "only the untriggered template stays on the worklist"
    );
    assert!(m.assert_errors.is_empty());
}

#[test]
fn apply_clone_fails_when_the_argument_violates_the_assert() {
    // The failed clone's entry is consumed too; its error is what stays.
    let m = applied_equality_assert(2);
    assert_eq!(m.assert_errors.len(), 1);
    assert_eq!(
        m.assert_errors[0].value,
        TestValue::LowValue(LowValue::USize(0))
    );
    assert_eq!(m.asserts.len(), 1, "the decided clone left the worklist");
}

#[test]
fn never_called_function_assert_stays_pending() {
    // Its assert's condition stays undecided, so it is neither triggered nor failed.
    let mut m = Module::new();
    let (func_node, _, _) = function(&mut m, |m, ret, param| {
        let block = m.node_block(ret);
        let one = u128_node(m, block, 1);
        let operands = array_node(m, block, &[param, one], None);
        let eq = op_node(m, block, TestOperator::Eq, Some(operands));
        m.add_assert(eq);
        let pair = array_node(m, block, &[eq, one], None);
        m.write_node_value(ret, Some(m.node_value(AnyNodeId::Dynamic(pair)).unwrap()));
    });
    // The function value is proven concrete (referenced, never applied).
    m.evaluate_node_deep(func_node, None);
    assert_eq!(m.asserts.len(), 1);
    m.check_asserts();
    assert_eq!(
        m.asserts.len(),
        1,
        "the untriggered entry stays on the worklist"
    );
    assert!(
        m.assert_errors.is_empty(),
        "an untriggered assert is no failure"
    );
}

#[test]
fn a_satisfied_assert_is_consumed_from_the_worklist() {
    // A satisfied condition leaves the worklist, so a second drain finds nothing.
    let mut m = Module::new();
    let root = m.add_block(None);
    let one = usize_node(&mut m, root, 1);
    m.add_assert(one);

    m.check_asserts();

    assert!(m.asserts.is_empty(), "the satisfied entry was consumed");
    assert!(m.assert_errors.is_empty());

    m.check_asserts();

    assert!(m.asserts.is_empty(), "re-draining is a no-op");
    assert!(m.assert_errors.is_empty());
}

#[test]
fn a_failed_assert_is_consumed_but_its_error_stays() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let three = usize_node(&mut m, root, 3);
    m.add_assert(three);

    m.check_asserts();

    assert!(m.asserts.is_empty(), "the decided entry left the worklist");
    assert_eq!(m.assert_errors.len(), 1, "the failure is recorded once");
}

#[test]
fn an_untriggered_assert_is_decided_by_a_later_drain() {
    // Once the cell is bound by a later unification, the same entry is picked up and decided.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = undecided_node(&mut m, root);
    let one = u128_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[x, one], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    let condition = m.add_assert(eq);

    m.check_asserts();

    assert_eq!(
        m.asserts.iter().map(|e| e.condition).collect::<Vec<_>>(),
        vec![condition],
        "still pending"
    );
    assert!(m.assert_errors.is_empty());

    // The binding happens outside the drain; here it writes the cell directly.
    let p = m.blocks[root].arena.alloc(1u128);
    m.write_node_value(x, Some(TestValue::U128(dyn_handle(p as *const u128))));

    m.check_asserts();

    assert!(m.asserts.is_empty(), "the pending entry got decided");
    assert!(m.assert_errors.is_empty(), "x == 1 now holds");
}

#[test]
fn a_shallow_marked_operand_leaves_the_condition_pending() {
    // The walk does not descend a masked position, and descending it would not help:

    // the operator's operand gate refuses an undecided operand, and the mark alone decides the array.
    let mut m = Module::new();
    let root = m.add_block(None);
    let zero = u128_node(&mut m, root, 0);
    let one = u128_node(&mut m, root, 1);
    let add_operands = array_node(&mut m, root, &[zero, one], None);
    let hidden = op_node(&mut m, root, TestOperator::Add, Some(add_operands));
    let operands = array_node(&mut m, root, &[hidden, one], Some(&[true, false]));
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    m.add_assert(eq);

    assert!(
        m.evaluate_node_deep(eq, Some(root)).is_none(),
        "the pass cannot resolve the masked operand"
    );

    m.check_asserts();

    assert!(
        m.assert_errors.is_empty(),
        "an unresolvable condition is no failure"
    );
    assert_eq!(
        m.asserts.len(),
        1,
        "the masked condition stays pending, not consumed"
    );
}

#[test]
fn a_shallow_marked_undecided_cell_keeps_the_condition_lazy() {
    // No walk invents values for a masked position nothing can decide.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = undecided_node(&mut m, root);
    let one = u128_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[x, one], Some(&[true, false]));
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    m.add_assert(eq);

    m.check_asserts();

    assert!(
        m.assert_errors.is_empty(),
        "still untriggered — the cell is undecided"
    );
    assert_eq!(m.asserts.len(), 1, "and still pending");
}

#[test]
fn gc_prunes_asserts_of_dropped_blocks() {
    // The entry is pruned with its block, so the check pass never walks a dangling id.
    let mut m = Module::new();
    let root = m.add_block(None);
    let child = m.add_block(Some(root));
    let moved = usize_node(&mut m, child, 1); // survives via the operand edge
    let condition = usize_node(&mut m, child, 9);
    m.add_assert(condition);
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(moved));

    m.evaluate_node_deep(root_node, None);

    assert!(!m.blocks.contains_key(child));
    assert!(
        !m.nodes.contains_key(condition),
        "the condition died with its block"
    );
    assert!(
        !m.asserts.iter().any(|e| e.condition == condition),
        "the dropped entry is pruned"
    );
    m.check_asserts();
    assert!(m.assert_errors.is_empty());
}

#[test]
fn gc_moves_an_assert_condition_with_its_function() {
    // The condition is reachable only through the function's assert list, so compaction moves it too.
    let mut m = Module::new();
    let root = m.add_block(None);
    let body = m.add_block(Some(root));
    let param = undecided_node(&mut m, body);
    let ret = m.add_node(body, None, None);
    let condition = usize_node(&mut m, body, 1);
    m.add_assert(condition);
    let func_placeholder = m.add_node(body, None, None);
    let nodes: Vec<NodeId> = m.blocks[body]
        .nodes
        .iter()
        .copied()
        .filter(|&id| id != condition)
        .collect();
    let function = m.functions.insert(Function {
        nodes: Vec::new(),
        r#return: ret,
        parameter: param,
        return_type: NodeId::default(),
        static_origin: None,
        asserts: vec![condition],
        parent: None,
        block: body,
        looping: false,
    });
    tag_scope(&mut m, function, nodes);
    m.blocks[body].functions.push(function);
    m.write_node_value(
        func_placeholder,
        Some(TestValue::LowValue(LowValue::Function(
            AnyFunctionId::Dynamic(function),
        ))),
    );
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(func_placeholder));

    m.evaluate_node_deep(root_node, None);

    assert!(!m.blocks.contains_key(body));
    assert_eq!(
        m.node_block(condition),
        root,
        "the condition moved with its function"
    );
    assert!(
        m.asserts.iter().any(|e| e.condition == condition),
        "the moved entry stays registered"
    );
    m.check_asserts();
    assert!(m.assert_errors.is_empty());
}

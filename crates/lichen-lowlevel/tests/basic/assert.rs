//! Asserts: explicit constraints — an assert registers a condition node
//! that `Module::check_asserts` force-evaluates (ignoring laziness) and
//! requires to be `USize(1)`.  Unlike a unification the constraint does not
//! bind its node: an unbound condition is not triggered, and the apply
//! clone re-checks the instantiated condition per call.  The registry is a
//! worklist: a drain consumes every decided entry (failures land in
//! `assert_errors`) and keeps exactly the untriggered ones for the next
//! call.

use super::*;

#[test]
fn passing_assert_records_no_error() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let one = usize_node(&mut m, root, 1);
    let condition = m.add_assert(one);
    assert_eq!(m.asserts, vec![condition], "the condition is registered");

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
fn assert_on_an_unbound_condition_is_not_triggered() {
    // The condition reads an unbound cell: the evaluation stays lazy, so
    // the assert is deferred — not bound to `1` (that is what makes the
    // constraint explicit rather than a unification) and not failed.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = unbound_node(&mut m, root);
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
        matches!(
            m.nodes[x].value,
            Some(TestValue::LowValue(LowValue::Parameterized))
        ),
        "the unbound cell was not bound by the assert"
    );
}

/// `f(x) = assert(x == 1)` — the template's assert cannot resolve at
/// normalize (the parameter is unbound), so the apply clones it and the
/// clone re-checks against the argument.  Returns the module with `f`
/// applied to `arg`.
fn applied_equality_assert(arg: usize) -> Module<TestProgram> {
    let mut m = Module::new();
    let root = m.add_block(None);
    let (func_node, _, _) = function(&mut m, |m, ret, param| {
        let block = m.nodes[ret].block;
        let one = u128_node(m, block, 1);
        let operands = array_node(m, block, &[param, one], None);
        let eq = op_node(m, block, TestOperator::Eq, Some(operands));
        m.add_assert(eq);
        let pair = array_node(m, block, &[eq, one], None);
        m.nodes[ret].value = Some(m.nodes[pair].value.unwrap());
    });
    let arg = u128_node(&mut m, root, arg as u128);
    let call = call_node(&mut m, root, func_node, arg);
    m.evaluate_node_deep(call, None);
    m.check_asserts();
    m
}

#[test]
fn apply_clones_the_untriggered_assert_and_checks_the_call() {
    // f(1): the clone's condition resolves to 1 and passes — consumed by
    // the drain; the template's own assert stays untriggered and pending.
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
    // f(2): the clone's condition resolves to 0 — a failed assert.  The
    // failed entry is consumed too; its error is what stays.
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
    // The function is defined but never applied: its assert's condition
    // stays unbound, so it is not triggered — and not failed.
    let mut m = Module::new();
    let (func_node, _, _) = function(&mut m, |m, ret, param| {
        let block = m.nodes[ret].block;
        let one = u128_node(m, block, 1);
        let operands = array_node(m, block, &[param, one], None);
        let eq = op_node(m, block, TestOperator::Eq, Some(operands));
        m.add_assert(eq);
        let pair = array_node(m, block, &[eq, one], None);
        m.nodes[ret].value = Some(m.nodes[pair].value.unwrap());
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
    // A top-level entry whose condition resolves to `USize(1)` leaves the
    // worklist: a second drain finds nothing to re-check.
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
    // The condition reads an unbound cell: the entry stays pending across
    // calls.  Once the cell is bound by a later unification (outside the
    // drain), the very same entry is picked up and decided.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = unbound_node(&mut m, root);
    let one = u128_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[x, one], None);
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    let condition = m.add_assert(eq);

    m.check_asserts();

    assert_eq!(m.asserts, vec![condition], "still pending");
    assert!(m.assert_errors.is_empty());

    // Later unification binds through the cell's class (outside the drain;
    // here we bind directly), so the next drain resolves it.
    let p = m.blocks[root].arena.alloc(1u128);
    m.nodes[x].value = Some(TestValue::U128(dyn_handle(p as *const u128)));

    m.check_asserts();

    assert!(m.asserts.is_empty(), "the pending entry got decided");
    assert!(m.assert_errors.is_empty(), "x == 1 now holds");
}

#[test]
fn forced_evaluation_ignores_shallow_markers() {
    // The condition's operand array marks `hidden` shallow: the deep pass
    // inside the Eq's evaluation skips it, leaving the computation
    // unevaluated — a lazy condition.  The assert's forced evaluation runs
    // it anyway, resolving 0 + 1 == 1.
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
        matches!(
            m.evaluate_node_deep(eq, Some(root)).as_enum(),
            Some(LowValue::Parameterized)
        ),
        "the lazy pass cannot resolve the masked operand"
    );

    m.check_asserts();

    assert!(m.assert_errors.is_empty(), "the forced pass resolves it");
}

#[test]
fn forced_evaluation_keeps_a_genuinely_unbound_condition_lazy() {
    // Forcing past the markers does not invent values: a shallow-marked
    // unbound cell still leaves the condition untriggered.
    let mut m = Module::new();
    let root = m.add_block(None);
    let x = unbound_node(&mut m, root);
    let one = u128_node(&mut m, root, 1);
    let operands = array_node(&mut m, root, &[x, one], Some(&[true, false]));
    let eq = op_node(&mut m, root, TestOperator::Eq, Some(operands));
    m.add_assert(eq);

    m.check_asserts();

    assert!(
        m.assert_errors.is_empty(),
        "still untriggered — the cell is unbound"
    );
}

#[test]
fn gc_prunes_asserts_of_dropped_blocks() {
    // The condition lives in a child block and nothing outside references
    // it: when the block is compacted away it dies with it, and the
    // registry entry is pruned so the check pass does not walk a dangling
    // id.
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
        !m.asserts.contains(&condition),
        "the dropped entry is pruned"
    );
    m.check_asserts();
    assert!(m.assert_errors.is_empty());
}

#[test]
fn gc_moves_an_assert_condition_with_its_function() {
    // The condition is an assert of a function homed in a child block; it
    // is reachable only through the function's own assert list (excluded
    // from the scope, referenced by no value), so compacting the block must
    // move it with the function — it stays callable and the registry entry
    // stays valid.
    let mut m = Module::new();
    let root = m.add_block(None);
    let body = m.add_block(Some(root));
    let param = unbound_node(&mut m, body);
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
        asserts: vec![condition],
        parent: None,
        block: body,
    });
    tag_scope(&mut m, function, nodes);
    m.blocks[body].functions.push(function);
    m.nodes[func_placeholder].value = Some(TestValue::LowValue(LowValue::Function(
        AnyFunctionId::Dynamic(function),
    )));
    let root_node = op_node(&mut m, root, TestOperator::Id, Some(func_placeholder));

    m.evaluate_node_deep(root_node, None);

    assert!(!m.blocks.contains_key(body));
    assert_eq!(
        m.nodes[condition].block, root,
        "the condition moved with its function"
    );
    assert!(
        m.asserts.contains(&condition),
        "the moved entry stays registered"
    );
    m.check_asserts();
    assert!(m.assert_errors.is_empty());
}

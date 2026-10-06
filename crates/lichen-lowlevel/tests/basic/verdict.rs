//! The deep pass's concreteness verdict (`evaluated_deep`) and the two facts its
//! `None` used to carry: "the pass never ran here" and "a cycle cut re-entered
//! this node while its own frame was still computing it" (`P1-31`).

use super::*;

fn usize_of(value: impl Into<Option<TestValue>>) -> usize {
    let Some(TestValue::LowValue(LowValue::USize(n))) = value.into() else {
        panic!("expected a USize");
    };
    n
}

/// A cyclic value must still be **proven concrete**.  This is the behaviour the
/// `P1-31` split has to preserve: a self-referential structure is only provable
/// by assuming the re-entered node concrete at the cut, and if this fails the
/// checker's canonical universe `[Type, ↺]` is cloned per apply instead of
/// referenced in place (a unification conflict, not a slowdown).
#[test]
fn a_cyclic_value_is_proven_concrete() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let one = usize_node(&mut m, root, 1);

    // A direct self-reference — the universe's shape, where the reader *is* the
    // re-entered node.
    let self_ref = m.add_node(root, None, None);
    let items = [item(one), item(self_ref)];
    m.write_node_value(
        self_ref,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&items, root),
        ))),
    );

    // A two-node cycle, where the re-entered node's frame is an ancestor one
    // level up rather than the reader itself.
    let a = m.add_node(root, None, None);
    let b = m.add_node(root, None, None);
    let a_items = [item(b), item(one)];
    m.write_node_value(
        a,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&a_items, root),
        ))),
    );
    let b_items = [item(a), item(one)];
    m.write_node_value(
        b,
        Some(TestValue::LowValue(LowValue::Array(
            m.alloc_array(&b_items, root),
        ))),
    );

    m.evaluate_node_deep(self_ref, None);
    m.evaluate_node_deep(a, None);

    let concrete = Some(EvaluatedDeep {
        parameterized: false,
    });
    assert_eq!(
        m.node_evaluated_deep(self_ref),
        concrete,
        "a direct self-reference"
    );
    assert_eq!(m.node_evaluated_deep(a), concrete, "a two-node cycle");
    assert_eq!(m.node_evaluated_deep(b), concrete, "a two-node cycle");
}

/// A subtree the pass **refused on** has no verdict, and a node no frame is
/// computing must read unproven — not concrete.  The refusal leaves the verdict
/// absent (`evaluate_node_deep_inner` returns before it writes), which is the
/// case the cycle cut's assumption must not cover.
#[test]
fn a_refused_subtree_leaves_its_parent_unproven() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let limit = 4;
    m.evaluate_depth_limit = limit;
    // Deeper than the limit, so the pass refuses inside the chain and writes no
    // verdict for the node it refused on.
    let mut deep = usize_node(&mut m, root, 7);
    for _ in 0..24 {
        deep = array_node(&mut m, root, &[deep], None);
    }
    // A sibling the pass does walk, so the parent's own arm sees a decided
    // position next to the refused one.
    let shallow = usize_node(&mut m, root, 1);
    let parent = array_node(&mut m, root, &[deep, shallow], None);

    m.evaluate_node_deep(parent, None);

    assert_eq!(
        m.budget_exhausted,
        Some(BudgetExhausted::EvaluateDepth { limit })
    );
    assert_eq!(
        m.node_evaluated_deep(parent),
        Some(EvaluatedDeep {
            parameterized: true
        }),
        "a refused subtree is unproven, so its parent cannot be certified concrete"
    );
}

/// The **operand exemption**, pinned.  A core operator's operand is the argument
/// array a layer above synthesized, and the deep pass descends value-reachable
/// edges only, so "the operand was never walked" is the normal case.  The node
/// is therefore certified concrete from its own *value* — and reading the
/// operand conservatively instead would flip every pair read in a template and
/// clone them all per apply.
///
/// The second half is the harm probe `P1-31` left open: the exemption must not
/// corrupt a second call, because the baked node's cached value has to be
/// call-independent.
#[test]
fn an_operand_the_pass_never_walked_certifies_the_node() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The body is `p => [1, p](0)`: the pair's element 1 depends on the
    // parameter, and the indexed read only ever reads element 0, so the
    // parameter-dependent sibling is never evaluated.
    let mut pair_in_body = None;
    let (func_node, ret, _param) = function(&mut m, |m, ret, param| {
        let block = m.node_block(ret);
        let read_param = op_node(m, block, TestOperator::Id, Some(param));
        let one = usize_node(m, block, 1);
        let pair = array_node(m, block, &[one, read_param], None);
        pair_in_body = Some(pair);
        let zero = usize_node(m, block, 0);
        let operands = array_node(m, block, &[pair, zero], None);
        m.close_operation_cycle(
            ret,
            Operation {
                operator: TestOperator::LowOperator(LowOperator::Index),
                operand: Some(operands),
            },
        );
    });
    let pair = pair_in_body.expect("the body built its pair");

    // The checker's order: prove the body's own return before any apply.
    m.evaluate_node_deep(ret, None);

    assert_eq!(
        m.node_evaluated_deep(ret),
        Some(EvaluatedDeep {
            parameterized: false
        }),
        "the indexed read is certified concrete although its operand was never walked"
    );
    assert_eq!(
        m.node_evaluated_deep(pair),
        None,
        "the operand itself has no verdict — that is what makes this the exemption"
    );

    // Two calls with different arguments must agree: the baked read's value is
    // the literal, and nothing re-reads the parameter-dependent sibling.
    let eleven = usize_node(&mut m, root, 11);
    let twenty_two = usize_node(&mut m, root, 22);
    let first = call_node(&mut m, root, func_node, eleven);
    let second = call_node(&mut m, root, func_node, twenty_two);
    assert_eq!(usize_of(m.evaluate_node_deep(first, None).unwrap()), 1);
    assert_eq!(usize_of(m.evaluate_node_deep(second, None).unwrap()), 1);
}

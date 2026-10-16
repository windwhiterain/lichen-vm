//! Dependent types are lazy computations; see docs/notes/eval-before-unify.md.
//!
//! # Invariant
//! Unify never evaluates a computation: it merges, and the computation's own
//! result is what must agree with the merged value once something reads it.

use lichen_highlevel::program::{HighProgramOperator, HighProgramValue, ProgramImpl};
use lichen_lowlevel::{
    AnyNodeId, ArrayItem, BlockId, LowOperator, LowValue, Module, NodeId, Operation,
};

/// The dynamic node behind an item ref — the checker builds only dynamic graphs.
fn dyn_node(id: AnyNodeId) -> NodeId {
    match id {
        AnyNodeId::Dynamic(node) => node,
        AnyNodeId::Static(_) => unreachable!("checker graphs are dynamic"),
    }
}

fn usize_node(m: &mut Module<ProgramImpl>, block: BlockId, n: usize) -> NodeId {
    m.add_node(
        block,
        None,
        Some(HighProgramValue::LowValue(LowValue::USize(n))),
    )
}

fn undecided_node(m: &mut Module<ProgramImpl>, block: BlockId) -> NodeId {
    m.add_node(block, None, None)
}

fn array_node(m: &mut Module<ProgramImpl>, block: BlockId, ids: &[NodeId]) -> NodeId {
    let items: Vec<ArrayItem> = ids
        .iter()
        .map(|&node| ArrayItem::new(AnyNodeId::Dynamic(node)))
        .collect();
    m.add_node(
        block,
        None,
        Some(HighProgramValue::LowValue(LowValue::Array(
            m.alloc_array(&items, block),
        ))),
    )
}

/// An `Index` over `[branches, condition]`: the dependent-codomain stand-in.
fn index_node(
    m: &mut Module<ProgramImpl>,
    block: BlockId,
    branches: NodeId,
    cond: NodeId,
) -> NodeId {
    let operands = array_node(m, block, &[branches, cond]);
    m.add_node(
        block,
        Some(Operation {
            operator: HighProgramOperator::LowOperator(LowOperator::Index),
            operand: Some(operands),
        }),
        None,
    )
}

/// An `Apply` node with operand array `[function, argument]`.
fn apply_node(m: &mut Module<ProgramImpl>, block: BlockId, func: NodeId, arg: NodeId) -> NodeId {
    let operands = array_node(m, block, &[func, arg]);
    m.add_node(
        block,
        Some(Operation {
            operator: HighProgramOperator::LowOperator(LowOperator::Apply),
            operand: Some(operands),
        }),
        None,
    )
}

fn array_ids(value: HighProgramValue) -> Vec<NodeId> {
    let HighProgramValue::LowValue(LowValue::Array(array)) = value else {
        panic!("expected an array value")
    };
    // SAFETY: the value was just produced by the module under test, whose
    // block has not been dropped.
    unsafe { array.items() }
        .iter()
        .map(|item| dyn_node(item.node))
        .collect()
}

#[test]
fn dependent_type_resolves_per_argument_via_laziness() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The codomain's condition is the parameter, so an apply's clone
    // rewrites it to the fresh parameter clone.
    let x = undecided_node(&mut m, root);
    let float = usize_node(&mut m, root, 0);
    let int = usize_node(&mut m, root, 1);
    let branches = array_node(&mut m, root, &[float, int]);
    let codomain_operands = array_node(&mut m, root, &[branches, x]);
    let codomain = m.add_node(
        root,
        Some(Operation {
            operator: HighProgramOperator::LowOperator(LowOperator::Index),
            operand: Some(codomain_operands),
        }),
        None,
    );
    let ret = array_node(&mut m, root, &[x, codomain]);
    let f = m.add_function(
        root,
        ret,
        x,
        [x, codomain, codomain_operands, ret, branches, float, int],
        [],
    );

    // applied to 1
    let one = usize_node(&mut m, root, 1);
    let call = apply_node(&mut m, root, f, one);
    let value = m.evaluate_node_deep(call, None).unwrap();
    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(HighProgramValue::LowValue(LowValue::USize(1)))
    ));

    // applied to 0
    let zero = usize_node(&mut m, root, 0);
    let call = apply_node(&mut m, root, f, zero);
    let value = m.evaluate_node_deep(call, None).unwrap();
    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(HighProgramValue::LowValue(LowValue::USize(0)))
    ));
}

#[test]
fn a_concrete_type_is_never_bound_over_a_dependent_codomain() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // The function is passed as a value, so its codomain meets a concrete
    // value while the parameter is still undecided.
    let float = usize_node(&mut m, root, 0);
    let int = usize_node(&mut m, root, 1);
    let branches = array_node(&mut m, root, &[float, int]);

    // x = 1
    let x1 = undecided_node(&mut m, root);
    let codomain1 = index_node(&mut m, root, branches, x1);
    m.unify(int, codomain1);
    assert!(m.unify_errors.is_empty());
    let one = usize_node(&mut m, root, 1);
    m.unify(x1, one);
    let _ = m.evaluate_node_deep(codomain1, None);
    assert!(
        m.unify_errors.is_empty(),
        "a codomain that resolves to the value it was given is not a conflict"
    );
    assert_eq!(
        m.equality_representative(int),
        m.equality_representative(codomain1)
    );

    // x = 0
    let x0 = undecided_node(&mut m, root);
    let codomain0 = index_node(&mut m, root, branches, x0);
    m.unify(int, codomain0);
    assert!(m.unify_errors.is_empty());
    let zero = usize_node(&mut m, root, 0);
    m.unify(x0, zero);
    let _ = m.evaluate_node_deep(codomain0, None);
    assert_eq!(
        m.unify_errors.len(),
        1,
        "a codomain that resolves to a different value is a conflict"
    );
}

#[test]
fn a_resolvable_computation_is_forced_and_compared() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let four_v = usize_node(&mut m, root, 4);
    let five_v = usize_node(&mut m, root, 5);
    let branches = array_node(&mut m, root, &[four_v, five_v]);
    let cond = usize_node(&mut m, root, 1);
    let pick_five = index_node(&mut m, root, branches, cond);

    let five = usize_node(&mut m, root, 5);
    m.unify(five, pick_five);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(five),
        m.equality_representative(pick_five)
    );
    let _ = m.evaluate_node_deep(pick_five, None);
    assert!(
        m.unify_errors.is_empty(),
        "an equal expectation is not a conflict"
    );

    let four = usize_node(&mut m, root, 4);
    m.unify(four, pick_five);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(four),
        m.equality_representative(pick_five)
    );
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(pick_five)),
        Some(HighProgramValue::LowValue(LowValue::USize(5)))
    ));
}

#[test]
fn a_resolvable_index_read_pins_its_element() {
    // A resolvable read aliases its element and pins the value onto it; the
    // operation edge stays for an apply's clone.
    let mut m = Module::new();
    let root = m.add_block(None);
    let cell = undecided_node(&mut m, root);
    let container = array_node(&mut m, root, &[cell]);
    let zero = usize_node(&mut m, root, 0);
    let read = index_node(&mut m, root, container, zero);
    let three = usize_node(&mut m, root, 3);
    m.unify(three, read);
    assert!(m.unify_errors.is_empty());
    // The subscript is only known by evaluating the read, so the alias forms
    // at evaluation, not at unify time.
    let _ = m.evaluate_node_deep(read, None);
    assert_eq!(
        m.equality_representative(three),
        m.equality_representative(cell),
        "the read aliased the element"
    );
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(read)),
        Some(HighProgramValue::LowValue(LowValue::USize(3)))
    ));
    assert!(
        m.node_operation(read).is_some(),
        "the pinned read keeps its operation (the operand edge must survive)"
    );

    let five = usize_node(&mut m, root, 5);
    m.unify(five, read);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(five),
        m.equality_representative(read)
    );
}

#[test]
fn two_resolvable_computations_are_compared_after_forcing() {
    let mut m = Module::new();
    let root = m.add_block(None);
    let four_v = usize_node(&mut m, root, 4);
    let five_v = usize_node(&mut m, root, 5);
    let branches = array_node(&mut m, root, &[four_v, five_v]);
    let cond1 = usize_node(&mut m, root, 1);
    let pick5 = index_node(&mut m, root, branches, cond1);
    let cond0 = usize_node(&mut m, root, 0);
    let pick4 = index_node(&mut m, root, branches, cond0);

    let _ = m.evaluate_node_deep(pick5, None);
    let _ = m.evaluate_node_deep(pick4, None);
    m.unify(pick5, pick4);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(pick5),
        m.equality_representative(pick4)
    );
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(pick5)),
        Some(HighProgramValue::LowValue(LowValue::USize(5)))
    ));
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(pick4)),
        Some(HighProgramValue::LowValue(LowValue::USize(4)))
    ));

    // equal computations merge
    let cond1b = usize_node(&mut m, root, 1);
    let pick5b = index_node(&mut m, root, branches, cond1b);
    let _ = m.evaluate_node_deep(pick5b, None);
    m.unify(pick5, pick5b);
    // the earlier mismatch error persists: unify errors are permanent
    assert_eq!(m.unify_errors.len(), 1);
    assert_eq!(
        m.equality_representative(pick5),
        m.equality_representative(pick5b)
    );
}

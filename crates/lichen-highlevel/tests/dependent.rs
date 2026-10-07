//! The dependent-type feature: a type is a lazy computation over a value —
//! `if x > 0 then int else float` is an unevaluated `Index` over the branch
//! types with the parameter as its condition.  It stays lazy until forced,
//! so the branch selection is only resolved once the argument binds: the
//! same template yields a different type per argument.
//!
//! The checker's expression IR cannot express conditionals yet, so these
//! build lowlevel graphs directly and exercise the laziness + unification
//! rules the highlevel layer will sit on.

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

/// An `Index` node over `[branches, condition]` — the dependent-codomain
/// stand-in.  Like `if cond then a else b`, it stays lazy until its
/// condition is bound and then selects one branch.
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
    // f(x) = [x, if x > 0 then int else float]: the returned pair's second
    // element is the dependent type — an unevaluated Index over [float, int]
    // with the parameter as its condition.  The Index's operand array is
    // part of the template's scope, so each apply's clone rewrites the
    // condition to the fresh parameter clone.
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

    // applied to 1: the cloned condition binds, and forcing the codomain
    // selects the `int` branch
    let one = usize_node(&mut m, root, 1);
    let call = apply_node(&mut m, root, f, one);
    let value = m.evaluate_node_deep(call, None).unwrap();
    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert!(matches!(
        m.node_value(AnyNodeId::Dynamic(ids[1])),
        Some(HighProgramValue::LowValue(LowValue::USize(1)))
    ));

    // applied to 0: the same template picks the `float` branch
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
    // Boundary: a dependent function's codomain (`[0, 1][x]`) meets a concrete
    // `1` while the parameter is still undecided (the function is passed as a
    // value, not applied).  **Unify does not evaluate**, so it merges and the
    // concrete value is what the class holds; the codomain's own computation is
    // what has to agree with it, and that comparison happens when the codomain
    // is read.  The two parameter values below are the two outcomes: the
    // instance that resolves to `1` agrees, the one that resolves to `0` does
    // not.
    let float = usize_node(&mut m, root, 0);
    let int = usize_node(&mut m, root, 1);
    let branches = array_node(&mut m, root, &[float, int]);

    // x = 1: the codomain computes to `1`, which is what the class holds → clean.
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

    // x = 0: the same shape resolves to `0`, which conflicts with the `1` the
    // class holds — reported when the computation runs.
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
    // A concrete expectation meets a computation whose operands are already
    // bound (a constant condition).  Unify does not evaluate it: the two
    // classes merge and the expectation is what the class holds.  The
    // computation runs when something reads it, and its result is compared
    // against that value then.
    let four_v = usize_node(&mut m, root, 4);
    let five_v = usize_node(&mut m, root, 5);
    let branches = array_node(&mut m, root, &[four_v, five_v]);
    let cond = usize_node(&mut m, root, 1);
    let pick_five = index_node(&mut m, root, branches, cond);

    // an equal expectation merges
    let five = usize_node(&mut m, root, 5);
    m.unify(five, pick_five);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(five),
        m.equality_representative(pick_five)
    );
    // Reading it runs the computation, which agrees with what the class holds.
    let _ = m.evaluate_node_deep(pick_five, None);
    assert!(
        m.unify_errors.is_empty(),
        "an equal expectation is not a conflict"
    );

    // an unequal one conflicts against the value — the computation was not
    // erased, and still reads 5
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
    // A concrete expectation meets an `Index` read over an undecided element
    // with a concrete index: the read resolves to a pure reference — the
    // operator node is aliased to the element — and the concrete value is
    // written onto it (pinning the element, the "monomorphized" trade).  The
    // read keeps its operation — the operand edge stays live so an apply's
    // clone can reach the element and enforce the pin — and a conflicting
    // expectation fails against the pinned value.
    let mut m = Module::new();
    let root = m.add_block(None);
    let cell = undecided_node(&mut m, root);
    let container = array_node(&mut m, root, &[cell]);
    let zero = usize_node(&mut m, root, 0);
    let read = index_node(&mut m, root, container, zero);
    let three = usize_node(&mut m, root, 3);
    m.unify(three, read);
    assert!(m.unify_errors.is_empty());
    // The read's **subscript** is only known by evaluating it, so the equation
    // "this read is that element" cannot be established at unify time: the
    // evaluation establishes it (`alias_read`), and that unification is what
    // carries the concrete value onto the element.
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

    // a conflicting expectation now fails against the pinned value
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

    // two different computations: each reads its own value, and the mismatch is
    // detected by the unify that follows — unify itself never computes.
    let _ = m.evaluate_node_deep(pick5, None);
    let _ = m.evaluate_node_deep(pick4, None);
    m.unify(pick5, pick4);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(pick5),
        m.equality_representative(pick4)
    );
    // each kept its own computed value — neither was erased onto the other
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
    // the earlier mismatch error persists in the collection
    assert_eq!(m.unify_errors.len(), 1);
    assert_eq!(
        m.equality_representative(pick5),
        m.equality_representative(pick5b)
    );
}

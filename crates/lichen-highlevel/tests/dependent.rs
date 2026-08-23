//! The dependent-type feature: a type is a lazy computation over a value —
//! `if x > 0 then int else float` is an unevaluated `Index` over the branch
//! types with the parameter as its condition.  It stays lazy until forced,
//! so the branch selection is only resolved once the argument binds: the
//! same template yields a different type per argument.
//!
//! The checker's expression IR cannot express conditionals yet, so these
//! build lowlevel graphs directly and exercise the laziness + unification
//! rules the highlevel layer will sit on.

use lichen_highlevel::program::{HighProgram, HighProgramValue};
use lichen_lowlevel::{BlockId, Module, NodeId, Operation, Operator};

fn usize_node(m: &mut Module<HighProgram>, block: BlockId, n: usize) -> NodeId {
    m.add_node(block, None, Some(HighProgramValue::USize(n)))
}

fn unbound_node(m: &mut Module<HighProgram>, block: BlockId) -> NodeId {
    m.add_node(block, None, Some(HighProgramValue::Parameterized))
}

fn array_node(m: &mut Module<HighProgram>, block: BlockId, ids: &[NodeId]) -> NodeId {
    let slice = m.blocks[block].arena.alloc_slice_copy(ids);
    m.add_node(
        block,
        None,
        Some(HighProgramValue::Array(std::ptr::slice_from_raw_parts(
            slice.as_ptr(),
            slice.len(),
        ))),
    )
}

/// An `Index` node over `[branches, condition]` — the dependent-codomain
/// stand-in.  Like `if cond then a else b`, it stays lazy until its
/// condition is bound and then selects one branch.
fn index_node(
    m: &mut Module<HighProgram>,
    block: BlockId,
    branches: NodeId,
    cond: NodeId,
) -> NodeId {
    let operands = array_node(m, block, &[branches, cond]);
    m.add_node(
        block,
        Some(Operation {
            operator: Operator::Index,
            operand: Some(operands),
        }),
        None,
    )
}

/// An `Apply` node with operand array `[function, argument]`.
fn apply_node(m: &mut Module<HighProgram>, block: BlockId, func: NodeId, arg: NodeId) -> NodeId {
    let operands = array_node(m, block, &[func, arg]);
    m.add_node(
        block,
        Some(Operation {
            operator: Operator::Apply,
            operand: Some(operands),
        }),
        None,
    )
}

fn array_ids(value: HighProgramValue) -> Vec<NodeId> {
    let HighProgramValue::Array(ptr) = value else {
        panic!("expected an array value")
    };
    unsafe { &*ptr }.to_vec()
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
    let x = unbound_node(&mut m, root);
    let float = usize_node(&mut m, root, 0);
    let int = usize_node(&mut m, root, 1);
    let branches = array_node(&mut m, root, &[float, int]);
    let codomain_operands = array_node(&mut m, root, &[branches, x]);
    let codomain = m.add_node(
        root,
        Some(Operation {
            operator: Operator::Index,
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
    );

    // applied to 1: the cloned condition binds, and forcing the codomain
    // selects the `int` branch
    let one = usize_node(&mut m, root, 1);
    let call = apply_node(&mut m, root, f, one);
    let value = m.evaluate_node_deep(call, None);
    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert!(matches!(
        m.nodes[ids[1]].value,
        Some(HighProgramValue::USize(1))
    ));

    // applied to 0: the same template picks the `float` branch
    let zero = usize_node(&mut m, root, 0);
    let call = apply_node(&mut m, root, f, zero);
    let value = m.evaluate_node_deep(call, None);
    assert!(m.unify_errors.is_empty());
    let ids = array_ids(value);
    assert!(matches!(
        m.nodes[ids[1]].value,
        Some(HighProgramValue::USize(0))
    ));
}

#[test]
fn a_concrete_type_is_never_bound_over_a_dependent_codomain() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // Boundary: a dependent function's codomain meets a concrete `int`
    // while the parameter is still unbound (the function is passed as a
    // value, not applied).  The computation cannot be forced — the unify
    // fails instead of silently binding `int` over it.
    let x = unbound_node(&mut m, root);
    let float = usize_node(&mut m, root, 0);
    let int = usize_node(&mut m, root, 1);
    let branches = array_node(&mut m, root, &[float, int]);
    let codomain = index_node(&mut m, root, branches, x);
    m.unify(int, codomain);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(int),
        m.equality_representative(codomain)
    );

    // Once the parameter is bound — a fresh instance, like each application's
    // clone — the same shape resolves: `int` against the x = 1 instance
    // merges...
    let x1 = unbound_node(&mut m, root);
    let codomain1 = index_node(&mut m, root, branches, x1);
    let one = usize_node(&mut m, root, 1);
    m.unify(x1, one);
    m.unify(int, codomain1);
    // the boundary error above persists in the collection
    assert_eq!(m.unify_errors.len(), 1);
    assert_eq!(
        m.equality_representative(int),
        m.equality_representative(codomain1)
    );

    // ...and `int` against the x = 0 instance (which is `float`) conflicts.
    let x0 = unbound_node(&mut m, root);
    let codomain0 = index_node(&mut m, root, branches, x0);
    let zero = usize_node(&mut m, root, 0);
    m.unify(x0, zero);
    m.unify(int, codomain0);
    assert_eq!(m.unify_errors.len(), 2);
    assert_ne!(
        m.equality_representative(int),
        m.equality_representative(codomain0)
    );
}

#[test]
fn a_resolvable_computation_is_forced_and_compared() {
    let mut m = Module::new();
    let root = m.add_block(None);
    // A concrete expectation meets an unevaluated computation whose operands
    // are already bound (a constant condition): it is forced, and the
    // comparison happens against the computed value.
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

    // an unequal one conflicts against the computed value — the computation
    // was not erased, and still reads 5
    let four = usize_node(&mut m, root, 4);
    m.unify(four, pick_five);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(four),
        m.equality_representative(pick_five)
    );
    assert!(matches!(
        m.nodes[pick_five].value,
        Some(HighProgramValue::USize(5))
    ));
}

#[test]
fn a_resolvable_index_read_pins_its_element() {
    // A concrete expectation meets an `Index` read over an unbound element
    // with a concrete index: the read resolves to a pure reference — the
    // operator node is aliased to the element — and the concrete value is
    // written onto it (pinning the element, the "monomorphized" trade).  The
    // read keeps its operation — the operand edge stays live so an apply's
    // clone can reach the element and enforce the pin — and a conflicting
    // expectation fails against the pinned value.
    let mut m = Module::new();
    let root = m.add_block(None);
    let cell = unbound_node(&mut m, root);
    let container = array_node(&mut m, root, &[cell]);
    let zero = usize_node(&mut m, root, 0);
    let read = index_node(&mut m, root, container, zero);
    let three = usize_node(&mut m, root, 3);
    m.unify(three, read);
    assert!(m.unify_errors.is_empty());
    assert_eq!(
        m.equality_representative(three),
        m.equality_representative(cell),
        "the read aliased the element"
    );
    assert!(matches!(
        m.nodes[read].value,
        Some(HighProgramValue::USize(3))
    ));
    assert!(
        m.nodes[read].operation.is_some(),
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

    // two different computations: both force, and the mismatch is detected
    m.unify(pick5, pick4);
    assert_eq!(m.unify_errors.len(), 1);
    assert_ne!(
        m.equality_representative(pick5),
        m.equality_representative(pick4)
    );
    // each kept its own computed value — neither was erased onto the other
    assert!(matches!(
        m.nodes[pick5].value,
        Some(HighProgramValue::USize(5))
    ));
    assert!(matches!(
        m.nodes[pick4].value,
        Some(HighProgramValue::USize(4))
    ));

    // equal computations merge
    let cond1b = usize_node(&mut m, root, 1);
    let pick5b = index_node(&mut m, root, branches, cond1b);
    m.unify(pick5, pick5b);
    // the earlier mismatch error persists in the collection
    assert_eq!(m.unify_errors.len(), 1);
    assert_eq!(
        m.equality_representative(pick5),
        m.equality_representative(pick5b)
    );
}

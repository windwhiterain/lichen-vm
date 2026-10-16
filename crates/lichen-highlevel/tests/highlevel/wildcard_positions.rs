//! Where a lambda's value positions stand: the parameter pair's own slot. See
//! docs/notes/lowlevel-vm.md.

use lichen_highlevel::ir::BinOp;
use lichen_highlevel::program::{HighProgram, ProgramImpl, ValueType};
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_testkit::{Probe, Writer};
use lichen_utils::extend::AsEnum;

/// `f = (\n. (\x. x + 0) n); f 3` — an operation over an applied parameter.
fn applied_parameter() -> Probe<ProgramImpl> {
    let w = Writer::new();
    let f = w.binding("f");
    let n = w.param("n");
    let x = w.param("x");
    let zero = w.int("zero", 0);
    let sum = w.binop("sum", BinOp::Add, x, zero);
    let inner = w.lam("inner", x, sum);
    let call = w.app("call", inner, n);
    w.close_binding(f, n, call, false);
    let three = w.int("three", 3);
    let root = w.app("root", f, three);
    Probe::build(w.finish(root))
}

/// The pair `node` and its own value slot, with the slot's value.
fn pair_value<P>(module: &Module<P>, node: NodeId) -> (NodeId, Option<LowValue>)
where
    P: HighProgram,
    P::Value: ValueType,
{
    let Some(LowValue::Array(array)) = module
        .node_value(AnyNodeId::Dynamic(node))
        .and_then(|v| v.as_enum())
    else {
        panic!("node {node:?} is not a pair");
    };
    // SAFETY: a live payload of the module under test.
    let slot = lichen_testkit::dyn_node(unsafe { array.items() }[0].node);
    (
        slot,
        module
            .node_value(AnyNodeId::Dynamic(slot))
            .and_then(|v| v.as_enum()),
    )
}

#[test]
fn an_applied_parameter_still_computes() {
    let mut probe = applied_parameter();
    assert!(probe.ok(), "the shape checks");
    assert_eq!(probe.usize_of("root"), 3, "`f 3` is the body's answer");
}

#[test]
fn every_parameter_owns_an_undecided_value_position() {
    let probe = applied_parameter();
    let (n_slot, n_value) = pair_value(&probe.checked().module, probe.pair_of("n"));
    let (x_slot, x_value) = pair_value(&probe.checked().module, probe.pair_of("x"));
    assert_eq!(
        n_value, None,
        "the outer parameter's value position is undecided, not a value"
    );
    assert_eq!(x_value, None);
    assert_ne!(
        n_slot, x_slot,
        "each parameter owns its own cell, so an apply clones it per call"
    );
}

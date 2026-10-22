//! A `compute.Buf Float` element class read from a cell nothing has filled yet.
//! See `docs/notes/compile-order-class.md`.
//!
//! # Invariant
//!
//! **The element class is a fact about the parameter's declaration, not about what
//! the module evaluated first.** `Par` names `.in In` and `.out OutE`, and both name
//! a `compute.Buf Float`, so the fragment is `Float` on every side. The program text
//! is `buf_element_class_recorded.lichen`, which the compiler CLI runs unchanged.

use super::common;

use lichen_compute::ComputeValue;
use lichen_highlevel::program::TypeValue;
use lichen_kernel_ir::ScalarClass;
use lichen_lowlevel::AnyNodeId;
use lichen_utils::extend::AsEnum;

/// The recorded program: the kernel is forced by the recorded step, not by the root.
const PROGRAM: &str = include_str!("buf_element_class_recorded.lichen");

/// The module does hold a decided `Float` element type, so the class is known somewhere.
///
/// # Invariant
///
/// The program declares `compute.Buf Float` twice, so a decided `Float` type value and a
/// decided class for it exist in the module whatever the fragment says. This is the
/// *known* side of the propagation gap; the fragment tests below are the *needed* side.
#[test]
fn the_module_holds_a_decided_float_element_type() {
    let (module, _value, _root) = common::run(PROGRAM);
    let decided = module
        .nodes
        .iter()
        .filter(|(id, _node)| {
            module
                .node_value(AnyNodeId::Dynamic(*id))
                .is_some_and(|value| {
                    AsEnum::<TypeValue>::as_enum(&value) == Some(TypeValue::TypeFloat)
                })
        })
        .count();
    assert!(
        decided > 0,
        "`compute.Buf Float` is declared, so the module holds a decided `Float` type value"
    );
    // **The definition keeps its parameter**: an application supplies `T`; only an
    // instance is asked for a decided class.
    let reads: Vec<_> = common::buf_element_reads(&module)
        .into_iter()
        .filter(|(node, _read)| module.node_origin(*node).is_some())
        .collect();
    assert!(
        !reads.is_empty(),
        "the module has instances of `compute.Buf Float`"
    );
    assert!(
        reads.iter().all(|(_node, read)| read == "Float"),
        "and every instance's element cell reads `Float`: {reads:?}"
    );
}

/// The compiled fragment names the element class the parameter declares.
///
/// # Invariant
///
/// **This fails on `dev`, and its output is the defect.** `walk_role` reads the element
/// through `low_type_of_slot`, which is the class the parameter's own `Buf Float` states.
/// The cell it reads belongs to the type term `static_node_apply` cloned out of the frozen
/// `compute.lichen`, and nothing has merged that cell into a decided `Float` class yet.
#[test]
fn the_compiled_fragment_names_the_declared_element_class() {
    let (module, _value, _root) = common::run(PROGRAM);
    let fragment = common::sole_fragment(&module);
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.in In` and `In` is `struct{{.d (compute.Buf Float)}}`"
    );
    assert_eq!(
        (fragment.outputs, fragment.output_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.out OutE` and `OutE` is `struct{{.e (compute.Buf Float)}}`"
    );
}

/// The recorded graph node carries the program's own element class.
///
/// # Invariant
///
/// The recording is the artefact a device run replays, so a class the compiler read from
/// an empty cell is carried into it whole: this fails with `in [Int] out [Int]`.
#[test]
fn the_recorded_node_carries_the_declared_element_class() {
    let (_module, value, _root) = common::run(PROGRAM);
    let ComputeValue::Graph(id, _) =
        AsEnum::<ComputeValue>::as_enum(&value).expect("the root is the recorded graph")
    else {
        panic!("the step recorded no graph");
    };
    let graph = lichen_compute::compute::graph::lookup(id).expect("the graph is registered");
    assert_eq!(
        graph.len(),
        1,
        "a step that dispatches one kernel records one node: {graph:?}"
    );
    let lichen_graph_ir::Node::Kernel(kernel) = &graph.nodes()[0];
    assert_eq!(
        kernel.fragment.input_classes,
        vec![ScalarClass::Float],
        "the recorded node reads the one `compute.Buf Float` the dispatch passes"
    );
    assert_eq!(
        kernel.fragment.output_classes,
        vec![ScalarClass::Float],
        "and writes the one `compute.Buf Float` the dispatch returns"
    );
}

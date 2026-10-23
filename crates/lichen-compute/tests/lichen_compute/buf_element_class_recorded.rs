//! A `compute.Buf` element class read from a cell nothing has filled yet.
//! See `docs/notes/compile-order-class.md`.
//!
//! # Invariant
//!
//! **The element class is a fact about the declaration, not about what the module
//! evaluated first.** `Par` names `.in In` and the codomain names `OutE`, and both
//! hold a `Float` buffer, so the fragment is `Float` on every side. The program text is
//! `buf_element_class_recorded.lichen`, which the compiler CLI runs unchanged.

use super::common;

use lichen_compute::ComputeValue;
use lichen_highlevel::program::TypeValue;
use lichen_highlevel::shape::{TypeRef, field_type};
use lichen_kernel_ir::ScalarClass;
use lichen_lowlevel::AnyNodeId;
use lichen_utils::extend::AsEnum;

/// The recorded program: the kernel is forced by the recorded step, not by the root.
const PROGRAM: &str = include_str!("buf_element_class_recorded.lichen");

/// The module does hold a decided `Float` element type, so the class is known somewhere.
///
/// # Invariant
///
/// The program declares a `Float` buffer twice, so a decided `Float` type value and a
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
        "a `Float` buffer is declared, so the module holds a decided `Float` type value"
    );
    // The wrapper's second field states its type: a type constant, or nothing for a
    // `_` argument.
    let reads = common::buf_element_reads(&module);
    assert!(
        !reads.is_empty(),
        "the module carries `Buf`-shaped type terms"
    );
    for (id, _read) in &reads {
        let slot = AnyNodeId::Dynamic(*id);
        let element = [TypeRef::Term(slot), TypeRef::Slot(slot)]
            .into_iter()
            .find_map(|ty| field_type(&module, ty, 1));
        let Some(element) = element else {
            continue;
        };
        let value = module
            .element_value_node(element)
            .and_then(|node| node.dynamic())
            .and_then(|node| module.node_value(AnyNodeId::Dynamic(node)));
        if let Some(value) = value {
            assert!(
                AsEnum::<TypeValue>::as_enum(&value).is_some(),
                "`{id:?}`'s element field names a type or nothing, not a value: {value:?}"
            );
        }
    }
}

/// The compiled fragment names the element class the declaration states.
///
/// # Invariant
///
/// The buffer's type carries its element type, its length and its payload, so the
/// element class is that type's first shape position — `Float` here on both sides.
/// Nothing about the declaration changed between the two programs of the pair, so a
/// compiler that reads the element from the declaration answers `Float` for both.
#[test]
fn the_compiled_fragment_names_the_declared_element_class() {
    let (module, _value, _root) = common::run(PROGRAM);
    let fragment = common::sole_fragment(&module);
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`Par` declares `.in In` and `In` is `struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}}`"
    );
    assert_eq!(
        (fragment.outputs, fragment.output_classes.clone()),
        (1, vec![ScalarClass::Float]),
        "`OutE` is the codomain and it is `struct{{.e (compute.Buf ((compute.BufParam _)(.type Float, .length 1)))}}`"
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
        "the recorded node reads the one `Float` buffer the dispatch passes"
    );
    assert_eq!(
        kernel.fragment.output_classes,
        vec![ScalarClass::Float],
        "and writes the one `Float` buffer the dispatch returns"
    );
}

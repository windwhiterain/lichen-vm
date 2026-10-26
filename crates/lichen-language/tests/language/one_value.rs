//! One call site, one value.
//!
//! # Invariant
//!
//! A call a body binds once yields **one** value: every use of the binding
//! resolves to the same node, so a lowering can name the declaration its own
//! write names (`docs/notes/kernel-buffer-declarations.md`).
//!
//! The compute walk still refuses a write whose target is a clone's payload:
//! its `declared_ordinal` does not canonicalize it.

use lichen_language::compile;

/// A wrapper that returns a struct instance, bound once and used twice.
const PROGRAM: &str = "\
S := _x => {.a @any : Int}
f := (x : Int) => S{.a x}
y := f 1
(y, y)
";

/// The two uses of `y`, as the nodes the tuple holds.
fn uses(source: &str) -> (lichen_lowlevel::NodeId, lichen_lowlevel::NodeId) {
    let report = compile(source);
    assert!(report.ok(), "the program checks: {:?}", report.diagnostics);
    let mut build = report.build.expect("a build");
    // The root's value is a read when the root is a call: evaluate it first.
    build.module.evaluate_node_deep(build.root_val, None);
    // SAFETY: the root's value is a live node of this module.
    let items = unsafe { build.module.array_items(build.root_val) }.expect("the tuple");
    assert_eq!(items.len(), 2, "the tuple holds the two uses");
    (
        items[0].node.dynamic().expect("a dynamic node"),
        items[1].node.dynamic().expect("a dynamic node"),
    )
}

#[test]
fn a_call_site_yields_one_value() {
    let (first, second) = uses(PROGRAM);
    assert_eq!(
        first, second,
        "the two uses of `y` are one value: the call site yields one node"
    );
}

/// The same binding inside a body: the shape a kernel body writes.
#[test]
fn a_call_site_in_a_body_yields_one_value() {
    let (first, second) = uses(
        "\
S := _x => {.a @any : Int}
f := (x : Int) => S{.a x}
g := (n : Int) => {
  y := f n
  (y, y)
}
g 1
",
    );
    assert_eq!(
        first, second,
        "the two uses of `y` are one value: the call site yields one node"
    );
}

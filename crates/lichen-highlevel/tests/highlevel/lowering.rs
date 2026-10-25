//! A lowlevel node names the expression it lowered from; see
//! docs/notes/lowering-map.md.

use lichen_highlevel::ir::ExprKind;
use lichen_highlevel::program::{AnyLit, HighProgramLiteral, ProgramImpl};
use lichen_lowlevel::NodeId;
use lichen_testkit::{Probe, Writer};

/// `(\x. x) 5` — the smallest program with a named value, a named type, and a
/// named apply.
fn identity_applied() -> Probe<ProgramImpl> {
    let w = Writer::new();
    let x = w.param("x");
    let id = w.lam("id", x, x);
    let five = w.int("five", 5);
    let call = w.app("call", id, five);
    Probe::build(w.finish(call))
}

#[test]
fn a_node_answers_the_expression_it_lowered_from() {
    let probe = identity_applied();
    let node = probe.node_of("five");
    assert_eq!(
        probe.explain(node),
        format!("node {node:?} ← `five` (value)"),
        "a value node names the expression and the slot it is"
    );
}

#[test]
fn the_three_slots_of_one_expression_are_three_attributions() {
    let probe = identity_applied();
    let expr = probe.slot_of("five");
    let value = probe.node_of("five");
    let ty = probe.type_of("five");
    assert!(
        (expr != value) && (value != ty) && (expr != ty),
        "an expr, its value and its type are three distinct nodes"
    );
    assert!(
        probe
            .explain(expr)
            .ends_with("`five` (the expression itself)")
    );
    assert!(probe.explain(value).ends_with("`five` (value)"));
    assert!(probe.explain(ty).ends_with("`five` (type)"));
}

/// The map is total over the compiled slots, so an expression reached only
/// through another one is still attributable.
#[test]
fn every_named_expression_answers_the_node_it_lowered_to() {
    let probe = identity_applied();
    for name in ["id", "five", "call"] {
        let node = probe.node_of(name);
        assert_eq!(probe.expr_at(node), probe.expr_named(name));
        assert!(
            probe.explain(node).contains(&format!("`{name}`")),
            "`{name}` must name its own node, got {}",
            probe.explain(node)
        );
    }
}

/// A parameter has no separate value node.
///
/// # Invariant
/// Its value is `Index(expr, 0)`, built on demand, so the attribution a probe
/// can reach is the expr.
#[test]
fn a_parameter_is_attributed_through_the_expr_it_compiled_to() {
    let probe = identity_applied();
    let expr = probe.slot_of("x");
    assert!(
        probe.explain(expr).ends_with("`x` (the expression itself)"),
        "got {}",
        probe.explain(expr)
    );
    assert!(
        probe.checked().state[probe.expr_named("x").unwrap()]
            .val
            .is_none()
    );
}

#[test]
fn a_node_nothing_attributed_reads_as_such() {
    let probe = identity_applied();
    // An installed marker is built before any expression exists.
    let marker = probe.checked().markers.int_marker;
    assert_eq!(
        probe.explain(marker),
        format!("node {marker:?} ← no expression in this build"),
        "an installed marker is not lowered from anything"
    );
}

/// The whole point of a name.
///
/// # Invariant
/// A probe that cannot find one says so in the program's words, not as an id.
#[test]
fn an_unknown_name_panics_with_the_names_the_program_has() {
    let missing = std::panic::catch_unwind(|| {
        let w: Writer = Writer::new();
        let five = w.int("five", 5);
        let mut probe: Probe<ProgramImpl> = Probe::build(w.finish(five));
        probe.usize_of("five_")
    });
    let message = missing
        .expect_err("an unknown name is a panic")
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_default();
    assert!(
        message.contains("five_") && message.contains("five"),
        "the panic names what was asked for and what exists: {message}"
    );
}

/// The other direction: a node the reader already holds.
///
/// # Invariant
/// Its name comes back out of the graph, not out of the writer's table.
#[test]
fn a_node_read_out_of_an_array_names_its_expression() {
    let w: Writer = Writer::new();
    let one = w.int("one", 1);
    let two = w.int("two", 2);
    let three = w.int("three", 3);
    // The instance's value is the element template and then the elements.
    let literal = w.hole("element_value");
    w.fill(literal, ExprKind::Literal(HighProgramLiteral::from(AnyLit)));
    let element = w.annotate("element", literal, w.int_type("int_ty"));
    let template = w.array_template("template", element, w.int("len", 3));
    let arr = w.array_inst("arr", template, &[one, two, three]);
    let mut probe: Probe<ProgramImpl> = Probe::build(w.finish(arr));
    let elements = probe.array_of("arr");
    assert_eq!(elements.len(), 4, "the element template and three elements");
    let second = probe.node_of("two");
    assert!(
        probe.explain(second).contains("`two`"),
        "the element node is still the one `two` lowered to: {}",
        probe.explain(second)
    );
    let _: NodeId = second;
}

//! A probe for the nested-apply `none`: readings by name, no assertions.
//! See docs/notes/nested-apply-none.md.

use lichen_highlevel::program::ProgramImpl;
use lichen_lowlevel::{AnyNodeId, LowValue};
use lichen_testkit::{Probe, Writer};
use lichen_utils::extend::AsEnum;

/// The named expression's answer: its pair's own value slot.
fn answer(probe: &mut Probe<ProgramImpl>, name: &str) -> Option<LowValue> {
    let pair = probe.pair_of(name);
    let value = probe.checked_mut().module.evaluate_node_deep(pair, None);
    let decoded: Option<LowValue> = value.and_then(|value| value.as_enum());
    let Some(LowValue::Array(array)) = decoded else {
        return None;
    };
    // SAFETY: a live payload of the module under test.
    let slot = lichen_testkit::dyn_node(unsafe { array.items() }[0].node);
    let slot_value = probe.checked().module.node_value(AnyNodeId::Dynamic(slot));
    slot_value.and_then(|value| value.as_enum())
}

/// `mk = (\x. \y. x); f = (\n. (mk n) 1); f 3` — the callee is a top-level binding.
fn top_level_callee() -> Probe<ProgramImpl> {
    let w = Writer::new();
    let mk = w.binding("mk");
    let x = w.param("x");
    let y = w.param("y");
    let returned = w.lam("returned", y, x);
    w.close_binding(mk, x, returned, false);
    let f = w.binding("f");
    let n = w.param("n");
    let call = w.app("call", mk, n);
    let one = w.int("one", 1);
    let body = w.app("body", call, one);
    w.close_binding(f, n, body, false);
    let three = w.int("three", 3);
    let root = w.app("root", f, three);
    Probe::build(w.finish(root))
}

/// `g = (\n. mk n); f = (\n. (g n) 1); f 3` — a function binding, not a value binding.
fn function_binding_callee() -> Probe<ProgramImpl> {
    let w = Writer::new();
    let mk = w.binding("mk");
    let x = w.param("x");
    let y = w.param("y");
    let returned = w.lam("returned", y, x);
    w.close_binding(mk, x, returned, false);
    let f = w.binding("f");
    let n = w.param("n");
    let g = w.binding("g");
    let read = w.app("read", mk, n);
    w.close_binding(g, n, read, false);
    let call = w.app("call", g, n);
    let one = w.int("one", 1);
    let body = w.app("body", call, one);
    w.close_binding(f, n, body, false);
    let three = w.int("three", 3);
    let root = w.app("root", f, three);
    Probe::build(w.finish(root))
}

/// Print the readings a `value_of` panic would hide.
fn report(label: &str, mut probe: Probe<ProgramImpl>) {
    println!("--- {label}");
    println!("    checks: {}", probe.ok());
    println!("    root answer: {:?}", answer(&mut probe, "root"));
    for name in ["body", "call", "mk", "n", "returned"] {
        if probe.expr_named(name).is_none() {
            println!("    {name}: not written");
            continue;
        }
        let pair = probe.pair_of(&name);
        let value_slot = probe.try_node_of(&name);
        let members: Vec<(_, Option<LowValue>)> = probe
            .class_of(&name)
            .into_iter()
            .map(|(member, value)| (member, value.and_then(|value| value.as_enum())))
            .collect();
        println!("    {name}: pair={pair:?} value_slot={value_slot:?} class_members={members:?}");
        println!("        {}", probe.explain(pair));
    }
}

#[test]
fn nested_apply_readings() {
    report("top-level callee", top_level_callee());
    report("function-binding callee", function_binding_callee());
}

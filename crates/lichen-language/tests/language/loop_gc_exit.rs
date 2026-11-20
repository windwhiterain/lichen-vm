//! A `@loop` base arm that is a `@gc` scope inside another `@gc`. See `docs/notes/gc-block.md`.

use lichen_language::program::LangProgram;
use lichen_language::{compile, run};

/// The nested shape: the body is a `@gc` scope, and the base arm is another one.
const NESTED: &str =
    "@loop f := 's -> @gc { if s(0) == 0 then @gc (s(1), s(1)) else f (s(0) - 1, s(1)) }\nf (2, 5)";

/// The same loop with neither scope marked.
const UNMARKED: &str =
    "@loop f := 's -> if s(0) == 0 then (s(1), s(1)) else f (s(0) - 1, s(1))\nf (2, 5)";

/// Compile a program, asserting it checks; returns its build.
fn build(source: &str) -> lichen_highlevel::checker::Build<LangProgram> {
    let report = compile(source);
    assert!(
        report.ok(),
        "expected {source:?} to check, got: {:?}",
        report.diagnostics
    );
    report.build.unwrap()
}

#[test]
fn a_nested_gc_base_arm_answers_through_both_releases() {
    let out = run::evaluate(NESTED).expect("the program runs");
    assert_eq!(
        out, "(raw 5, raw 5): Tuple",
        "the compound exit survives the inner scope's release and the outer one's"
    );
}

#[test]
fn a_nested_gc_base_arm_answers_what_the_unmarked_loop_answers() {
    let marked = run::evaluate(NESTED).expect("the marked loop runs");
    let plain = run::evaluate(UNMARKED).expect("the unmarked loop runs");
    assert_eq!(marked, plain, "the two scopes change no answer");
}

#[test]
fn a_loop_bodys_gc_scopes_outlive_the_check() {
    let marked = build(NESTED);
    let plain = build(UNMARKED);
    // The body is undecided at check time, so each `@gc` keeps its block until the run decides it.
    assert_eq!(
        marked.module.blocks.len(),
        plain.module.blocks.len() + 2,
        "each `@gc` marker keeps its own block until the run decides it"
    );
}

#[test]
fn every_node_a_nested_gc_loop_names_is_live() {
    let build = build(NESTED);
    let live = |node: &Option<lichen_lowlevel::NodeId>| {
        node.is_none_or(|node| build.module.nodes.contains_key(node))
    };
    for state in &build.state {
        assert!(
            live(&state.term) && live(&state.val) && live(&state.ty) && live(&state.attr),
            "a named node outlived its block"
        );
    }
}

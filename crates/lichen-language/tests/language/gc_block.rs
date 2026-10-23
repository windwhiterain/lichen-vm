//! `@gc e` — the expression is checked into a block of its own, released
//! once decided. See `docs/notes/gc-block.md`.

use lichen_language::program::LangProgram;
use lichen_language::{compile, run};

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
fn a_gc_block_evaluates_to_its_tail() {
    let out = run::evaluate("x = @gc { a = 2; a * 3 }\nx").expect("the program runs");
    assert_eq!(out, "6: Int", "the block's value is its tail");
}

#[test]
fn a_gc_marker_over_an_application_evaluates_the_call() {
    let out = run::evaluate("f = n => n + 1\n@gc f 41").expect("the program runs");
    assert_eq!(out, "42: Int");
}

#[test]
fn a_gc_block_releases_its_scope() {
    let plain = build("let a = 2\nlet b = 3\na + b");
    let marked = build("x = @gc { a = 2; b = 3; a + b }\nx");
    assert_eq!(
        marked.module.blocks.len(),
        plain.module.blocks.len(),
        "the scope's block is released: the marked build must be back to one block"
    );
}

#[test]
fn a_released_expression_reads_as_having_no_slot() {
    let build = build("x = @gc { f = y => y + 1\n5 }\nx");
    // The scope's root survives: the enclosing block reads it.
    assert!(
        build.state.iter().any(|state| state.term.is_some()),
        "the scope's root moved up and is still named"
    );
    // Every node the build names is live: `nodes` is a `SlotMap`.
    let live = |node: &Option<lichen_lowlevel::NodeId>| {
        node.is_none_or(|node| build.module.nodes.contains_key(node))
    };
    for state in &build.state {
        assert!(live(&state.term) && live(&state.val) && live(&state.ty) && live(&state.attr));
    }
}

#[test]
fn a_gc_scope_inside_a_body_is_cloned_per_call() {
    let out = run::evaluate("f = n => @gc { a = n + 1\nb = a * 2\nb }\nf 3 + f 4")
        .expect("the program runs");
    assert_eq!(out, "18: Int", "each call collects its own scope");
}

#[test]
fn a_gc_block_yields_its_record() {
    let out = run::evaluate("@gc { a = 1; b = 2 }").expect("the program runs");
    assert!(
        out.contains("struct"),
        "the struct-returning form is a gc block too, got {out:?}"
    );
}

#[test]
fn an_undecided_scope_is_not_collected() {
    let build = build("@gc { let a = _\na }");
    // `_` never decides, so the scope's root is undecided: nothing is
    // released, and the build still answers with the hole.
    assert!(build.module.nodes.len() > 1, "nothing was released");
}

#[test]
fn a_closure_released_from_a_scope_is_still_callable() {
    // The closure's template lives in the scope's block; the release must move it too.
    let out = run::evaluate("mk = n => @gc { f = x => x + n\nf }\na = mk 1\nb = a 2\nb")
        .expect("the program runs");
    assert_eq!(out, "3: Int", "the escaped closure still calls");
}

#[test]
fn a_closure_stored_in_an_outside_array_is_still_callable() {
    // The scope's value is stored outside it: the closure must still call afterwards.
    let out = run::evaluate("y = [@gc (x => x + 1)]\n(y[0]) 3").expect("the program runs");
    assert_eq!(out, "4: Int");
}

#[test]
fn a_gc_block_calls_a_curried_binding_per_element() {
    // The example's program: a @gc block calling a curried expr, twice in one array.
    let out = run::evaluate(
        "add = x => y => x + y\ndouble = n => @gc { a = n + 1\nb = add a a\nb }\n@gc [double 3, double 7]",
    )
    .expect("the program runs");
    assert_eq!(out, "[8, 16]: array{Int, 2}");
}

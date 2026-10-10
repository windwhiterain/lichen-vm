//! Tests for the incremental [`BufferSession`] and the error-block machinery.

use lichen_highlevel::ir::ExprKind;

use super::{edit_span, splice_program};
use crate::ast::{Expr, Stmt};
use crate::diag::Stage;
use crate::lex;
use crate::parse::{self, Parsed};
use crate::program::LangProgram;
use crate::session::BufferSession;

/// The parsed program + diagnostics for a source (lex + parse).
fn parsed(source: &str) -> Parsed {
    let tokens = lex::lex(source).tokens;
    parse::parse(&tokens)
}

/// The resolved content key of a source (lex, parse, resolve, key).
fn content_key_of(source: &str) -> Vec<u64> {
    let tokens = lex::lex(source).tokens;
    let mut program = parse::parse(&tokens).program;
    crate::resolve::resolve(&mut program, &[]);
    crate::resolve::content_key(&program)
}

#[test]
fn a_recovered_error_block_carries_a_byte_range() {
    // A recovered error block's byte range masks the broken `)` token, so a diff
    // can exclude the region.
    let source = "a = ); b = 2; b";
    let Parsed { program, errors } = parsed(source);
    assert!(!errors.is_empty(), "the stray ')' is a parse error");
    let Stmt::Binding(binding) = &program.statements[0].stmt else {
        panic!("the first statement is a binding, got {:?}", program);
    };
    let Expr::Err { range, .. } = &binding.value else {
        panic!(
            "the broken binding value is a masked error block, got {:?}",
            binding.value
        );
    };
    // The mask covers the broken region (non-degenerate) and stays in scope.
    assert!(range.0 < range.1, "a non-empty byte range: {range:?}");
    assert!(
        range.1 as usize <= source.len(),
        "the range ends at the source:"
    );
    // The recovered error regions are surfaced on the program as byte-range
    // masks (the frontend's diff/Mask record).
    assert!(
        !program.error_blocks.is_empty(),
        "the program carries its masks"
    );
    assert!(
        program.error_blocks.iter().any(|b| b.range == *range),
        "the mask list includes the recovered value's range: {:?}",
        program.error_blocks
    );
}

#[test]
fn an_error_block_lowers_to_errorblock_not_placeholder() {
    // A recovered parse error must lower to a distinct `ErrorBlock`, not `_`.
    let source = "a = ); b = 2; b";
    let tokens = lex::lex(source).tokens;
    let Parsed { mut program, .. } = parse::parse(&tokens);
    let ir = crate::compile::compile(&mut program).0;
    let error_blocks = ir
        .expr
        .iter()
        .filter(|e| matches!(e.kind, ExprKind::ErrorBlock))
        .count();
    assert!(
        error_blocks >= 1,
        "the error region lowers to an ErrorBlock"
    );
    // Here `_` is covered elsewhere; the broken region must not be a Placeholder.
    let placeholder_at_error = ir
        .expr
        .iter()
        .filter(|e| matches!(e.kind, ExprKind::Placeholder))
        .count();
    assert_eq!(
        placeholder_at_error, 0,
        "a broken region is not a `_` placeholder"
    );
}

#[test]
fn the_checker_skips_an_error_block_no_type_error() {
    // The masked region is skipped, so no type-level diagnostic comes from it.
    let report = crate::compile("a = ); b = 2; b");
    assert!(report.build.is_some(), "the partial program still builds");
    assert_eq!(
        report.build.as_ref().map(|b| b.ok),
        Some(true),
        "no checker failure from the masked region"
    );
    assert!(
        report.diagnostics.iter().all(|d| d.stage != Stage::Check),
        "the error block produces parse diagnostics, never a check diagnostic: {:?}",
        report.diagnostics
    );
}

#[test]
fn growing_an_error_block_reuses_the_established_build() {
    // Busy typing: growing the error block changes only the mask, so the build
    // is reused.
    let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
    let r0 = sess.compile();
    assert!(!r0.reused, "the first compile is a fresh build");
    assert!(r0.ok(), "the clean program checks");

    // `fix1 = (2` adds a binding, so the clean content changed: a fresh build.
    sess.push("fix1 = (2");
    let r1 = sess.compile();
    assert!(!r1.reused, "a new binding is a clean-content change");
    assert_eq!(
        r1.build.as_ref().map(|b| b.ok),
        Some(true),
        "the established program still checks; the broken value is skipped"
    );

    // `(2` -> `(22` grows only the masked error block: the build is reused.
    let before = r1.key;
    sess.push("2");
    let r2 = sess.compile();
    assert_eq!(r2.key, before, "the clean content signature is unchanged");
    assert!(r2.reused, "the established build is reused");
    assert_eq!(
        r2.build.as_ref().map(|b| b.ok),
        Some(true),
        "the reused build is still the (correct) established build"
    );
    // The established statements survived; the trailing broken binding was skipped.
    assert!(
        r2.diagnostics.iter().any(|d| d.stage == Stage::Parse),
        "the parse error for the still-unclosed paren is re-reported"
    );
}

#[test]
fn changing_clean_content_invalidates_the_cache() {
    let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
    let r0 = sess.compile();
    let sig0 = r0.key;

    // A clean-content edit (adding a real binding) must change the signature
    // and trigger a fresh build, not a stale reuse.
    sess.push("g = 5\n");
    let r1 = sess.compile();
    assert_ne!(r1.key, sig0);
    assert!(!r1.reused, "a clean-content change is a fresh build");
}

#[test]
fn typing_inside_an_unclosed_region_reuses_every_keystroke() {
    // Every keystroke inside a single masked error block grows only the mask, so
    // the established program is reused.
    let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
    let _ = sess.compile();
    sess.push("fix1 = (1");
    let r0 = sess.compile();
    assert!(!r0.reused, "the new binding is a clean change");
    let sig = r0.key;
    for digit in ["2", "3", "4"] {
        sess.push(digit);
        let r = sess.compile();
        assert_eq!(r.key, sig, "'{digit}': the clean signature is unchanged");
        assert!(
            r.reused,
            "'{digit}': a keystroke inside the mask reuses the build"
        );
        assert_eq!(r.build.as_ref().map(|b| b.ok), Some(true));
    }
}

#[test]
fn typing_a_long_unresolved_name_reuses_after_the_first_character() {
    // A long unresolved name is a name-resolution-only delta: extending it
    // changes nothing the lowering or check consume.
    let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
    let _ = sess.compile();
    sess.push("v"); // first character: a new (unresolved) name leaf appears.
    let r1 = sess.compile();
    assert!(
        !r1.reused,
        "the first character introduces a new name, a structural change"
    );
    assert_eq!(
        r1.build.as_ref().map(|b| b.ok),
        Some(true),
        "the unresolved name is masked, not fatal"
    );
    let sig = r1.key;
    for ch in "ery_long_variable_name".chars() {
        sess.push(&ch.to_string());
        let r = sess.compile();
        assert!(
            r.reused,
            "extending an unresolved name reuses the established build"
        );
        assert_eq!(r.key, sig, "the resolved structure is unchanged");
        assert_eq!(r.build.as_ref().map(|b| b.ok), Some(true));
        // The *current* name's resolve diagnostic is refreshed, not stale.
        assert!(
            r.diagnostics.iter().any(|d| d.stage == Stage::Resolve),
            "the current unresolved name is still reported"
        );
    }
}

#[test]
fn renaming_a_binding_consistently_reuses() {
    // The signature signs the resolution, not the spelling, so a consistent
    // rename reuses the established build.
    let mut sess = BufferSession::<LangProgram>::new("f = x => x + 1\nf 2\n");
    let r0 = sess.compile();
    assert!(!r0.reused);
    let sig = r0.key;
    sess.replace(0..sess.len(), "g = x => x + 1\ng 2\n");
    let r1 = sess.compile();
    assert_eq!(
        r1.key, sig,
        "a consistent rename keeps the resolved structure"
    );
    assert!(
        r1.reused,
        "the established build is reused across a consistent rename"
    );
    assert_eq!(r1.build.as_ref().map(|b| b.ok), Some(true));
}

/// The diagnostics of a report, as a comparable multiset (each rendered and
/// sorted; `Stage` is not `Ord`).
fn diag_set(report: &crate::session::SessionReport<crate::program::LangProgram>) -> Vec<String> {
    let mut v: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();
    v.sort();
    v
}

/// A report's key observable fields, in a comparable form.
///
/// # Invariant
///
/// `reused` is excluded: it depends on the session's history, not the source.
/// What must match is the content key, the check outcome and the diagnostics.
#[derive(PartialEq, Debug)]
struct ReportShape {
    key: Vec<u64>,
    build_ok: Option<bool>,
    diagnostics: Vec<String>,
}

fn shape(report: &crate::session::SessionReport<crate::program::LangProgram>) -> ReportShape {
    ReportShape {
        key: report.key.clone(),
        build_ok: report.build.as_ref().map(|b| b.ok),
        diagnostics: diag_set(report),
    }
}

#[test]
fn an_edited_session_compiles_identically_to_a_fresh_one() {
    // The incremental re-lex must never change the output: the report equals a
    // fresh session's after each edit.
    let cases: &[&[&str]] = &[
        // The source after each successive edit of the base program.
        &["a = 1\nz = 3\nf = x => a + x\nf 2\n"],
        &["a = 1\nf = x => a + x\nf 2\nzzz"],
        &["a = 99\nf = x => a + x\nf 2\n"],
        // A trailing binding makes the program a record program (a module): the
        // splice must reproduce the whole-program parse.
        &["a = 1\nf = x => a + x\nf 2\nner = 2"],
        &["a = 1\nf = x => a + x\nf 2"],
        &["a = 1\nf = x => a\nf 2\n"],
    ];
    for edit in cases {
        let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
        let _ = sess.compile();
        for (i, target) in edit.iter().enumerate() {
            // Apply the edit by replacing the whole source with the target.
            sess.replace(0..sess.len(), target);
            assert_eq!(
                shape(&sess.compile()),
                shape(&BufferSession::<LangProgram>::new(*target).compile()),
                "edit {i} to {target:?} diverged from a fresh compile"
            );
            assert_eq!(sess.source(), *target);
        }
    }
}

#[test]
fn a_bracket_swap_between_forms_rebuilds_instead_of_reusing() {
    // `[a, b]`, `(a, b)` and `<a, b>` lower differently, so a bracket swap
    // rebuilds.
    let forms = ["z = [1, 2]\nz\n", "z = (1, 2)\nz\n", "z = <1, 2>\nz\n"];
    for (i, target) in forms.iter().enumerate() {
        let previous = forms[(i + forms.len() - 1) % forms.len()];
        let mut sess = BufferSession::<LangProgram>::new(previous);
        let _ = sess.compile();
        sess.replace(0..sess.len(), target);
        let report = sess.compile();
        assert_eq!(
            shape(&report),
            shape(&BufferSession::<LangProgram>::new(*target).compile()),
            "the bracket swap {previous:?} -> {target:?} diverged from a fresh compile"
        );
        assert!(
            !report.reused,
            "the bracket swap {previous:?} -> {target:?} changed the resolved content, so it must rebuild"
        );
    }
}

/// Assert the window splice of `old` → `new` equals a whole-buffer parse.
fn assert_splice_equals_full_parse(old: &str, new: &str) {
    let old_starts = lex::line_starts(old);
    let old_tokens = lex::lex(old).tokens;
    let old_program = parse::parse(&old_tokens).program;
    let new_starts = lex::line_starts(new);
    let new_tokens = lex::lex(new).tokens;
    let parsed = splice_program(
        &old_tokens,
        &old_program,
        &old_starts,
        &new_tokens,
        &new_starts,
        edit_span(old, new).0,
        edit_span(old, new).1,
        edit_span(old, new).2,
    );
    assert!(
        parsed.is_some(),
        "the edit {old:?} -> {new:?} should be window-spliceable"
    );
    let out = parsed.unwrap();
    let (program, errors) = (out.program, out.errors);
    let full = parse::parse(&new_tokens);
    // The spliced statements/expr/ranges must equal a full parse's.
    assert_eq!(program.statements.len(), full.program.statements.len());
    assert_eq!(
        format!("{:?}", program.statements),
        format!("{:?}", full.program.statements)
    );
    assert_eq!(
        format!("{:?}", program.expr),
        format!("{:?}", full.program.expr)
    );
    assert_eq!(program.stmt_ranges, full.program.stmt_ranges);
    // The recovered error blocks are recomputed from the spliced AST.
    assert_eq!(
        format!("{:?}", program.error_blocks),
        format!("{:?}", full.program.error_blocks)
    );
    assert_eq!(errors.len(), full.errors.len());
}

#[test]
fn the_window_splice_reproduces_a_full_parse() {
    // Edits the splice must handle without a whole-buffer fallback.
    let base = "a = 1\nf = x => a + x\nf 2\n";
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 22\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + 1\nf 2\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 3\n");
    assert_splice_equals_full_parse(base, "g = 1\nf = x => a + x\nf 2\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 2\nner");
    assert_splice_equals_full_parse(base, "a = 1\nz = 3\nf = x => a + x\nf 2\n");
}

#[test]
fn a_splice_ending_in_a_binding_is_a_record_program() {
    // A program whose last statement is a binding is a record program (a module).
    let old = "a = 1\nf = x => a + x\nf 2\n";
    let new = "a = 1\nf = x => a + x\nner = (2";
    assert_splice_equals_full_parse(old, new);
    // An append *after* a tail expression (a tail program gains a trailing
    // binding, becoming a record program).
    let new2 = "a = 1\nf = x => a + x\nf 2\nner = (2";
    assert_splice_equals_full_parse(old, new2);
}

#[test]
fn the_resolved_content_key_tracks_resolution_not_spelling() {
    // The key is name-free and exact: a rename leaves it, a value change moves it.
    let base = content_key_of("f = x => x + 1\nf 2\n");
    assert_eq!(
        content_key_of("g = x => x + 1\ng 2\n"),
        base,
        "a rename reuses"
    );
    assert_ne!(
        content_key_of("f = x => x + 2\nf 2\n"),
        base,
        "a value edit rebuilds"
    );
}

#[test]
fn the_content_key_distinguishes_list_arities_around_an_apply() {
    // `Apply` concatenates two encodings with no separator, so list lengths keep
    // nested arities distinct.
    let split = "(1,) (2, 3)\n";
    let nested = "(1, (2,)) 3\n";
    assert!(
        parsed(split).errors.is_empty() && parsed(nested).errors.is_empty(),
        "both sources parse cleanly"
    );
    assert_ne!(
        content_key_of(split),
        content_key_of(nested),
        "a different tuple arity around an application is different content"
    );
}

#[test]
fn the_content_key_distinguishes_a_string_from_a_named_field_read() {
    // `Str` and `NamedFieldRead` once shared tag 24, encoding to the same key;
    // the tags are now distinct.
    let literal = format!("\"{}\"\n", "\u{2}ab");
    let named_field_read = "_.ab\n";
    assert!(
        parsed(&literal).errors.is_empty() && parsed(named_field_read).errors.is_empty(),
        "both sources parse cleanly"
    );
    assert_ne!(
        content_key_of(&literal),
        content_key_of(named_field_read),
        "a string literal is not a named field read"
    );
}

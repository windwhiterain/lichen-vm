//! Tests for the incremental [`BufferSession`] and the error-block machinery
//! it relies on (`Expr::Err` byte ranges, [`ExprKind::ErrorBlock`] lowering, the
//! checker's skip path).

use lichen_highlevel::ir::ExprKind;

use super::{edit_span, signature_full, signature_reuse, splice_program};
use crate::ast::{Expr, Stmt};
use crate::diag::Stage;
use crate::lex;
use crate::parse::{self, Parsed};
use crate::session::BufferSession;

/// The parsed program + diagnostics for a source (lex + parse).
fn parsed(source: &str) -> Parsed {
    let tokens = lex::lex(source).tokens;
    parse::parse(&tokens)
}

#[test]
fn a_recovered_error_block_carries_a_byte_range() {
    // `a = )` — a stray `)` is not an atom, so the binding's *value* is one
    // recovered error block whose byte range masks the broken `)` token.  The
    // parser previously kept only the (line, col); now it carries the byte
    // mask too, so a diff can exclude the region.
    let source = "a = ); b = 2; b";
    let Parsed { program, errors } = parsed(source);
    assert!(!errors.is_empty(), "the stray ')' is a parse error");
    let Stmt::Binding(binding) = &program.statements[0] else {
        panic!("the first statement is a binding, got {:?}", program);
    };
    let Expr::Err { range, .. } = &binding.value else {
        panic!("the broken binding value is a masked error block, got {:?}", binding.value);
    };
    // The mask covers the broken region (non-degenerate) and stays in scope.
    assert!(range.0 < range.1, "a non-empty byte range: {range:?}");
    assert!(range.1 as usize <= source.len(), "the range ends at the source:");
    // The recovered error regions are surfaced on the program as byte-range
    // masks (the frontend's diff/Mask record).
    assert!(!program.error_blocks.is_empty(), "the program carries its masks");
    assert!(
        program
            .error_blocks
            .iter()
            .any(|b| b.range == *range),
        "the mask list includes the recovered value's range: {:?}",
        program.error_blocks
    );
}

#[test]
fn an_error_block_lowers_to_errorblock_not_placeholder() {
    // A recovered parse error must NOT become the same highlevel construct as
    // an intentional `_` — that conflation is the leak the design fixes.  The
    // broken value lowers to a distinct `ExprKind::ErrorBlock`.
    let source = "a = ); b = 2; b";
    let tokens = lex::lex(source).tokens;
    let Parsed { program, .. } = parse::parse(&tokens);
    let ir = crate::compile::compile(&program).0;
    let error_blocks = ir
        .expr
        .iter()
        .filter(|e| matches!(e.kind, ExprKind::ErrorBlock))
        .count();
    assert!(error_blocks >= 1, "the error region lowers to an ErrorBlock");
    // The intentional-placeholder test in compile_tests covers `_`; here we
    // assert the *broken* region is never a Placeholder.
    let placeholder_at_error = ir
        .expr
        .iter()
        .filter(|e| matches!(e.kind, ExprKind::Placeholder))
        .count();
    assert_eq!(placeholder_at_error, 0, "a broken region is not a `_` placeholder");
}

#[test]
fn the_checker_skips_an_error_block_no_type_error() {
    // The whole pipeline: a stray `)` is a *parse* diagnostic, but the
    // checker must not report a type-level "expected X, found Y" from inside
    // the masked region (it is skipped).
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
    // The user's busy-typing case: typing an unfinished trailing piece first
    // changes the clean content (a new binding appears), so the first compile
    // is fresh; growing that *error block* changes only the mask — the clean
    // (beyond-error) content is unchanged, so the established build is reused.
    let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");
    let r0 = sess.compile();
    assert!(!r0.reused, "the first compile is a fresh build");
    assert!(r0.ok(), "the clean program checks");

    // An unfinished trailing binding: `fix1 = (2` (unclosed paren → an error
    // block for the value).  The clean content gains `fix1 = `, so this is a
    // fresh build.
    sess.push("fix1 = (2");
    let r1 = sess.compile();
    assert!(!r1.reused, "a new binding is a clean-content change");
    assert_eq!(
        r1.build.as_ref().map(|b| b.ok),
        Some(true),
        "the established program still checks; the broken value is skipped"
    );

    // Grow the broken region: `(2` → `(22` — only the masked error block grew.
    // The beyond-error content is unchanged, so the build is reused.
    let before = r1.signature;
    sess.push("2");
    let r2 = sess.compile();
    assert_eq!(r2.signature, before, "the clean content signature is unchanged");
    assert!(r2.reused, "the established build is reused");
    assert_eq!(
        r2.build.as_ref().map(|b| b.ok),
        Some(true),
        "the reused build is still the (correct) established build"
    );
    // The established statements survived: `a`, `f`, and the `f 2` application
    // are still in the IR, just the trailing broken binding was skipped.
    assert!(
        r2.diagnostics.iter().any(|d| d.stage == Stage::Parse),
        "the parse error for the still-unclosed paren is re-reported"
    );
}

#[test]
fn changing_clean_content_invalidates_the_cache() {
    let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");
    let r0 = sess.compile();
    let sig0 = r0.signature;

    // A clean-content edit (adding a real binding) must change the signature
    // and trigger a fresh build, not a stale reuse.
    sess.push("g = 5\n");
    let r1 = sess.compile();
    assert_ne!(r1.signature, sig0);
    assert!(!r1.reused, "a clean-content change is a fresh build");
}

#[test]
fn typing_inside_an_unclosed_region_reuses_every_keystroke() {
    // The editor's per-char case that DOES stay incremental: while a construct
    // is a single masked error block (an unclosed paren), every keystroke
    // grows only the mask — the clean structure is unchanged, so the
    // established program is reused per char.
    let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");
    let _ = sess.compile();
    sess.push("fix1 = (1");
    let r0 = sess.compile();
    assert!(!r0.reused, "the new binding is a clean change");
    let sig = r0.signature;
    for digit in ["2", "3", "4"] {
        sess.push(digit);
        let r = sess.compile();
        assert_eq!(r.signature, sig, "'{digit}': the clean signature is unchanged");
        assert!(r.reused, "'{digit}': a keystroke inside the mask reuses the build");
        assert_eq!(r.build.as_ref().map(|b| b.ok), Some(true));
    }
}

#[test]
fn typing_a_long_unresolved_name_reuses_after_the_first_character() {
    // The real editor case with a long identifier: a name that does not yet
    // resolve is a *name-resolution-only* delta — the structure is one `Name`
    // leaf, and its resolution stays the unresolved sentinel — so extending it
    // changes nothing the lowering/check consume, and only the resolve
    // diagnostic updates.
    let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");
    let _ = sess.compile();
    sess.push("v"); // first character: a new (unresolved) name leaf appears.
    let r1 = sess.compile();
    assert!(!r1.reused, "the first character introduces a new name, a structural change");
    assert_eq!(
        r1.build.as_ref().map(|b| b.ok),
        Some(true),
        "the unresolved name is masked, not fatal"
    );
    let sig = r1.signature;
    for ch in "ery_long_variable_name".chars() {
        sess.push(&ch.to_string());
        let r = sess.compile();
        assert!(r.reused, "extending an unresolved name reuses the established build");
        assert_eq!(r.signature, sig, "the resolved structure is unchanged");
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
    // The sound form of "exclude the name from the hash": the signature signs
    // the resolution (which binding each use resolves to), not the spelling, so
    // a *consistent* rename — same bindings, same uses, different names — reuses
    // the established build.  The name-free IR is genuinely identical.
    let mut sess = BufferSession::new("f = x => x + 1\nf 2\n");
    let r0 = sess.compile();
    assert!(!r0.reused);
    let sig = r0.signature;
    sess.replace(0..sess.len(), "g = x => x + 1\ng 2\n");
    let r1 = sess.compile();
    assert_eq!(r1.signature, sig, "a consistent rename keeps the resolved structure");
    assert!(r1.reused, "the established build is reused across a consistent rename");
    assert_eq!(r1.build.as_ref().map(|b| b.ok), Some(true));
}

/// The diagnostics of a report, as a comparable multiset (each rendered and
/// sorted; `Stage` is not `Ord`).
fn diag_set(report: &crate::session::SessionReport) -> Vec<String> {
    let mut v: Vec<String> = report
        .diagnostics
        .iter()
        .map(|d| format!("{:?}", d))
        .collect();
    v.sort();
    v
}

/// A report and its key observable fields, in a comparable form.  The `reused`
/// flag is intentionally excluded: it depends on the session's *history* (a
/// fresh session's first compile is never reused), not on the source, so it is
/// not comparable across differently-warmed sessions.  What must match is the
/// resolved structure (signature), the check outcome, and the diagnostics.
#[derive(PartialEq, Debug)]
struct ReportShape {
    signature: u64,
    build_ok: Option<bool>,
    diagnostics: Vec<String>,
}

fn shape(report: &crate::session::SessionReport) -> ReportShape {
    ReportShape {
        signature: report.signature,
        build_ok: report.build.as_ref().map(|b| b.ok),
        diagnostics: diag_set(report),
    }
}

#[test]
fn an_edited_session_compiles_identically_to_a_fresh_one() {
    // The incremental re-lex must never change what the compile *produces*.
    // After each edit, the session's report must equal a brand-new session over
    // the same source — identical signature, build outcome, and diagnostics.
    // (The established-build reuse still applies; what this guards is that the
    // incremental *lex* path is indistinguishable from a whole-buffer re-lex.)
    let cases: &[&[&str]] = &[
        // The source after each successive edit of the base program.
        &["a = 1\nz = 3\nf = x => a + x\nf 2\n"],
        &["a = 1\nf = x => a + x\nf 2\nzzz"],
        &["a = 99\nf = x => a + x\nf 2\n"],
        &["a = 1\nf = x => a + x\nf 2\nner = (2"],
        &["a = 1\nf = x => a + x\nf 2"],
        &["a = 1\nf = x => a\nf 2\n"],
    ];
    for edit in cases {
        let mut sess = BufferSession::new("a = 1\nf = x => a + x\nf 2\n");
        let _ = sess.compile();
        for (i, target) in edit.iter().enumerate() {
            // Apply the edit by replacing the whole source with the target.
            sess.replace(0..sess.len(), target);
            assert_eq!(
                shape(&sess.compile()),
                shape(&BufferSession::new(*target).compile()),
                "edit {i} to {target:?} diverged from a fresh compile"
            );
            assert_eq!(sess.source(), *target);
        }
    }
}

/// Assert the window splice of `old` → `new` is actually taken (`Some`) and
/// reproduces exactly a whole-buffer parse of the new source.
fn assert_splice_equals_full_parse(old: &str, new: &str) {
    let old_tokens = lex::lex(old).tokens;
    let old_program = parse::parse(&old_tokens).program;
    let new_tokens = lex::lex(new).tokens;
    let parsed = splice_program(
        &old_tokens,
        &old_program,
        &new_tokens,
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
    assert_eq!(format!("{:?}", program.statements), format!("{:?}", full.program.statements));
    assert_eq!(format!("{:?}", program.expr), format!("{:?}", full.program.expr));
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
    // Edits the incremental parser must handle *without* falling back to a
    // whole-buffer parse: a mid-statement insertion, a change inside a binding's
    // value, an edit in the trailing expression, an append of a new trailing
    // expression, a binding-name edit, and a mid-buffer statement insertion.
    // For each, the spliced frontend is identical to a fresh parse.
    let base = "a = 1\nf = x => a + x\nf 2\n";
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 22\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + 1\nf 2\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 3\n");
    assert_splice_equals_full_parse(base, "g = 1\nf = x => a + x\nf 2\n");
    assert_splice_equals_full_parse(base, "a = 1\nf = x => a + x\nf 2\nner");
    assert_splice_equals_full_parse(base, "a = 1\nz = 3\nf = x => a + x\nf 2\n");
}

#[test]
fn a_splice_that_ends_in_a_binding_falls_back() {
    // The splice does not try to replicate the whole-program parser's
    // "a program must end with an expression" error; a trailing binding (the new
    // value ends in `ner = (2`) makes it fall back to a full parse, which
    // reports that error precisely.
    let old = "a = 1\nf = x => a + x\nf 2\n";
    let new = "a = 1\nf = x => a + x\nner = (2";
    let old_tokens = lex::lex(old).tokens;
    let old_program = parse::parse(&old_tokens).program;
    let new_tokens = lex::lex(new).tokens;
    let (a, b, delta) = edit_span(old, new);
    assert!(
        splice_program(&old_tokens, &old_program, &new_tokens, a, b, delta).is_none(),
        "the trailing-binding edit falls back to a full parse"
    );
}

#[test]
fn the_incremental_signature_equals_the_full_signature() {
    // The incremental signature (`signature_reuse`, driven by the splice's
    // window) must produce the SAME combined signature, per-statement hashes,
    // and diagnostics as a whole-program `signature_full` walk — confirming the
    // reuse is sound and that the loop actually reuses, not just falls back.
    let base = "a = 1\nf = x => a + x\nf 2\n";
    let bt = lex::lex(base).tokens;
    let bp = parse::parse(&bt).program;
    let bsig = signature_full(&bp);
    for new in [
        "a = 1\nf = x => a + x\nf 22\n",
        "a = 1\nf = x => a + 1\nf 2\n",
        "g = 1\nf = x => a + x\nf 2\n",
        "a = 1\nf = x => a + x\nf 2\nner",
        "a = 1\nz = 3\nf = x => a + x\nf 2\n",
    ] {
        let nt = lex::lex(new).tokens;
        let (a, b, d) = edit_span(base, new);
        let out = splice_program(&bt, &bp, &nt, a, b, d).unwrap_or_else(|| {
            panic!("{new:?} should be spliceable");
        });
        // The window must actually be reused (a real incremental re-sign), not a
        // whole-program fallback: verify the reuse path matches the full walk.
        let reuse = signature_reuse(&out.program, &bsig, out.lo, out.hi, out.reuse);
        let full = signature_full(&out.program);
        assert_eq!(reuse.combined, full.combined, "combined for {new:?}");
        assert_eq!(reuse.stmt_hashes, full.stmt_hashes, "per-statement hashes for {new:?}");
        assert_eq!(
            reuse.diagnostics.len(),
            full.diagnostics.len(),
            "diagnostics for {new:?}"
        );
    }
}


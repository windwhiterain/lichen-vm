//! Tests for the incremental [`BufferSession`] and the error-block machinery
//! it relies on (`Expr::Err` byte ranges, [`ExprKind::ErrorBlock`] lowering, the
//! checker's skip path).

use lichen_highlevel::ir::ExprKind;

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

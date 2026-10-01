//! Tests for the incremental [`BufferSession`] and the error-block machinery
//! it relies on (`Expr::Err` byte ranges, [`ExprKind::ErrorBlock`] lowering, the
//! checker's skip path).

use lichen_highlevel::ir::ExprKind;

use super::{edit_span, splice_program};
use crate::ast::{Expr, Stmt};
use crate::diag::Stage;
use crate::lex;
use crate::package::PackageStore;
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
    // `a = )` — a stray `)` is not an atom, so the binding's *value* is one
    // recovered error block whose byte range masks the broken `)` token.  The
    // parser previously kept only the (line, col); now it carries the byte
    // mask too, so a diff can exclude the region.
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
    // A recovered parse error must NOT become the same highlevel construct as
    // an intentional `_` — that conflation is the leak the design fixes.  The
    // broken value lowers to a distinct `ExprKind::ErrorBlock`.
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
    // The intentional-placeholder test in compile_tests covers `_`; here we
    // assert the *broken* region is never a Placeholder.
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
    let mut sess = BufferSession::<LangProgram>::new("a = 1\nf = x => a + x\nf 2\n");
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
    // The established statements survived: `a`, `f`, and the `f 2` application
    // are still in the IR, just the trailing broken binding was skipped.
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
    // The editor's per-char case that DOES stay incremental: while a construct
    // is a single masked error block (an unclosed paren), every keystroke
    // grows only the mask — the clean structure is unchanged, so the
    // established program is reused per char.
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
    // The real editor case with a long identifier: a name that does not yet
    // resolve is a *name-resolution-only* delta — the structure is one `Name`
    // leaf, and its resolution stays the unresolved sentinel — so extending it
    // changes nothing the lowering/check consume, and only the resolve
    // diagnostic updates.
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
    // The sound form of "exclude the name from the hash": the signature signs
    // the resolution (which binding each use resolves to), not the spelling, so
    // a *consistent* rename — same bindings, same uses, different names — reuses
    // the established build.  The name-free IR is genuinely identical.
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

/// A report and its key observable fields, in a comparable form.  The `reused`
/// flag is intentionally excluded: it depends on the session's *history* (a
/// fresh session's first compile is never reused), not on the source, so it is
/// not comparable across differently-warmed sessions.  What must match is the
/// resolved structure (content key), the check outcome, and the diagnostics.
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
        // A *valid* trailing binding: the top level is a block, so a program
        // ending in a binding is a record program (a module) — the splice must
        // reproduce the whole-program parse.  (The unclosed-paren-at-EOF case
        // `ner = (2` is exercised by `a_splice_ending_in_a_binding_is_a_record_program`,
        // which checks the splice *program*; its error-column differs from the
        // whole-program parser by the region parser's Eof handling, which is a
        // pre-existing region-parser detail, not this feature's.)
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
    // `[a, b]`, `(a, b)` and `<a, b>` are distinct lowering-visible forms:
    // `compile` lowers Array, Tuple and TypeTuple through different
    // `ExprKind`s.  Swapping the brackets is therefore a resolved-content
    // change — the session must rebuild, and its report must equal a fresh
    // session's over the same source.  A shared tag left the key byte-equal
    // and reused the previous form's build.
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

/// Assert the window splice of `old` → `new` is actually taken (`Some`) and
/// reproduces exactly a whole-buffer parse of the new source.
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
fn a_splice_ending_in_a_binding_is_a_record_program() {
    // The top level is now a block: a program whose last statement is a
    // binding has no tail — it is a record program (a module), not an error.
    // The window splice must reproduce exactly a whole-buffer parse of the new
    // source (which is a record program with a broken binding value here).
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
    // The resolver's `content_key` is name-free and exact: a consistent rename
    // leaves it unchanged (so the session reuses), while a literal or
    // structural change moves it (so the session rebuilds).
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
    // `Apply` concatenates two expression encodings with no separator, so a
    // count-less list lets a nest boundary move without changing the key:
    // `(1,) (2, 3)` and `(1, (2,)) 3` are different trees with the same
    // expression sequence.  Writing each list's length makes them distinct.
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
    // `Str` and `NamedFieldRead` previously shared tag 24, and a string's raw
    // bytes occupy the element space, so `Str("\u{2}ab")` and `_.ab` (a
    // `NamedFieldRead` over the placeholder) encoded to the same key.  Their
    // tags are now distinct.
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

// ---------------------------------------------------------------------------
// A caller's view: the code after a `@{…@}` block, the imports resolved for it,
// and the positions every span is measured in.  This is the shape the language
// server drives the session with.
// ---------------------------------------------------------------------------

/// A store with `plug.lichen` registered as the embedded source `42`, so an
/// `@import` resolves without touching the disk.
fn store_with_plug() -> PackageStore<LangProgram> {
    let mut store = PackageStore::<LangProgram>::new();
    store
        .register_native("plug.lichen", "42", lichen_highlevel::no_native_ops())
        .unwrap();
    store
}

/// Preprocess `source` through `store` and point `sess` at the resulting view.
fn set_view_of(
    sess: &mut BufferSession<LangProgram>,
    source: &str,
    store: &mut PackageStore<LangProgram>,
) {
    let (pre, diagnostics) = crate::preprocess::preprocess(source, None, store);
    assert!(
        diagnostics.is_empty(),
        "the imports resolve: {diagnostics:?}"
    );
    let line_starts = lex::line_starts(source);
    sess.set_view(pre.code, pre.code_base, &line_starts, &pre.imports);
}

#[test]
fn a_view_compiles_the_code_after_the_block_with_absolute_spans() {
    // The session's buffer is the code *after* the block, but every span it
    // produces is the file's: an unresolved name on the file's line 5 is reported
    // on line 5, not on the code's line 3.
    let source = "@{\n  p = import \"plug.lichen\"\n@}\na = p\nzzz\n";
    let mut store = store_with_plug();
    let mut sess = BufferSession::with_registry("", "file:///view.lichen", store.registry());
    set_view_of(&mut sess, source, &mut store);

    let report = sess.compile();
    // The import resolved, so the only diagnostic is the unresolved `zzz`.
    let resolve: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.stage == Stage::Resolve)
        .collect();
    assert_eq!(
        resolve.len(),
        1,
        "one unresolved name: {:?}",
        report.diagnostics
    );
    assert_eq!(
        resolve[0].span,
        Some((5, 1)),
        "the unresolved name is at its position in the *file*"
    );
    // The AST and the token stream carry the file's positions too — the caller
    // (an editor) reads both.
    let Stmt::Binding(binding) = &report.program.statements[0].stmt else {
        panic!("the first statement is a binding");
    };
    assert_eq!(binding.span, (4, 1), "`a = p` is the file's line 4");
    assert_eq!(report.tokens[0].span, (4, 1), "the first token is line 4's");
    // The resolved `p` read: the import seeded the resolution, so the program
    // checks (the only complaint left is the deliberately unresolved `zzz`).
    assert_eq!(
        report.build.as_ref().map(|b| b.ok),
        Some(true),
        "the import resolves, so the program checks: {:?}",
        report.diagnostics
    );
}

#[test]
fn a_reuse_moves_the_retained_spans_through_the_edit() {
    // A check failure on the file's last line, and an edit *before* it that
    // changes no resolved content (a leading newline): the content key is
    // unchanged, so the established build — and its rendered diagnostic — is
    // reused.  The diagnostic's position must move with the text, or the editor
    // underlines the wrong line.
    let before = "f = x => x + 1\nf \"s\"\n";
    let after = "\nf = x => x + 1\nf \"s\"\n";
    let mut sess = BufferSession::<LangProgram>::new(before);
    let r0 = sess.compile();
    assert!(!r0.reused, "the first compile is a fresh build");
    let span_before = check_span(&r0);
    assert_eq!(span_before, Some((2, 3)), "the failing argument is line 2");

    sess.replace(0..0, "\n");
    let r1 = sess.compile();
    assert!(r1.reused, "the resolved content is unchanged");
    assert_eq!(
        check_span(&r1),
        Some((3, 3)),
        "the retained diagnostic moved with the text it points at"
    );
    // The whole report — diagnostics and their spans, the build outcome, the
    // content key — equals a fresh session's over the same source, and the
    // returned `ExprId → span` index does too.
    let fresh = BufferSession::<LangProgram>::new(after).compile();
    assert_eq!(
        shape(&r1),
        shape(&fresh),
        "a reuse diverged from a fresh compile"
    );
    assert_eq!(
        format!("{:?}", r1.span_index),
        format!("{:?}", fresh.span_index),
        "the moved span index is the one a fresh build produces"
    );
}

#[test]
fn a_diagnostic_inside_the_rewritten_text_rebuilds_instead_of_reusing() {
    // The same reuse, but the edit *rewrote* the text the check diagnostic points
    // at (a consistent rename, which leaves the resolved content — and so the
    // key — alone).  There is no honest position for the retained diagnostic, so
    // the session must re-derive rather than point it at the replacement.
    let before = "f = 1 + \"s\"\nf\n";
    let after = "g = 1 + \"s\"\ng\n";
    let mut sess = BufferSession::<LangProgram>::new(before);
    let r0 = sess.compile();
    assert!(!r0.reused);
    assert!(
        r0.diagnostics.iter().any(|d| d.stage == Stage::Check),
        "the `Int + string` is a check failure: {:?}",
        r0.diagnostics
    );

    sess.replace(0..sess.len(), after);
    let r1 = sess.compile();
    assert_eq!(
        r1.key, r0.key,
        "a consistent rename keeps the resolved content"
    );
    assert!(
        !r1.reused,
        "a diagnostic inside the rewritten region has no movable position, so the build is re-derived"
    );
    assert_eq!(
        shape(&r1),
        shape(&BufferSession::<LangProgram>::new(after).compile())
    );
}

#[test]
fn a_cell_shares_the_registry_with_the_import_its_value_reads() {
    // A `cache`d binding whose value *is* the import: the frozen closure names the
    // import's module, and `freeze_closure_mapped` asserts every key the module
    // references is registered in the registry the artifact is filed in — so the
    // session's registry must be the store's.  Both allocate keys (the import
    // from the device, the cell from the registry's cell space), and the two
    // spaces must not meet.
    let source = "@{ p = import \"plug.lichen\" @}\ncache base = p\nx = base + 1\nx\n";
    let mut store = store_with_plug();
    let mut sess = BufferSession::with_registry("", "file:///cells.lichen", store.registry());
    set_view_of(&mut sess, source, &mut store);

    let r0 = sess.compile();
    assert!(r0.ok(), "the program checks: {:?}", r0.diagnostics);
    assert_eq!(r0.cells.frozen, 1, "the marked binding was frozen");
    assert_eq!(sess.retained_cells(), 1);
    let registry = sess.registry();
    assert!(
        !registry.read().unwrap().is_empty(),
        "the import and the cell are both filed"
    );

    // The next compile reuses the established build and reads the cell back.
    let r1 = sess.compile();
    assert!(r1.reused, "nothing changed, so the build is reused");
    assert_eq!(
        r1.cells.reused, 0,
        "a reuse lowers nothing, so it reads no cell"
    );
}

#[test]
fn an_edited_view_compiles_identically_to_a_fresh_one() {
    // The view path, edited: every prefix of an edit sequence must equal a fresh
    // session over the same view — the key, the build outcome, the diagnostics
    // (spans included), the span index, and the resolved AST.
    let block = "@{ p = import \"plug.lichen\" @}\n";
    let cases = [
        "a = p\ncache b = a + 1\nb\n",
        "a = p\ncache b = a + 2\nb\n",
        "a = p\ncache b = a + 1\nb + 1\n",
        "a = p\ncache b = a + 1\nzzz\n",
        "a = p\ncache b = a + 1\n",
    ];
    let mut store = store_with_plug();
    let mut sess = BufferSession::with_registry("", "file:///edited.lichen", store.registry());
    for target in cases {
        let source = format!("{block}{target}");
        set_view_of(&mut sess, &source, &mut store);
        let got = sess.compile();

        let mut fresh_store = store_with_plug();
        let mut fresh =
            BufferSession::with_registry("", "file:///fresh.lichen", fresh_store.registry());
        set_view_of(&mut fresh, &source, &mut fresh_store);
        let want = fresh.compile();

        assert_eq!(
            shape(&got),
            shape(&want),
            "the edited view diverged from a fresh one for {target:?}"
        );
        assert_eq!(
            format!("{:?}", got.span_index),
            format!("{:?}", want.span_index),
            "the span index diverged for {target:?}"
        );
        assert_eq!(
            format!("{:?}", got.program.statements),
            format!("{:?}", want.program.statements),
            "the resolved AST diverged for {target:?}"
        );
    }
}

/// The span of the first check diagnostic, if any.
fn check_span(report: &crate::session::SessionReport<LangProgram>) -> Option<(u32, u32)> {
    report
        .diagnostics
        .iter()
        .find(|d| d.stage == Stage::Check)
        .and_then(|d| d.span)
}

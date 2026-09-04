//! The `Doc` attribute (`? doc{…}`): a label that attaches a `Doc`-shaped
//! metadata value to any expression.  A doc never constrains type-checking —
//! it is metadata, so an expression annotated with a doc checks and runs
//! exactly as it would without one, and two differing docs never conflict.
//!
//! These tests exercise the reachable surface of the mechanism: the doc
//! annotation parses and lowers, a doc'd value flows through an apply without a
//! diagnostic (the apply runs the doc's `unify_slots` through the checker's
//! relaxed-unify path, where `is_subtype` is `true`, so differing values are
//! suppressed), a later `? doc` overrides an earlier one (the `check_ann` label
//! branch: `b` replaces `a`), and a doc rides a struct definition.

use lichen_language::compile;
use lichen_language::run::evaluate;

/// A program with a doc annotation evaluates to the annotated value; the doc
/// rides the expression's attribute slot without changing the value/type.
#[test]
fn a_doc_annotation_evaluates_cleanly() {
    let out = evaluate("5 ? doc{ name = \"five\", description = \"an int\" }").unwrap();
    assert_eq!(out, "5 ? doc{ \"five\", \"an int\" }: Int");
}

/// A doc'd value passed as an argument to a plain function is accepted — the
/// apply runs the doc's `unify_slots`, which never reports a mismatch.
#[test]
fn a_doc_argument_to_a_plain_function_is_accepted() {
    let out = evaluate("f = x => x\nf (7 ? doc{ name = \"seven\", description = \"x\" })").unwrap();
    assert_eq!(out, "7: Int");
}

/// A doc annotation never produces a type error, however it's combined.
#[test]
fn a_doc_annotation_never_reports_a_diagnostic() {
    assert!(compile("(1 ? doc{ name = \"x\" }, 2 ? doc{ name = \"y\" })").ok());
}

/// A later `? doc` overrides an earlier one on the same expression (the
/// checker's label branch: `b` replaces `a`), and never errors.
#[test]
fn a_doc_annotation_overrides_a_prior_one() {
    assert!(
        compile("a = 5 ? doc{ name = \"a\" }\nb = a ? doc{ name = \"b\" }\nb").ok(),
        "re-annotating a doc'd value must not error"
    );
}

/// A doc rides a struct definition (the original motivation): the definition
/// carries metadata, and the struct type and an instance still check.
#[test]
fn a_doc_rides_a_struct_definition() {
    let src = "Point = struct<.x Int, .y Int> ? doc{ name = \"Point\", description = \"a point\" }\n(Point, Point(1, 2))";
    assert!(compile(src).ok(), "a doc on a struct definition must not error");
}

/// Two differing docs on the elements of one array do not conflict (array
/// homogeneity concerns the element *types*, and a label never constrains).
#[test]
fn two_differing_docs_in_one_array_do_not_conflict() {
    let out =
        evaluate("[1 ? doc{ name = \"x\" }, 2 ? doc{ name = \"y\" }][0]").unwrap();
    assert_eq!(out, "1: Int");
}

/// A perspective constraint and a doc label coexist on one expression
/// (`# p ? doc` → `[value, type, persp, doc]`): the perspective is a
/// constraint (enforced at apply), the doc is metadata (never a constraint).
#[test]
fn a_perspective_and_a_doc_coexist_on_one_expression() {
    assert!(
        compile("f = x # 4 => x\nf (5 # 4 ? doc{ name = \"five\" })").ok(),
        "a matching perspective with a doc must check"
    );
}

/// A perspective mismatch still fails even when a doc (a label) is attached —
/// a label never weakens a constraint.
#[test]
fn a_doc_does_not_weaken_a_perspective_mismatch() {
    assert!(
        !compile("f = x # 4 => x\nf (5 # 2 ? doc{ name = \"five\" })").ok(),
        "the perspective constraint must still reject a mismatched argument"
    );
}

/// The output renderer shows an expression's attributes **only when the
/// expression actually carries them** — an un-annotated expression spells
/// exactly as before, and a perspective/doc that is present is spelled.
#[test]
fn attributes_render_only_when_present() {
    assert_eq!(evaluate("5").unwrap(), "5: Int");
    assert_eq!(evaluate("5 # 4").unwrap(), "5 # 4: Int");
    assert_eq!(
        evaluate("5 ? doc{ name = \"five\" }").unwrap(),
        "5 ? doc{ \"five\" }: Int"
    );
    assert_eq!(
        evaluate("5 # 4 ? doc{ name = \"five\" }").unwrap(),
        "5 # 4 ? doc{ \"five\" }: Int"
    );
}

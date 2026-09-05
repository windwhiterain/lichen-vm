//! The `Doc` attribute (`? expr`): a **label** that attaches a metadata
//! value — a plain struct instance — to any expression.  There is no builtin
//! `doc` keyword or prelude; the user defines/imports a `Doc` struct and
//! constructs an instance by hand.  A doc never constrains type-checking —
//! it is metadata, so an expression annotated with a doc checks and runs
//! exactly as it would without one, and two differing docs never conflict.
//!
//! These tests exercise the reachable surface of the mechanism: the doc
//! annotation parses and lowers, a doc'd value flows through an apply without a
//! diagnostic (the apply runs the doc's `unify_slots` through the checker's
//! relaxed-unify path, where `is_subtype` is `true`, so differing values are
//! suppressed), a later `? b` overrides an earlier `? a` (the `check_ann` label
//! branch), and a doc rides a struct definition.  The renderer reads a doc's
//! field *names* from the value's type chain (never a hardcoded shape).

use lichen_language::compile;
use lichen_language::run::evaluate;

/// `Doc = struct<.name string, .description string>` — a user-made 2-field
/// struct, the conventional doc shape.
const DOC: &str = "Doc = struct<.name string, .description string>\n";

/// A program with a doc annotation evaluates to the annotated value; the doc
/// rides the expression's attribute slot without changing the value/type.
#[test]
fn a_doc_annotation_evaluates_cleanly() {
    let out = evaluate(&format!(
        "{DOC}5 ? Doc(.name \"five\", .description \"an int\")"
    ))
    .unwrap();
    assert_eq!(out, "5 ? name = \"five\", description = \"an int\": Int");
}

/// A doc'd value passed as an argument to a plain function is accepted — the
/// apply runs the doc's `unify_slots`, which never reports a mismatch.
#[test]
fn a_doc_argument_to_a_plain_function_is_accepted() {
    let out = evaluate(&format!(
        "{DOC}f = x => x\nf (7 ? Doc(.name \"seven\", .description \"x\"))"
    ))
    .unwrap();
    assert_eq!(out, "7: Int");
}

/// A doc annotation never produces a type error, however it's combined.
#[test]
fn a_doc_annotation_never_reports_a_diagnostic() {
    assert!(compile(&format!(
        "{DOC}(1 ? Doc(.name \"x\", .description \"a\"), 2 ? Doc(.name \"y\", .description \"b\"))"
    ))
    .ok());
}

/// A later `? b` overrides an earlier `? a` on the same expression (the
/// checker's label branch: `b` replaces `a`), and never errors.
#[test]
fn a_doc_annotation_overrides_a_prior_one() {
    assert!(
        compile(&format!(
            "{DOC}a = 5 ? Doc(.name \"a\", .description \"first\")\nb = a ? Doc(.name \"b\", .description \"second\")\nb"
        ))
        .ok(),
        "re-annotating a doc'd value must not error"
    );
}

/// A doc rides a struct definition (the original motivation): the definition
/// carries metadata, and the struct type and an instance still check.
#[test]
fn a_doc_rides_a_struct_definition() {
    let src = format!(
        "{DOC}Point = struct<.x Int, .y Int> ? Doc(.name \"Point\", .description \"a point\")\n(Point, Point(1, 2))"
    );
    assert!(
        compile(&src).ok(),
        "a doc on a struct definition must not error"
    );
}

/// Two differing docs on the elements of one array do not conflict (array
/// homogeneity concerns the element *types*, and a label never constrains).
#[test]
fn two_differing_docs_in_one_array_do_not_conflict() {
    let out = evaluate(&format!(
        "{DOC}[1 ? Doc(.name \"x\", .description \"a\"), 2 ? Doc(.name \"y\", .description \"b\")][0]"
    ))
    .unwrap();
    assert_eq!(out, "1: Int");
}

/// A perspective constraint and a doc label coexist on one expression
/// (`# p ? doc` → `[value, type, persp, doc]`): the perspective is a
/// constraint (enforced at apply), the doc is metadata (never a constraint).
#[test]
fn a_perspective_and_a_doc_coexist_on_one_expression() {
    assert!(
        compile(&format!(
            "{DOC}f = x # 4 => x\nf (5 # 4 ? Doc(.name \"five\", .description \"a\"))"
        ))
        .ok(),
        "a matching perspective with a doc must check"
    );
}

/// A perspective mismatch still fails even when a doc (a label) is attached —
/// a label never weakens a constraint.
#[test]
fn a_doc_does_not_weaken_a_perspective_mismatch() {
    assert!(
        !compile(&format!(
            "{DOC}f = x # 4 => x\nf (5 # 2 ? Doc(.name \"five\", .description \"a\"))"
        ))
        .ok(),
        "the perspective constraint must still reject a mismatched argument"
    );
}

/// A doc that drops a field is a struct-instantiation arity error (the
/// struct forces all its fields), not a doc-specific check.
#[test]
fn a_partial_doc_is_a_struct_arity_error() {
    assert!(
        !compile(&format!("{DOC}5 ? Doc(.name \"five\")")).ok(),
        "a Doc missing its .description field must be an arity error"
    );
}

/// The output renderer shows an expression's attributes **only when the
/// expression actually carries them** — an un-annotated expression spells
/// exactly as before, and a perspective/doc that is present is spelled.
/// A doc's field names come from its value's type chain.
#[test]
fn attributes_render_only_when_present() {
    assert_eq!(evaluate("5").unwrap(), "5: Int");
    assert_eq!(evaluate("5 # 4").unwrap(), "5 # 4: Int");
    assert_eq!(
        evaluate(&format!("{DOC}5 ? Doc(.name \"five\", .description \"a\")")).unwrap(),
        "5 ? name = \"five\", description = \"a\": Int"
    );
    assert_eq!(
        evaluate(&format!("{DOC}5 # 4 ? Doc(.name \"five\", .description \"a\")")).unwrap(),
        "5 # 4 ? name = \"five\", description = \"a\": Int"
    );
}

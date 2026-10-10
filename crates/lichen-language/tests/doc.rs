//! The `Doc` attribute (`? expr`): user-made metadata, never a constraint.
//! See attributes.md.

mod common;

use lichen_language::compile;
use lichen_language::run::evaluate;

/// `Doc = struct<.name string, .description string>` — a user-made 2-field
/// struct.
const DOC: &str = "Doc = struct<.name string, .description string>\n";

/// The doc rides the expression's attribute slot, not its value.
#[test]
fn a_doc_annotation_evaluates_cleanly() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}5 ? Doc(.name \"five\", .description \"an int\")"
    ));
    assert_eq!(common::usize_of(&value), 5);
}

/// The apply runs the doc's `unify_slots`, which never reports a mismatch.
#[test]
fn a_doc_argument_to_a_plain_function_is_accepted() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}f = x => x\nf (7 ? Doc(.name \"seven\", .description \"x\"))"
    ));
    assert_eq!(common::usize_of(&value), 7);
}

/// A doc annotation never produces a type error, however it's combined.
#[test]
fn a_doc_annotation_never_reports_a_diagnostic() {
    assert!(compile(&format!(
        "{DOC}(1 ? Doc(.name \"x\", .description \"a\"), 2 ? Doc(.name \"y\", .description \"b\"))"
    ))
    .ok());
}

/// The checker's label branch: `b` replaces `a`.
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

/// A doc rides a struct definition; the type and an instance still check.
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

/// Array homogeneity concerns the element *types*; a label never constrains.
#[test]
fn two_differing_docs_in_one_array_do_not_conflict() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}[1 ? Doc(.name \"x\", .description \"a\"), 2 ? Doc(.name \"y\", .description \"b\")][0]"
    ));
    assert_eq!(common::usize_of(&value), 1);
}

/// `# p ? doc` is `[value, type, persp, doc]`: the perspective constrains, the
/// doc does not.
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

/// `# 4` over a `# 8 ? doc` replaces the perspective and preserves the doc.
#[test]
fn reinterpret_the_perspective_replaces_it_and_preserves_the_doc() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}(5 # 8 ? Doc(.name \"five\", .description \"a\")) # 4"
    ));
    assert_eq!(common::usize_of(&value), 5);
}

/// Re-annotating the doc (`? b` over a `# 8 ? a` value) replaces the doc and
/// **preserves the perspective**.
#[test]
fn reinterpret_the_doc_preserves_the_perspective() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}(5 # 8 ? Doc(.name \"a\", .description \"first\")) ? Doc(.name \"b\", .description \"second\")"
    ));
    assert_eq!(common::usize_of(&value), 5);
}

/// A `#` added to a doc-only value keeps the doc and adds the perspective.
#[test]
fn a_perspective_added_to_a_doc_value_keeps_the_doc() {
    let (_, value, _) = common::evaluate(&format!(
        "{DOC}(5 ? Doc(.name \"five\", .description \"a\")) # 4"
    ));
    assert_eq!(common::usize_of(&value), 5);
}

/// A label never weakens a constraint; the requirement is checked against the
/// provider.
#[test]
fn a_broader_perspective_requirement_does_not_weaken_a_doc() {
    assert!(
        !compile(&format!(
            "{DOC}(5 # 4 ? Doc(.name \"five\", .description \"a\")) # 8"
        ))
        .ok(),
        "the perspective constraint must still reject a mismatched requirement"
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

/// A struct forces all its fields, so a dropped one is an arity error.
#[test]
fn a_partial_doc_is_a_struct_arity_error() {
    assert!(
        !compile(&format!("{DOC}5 ? Doc(.name \"five\")")).ok(),
        "a Doc missing its .description field must be an arity error"
    );
}

/// Attributes are spelled only when present; a doc's field names come from its
/// value's type chain.
#[test]
fn attributes_render_only_when_present() {
    assert_eq!(evaluate("5").unwrap(), "5: Int");
    assert_eq!(evaluate("5 # 4").unwrap(), "5 # 4: Int");
    assert_eq!(
        evaluate(&format!("{DOC}5 ? Doc(.name \"five\", .description \"a\")")).unwrap(),
        "5 ? name = \"five\", description = \"a\": Int"
    );
    assert_eq!(
        evaluate(&format!(
            "{DOC}5 # 4 ? Doc(.name \"five\", .description \"a\")"
        ))
        .unwrap(),
        "5 # 4 ? name = \"five\", description = \"a\": Int"
    );
}

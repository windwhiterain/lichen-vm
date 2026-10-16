//! Typed-perspective acceptance: annotations are the loosest operators.
//! See attributes.md.

use lichen_language::compile;
use lichen_lowlevel::{AnyNodeId, LowValue};
use lichen_utils::extend::AsEnum;

/// Whether the source compiles *and* passes every check with no diagnostics.
fn ok(source: &str) -> bool {
    compile(source).ok()
}

/// The first rendered diagnostic message (for the error cases).
fn message(source: &str) -> String {
    compile(source).diagnostics[0].message.clone()
}

/// The root's static perspective slot. It is a `[value, type]` pair whose
/// lattice value is element 0.
fn root_persp(source: &str) -> usize {
    let build = compile(source)
        .build
        .expect("the program must compile clean");
    let root = build.ir.root;
    let slot = build.state[root]
        .attr
        .expect("the root carries a perspective slot");
    let mut module = build.module;
    let value = module.evaluate_node_deep(slot, None).unwrap();
    // A slot is a `[value, type]` term pair; the lattice value is element 0.
    let value = match value.as_enum() {
        Some(LowValue::Array(items)) => {
            // SAFETY: `items` is the payload of the value just evaluated, whose block
            // is not dropped.
            let node = match unsafe { items.items() }.first().map(|item| item.node) {
                Some(AnyNodeId::Dynamic(n)) => n,
                _ => panic!("expected a dynamic perspective value"),
            };
            module
                .evaluate_node_deep(node, None)
                .expect("the perspective value is decided")
        }
        other => {
            // A bare slot (not yet a pair) already holds the lattice value.
            let _ = other;
            value
        }
    };
    let Some(LowValue::USize(n)) = value.as_enum() else {
        panic!("expected a USize perspective slot")
    };
    n
}

#[test]
fn a_leaf_annotation_binds_its_perspective() {
    // `1 # 4` — a leaf's slot is simply `p`; the pair is 3-wide.
    assert!(ok("1 # 4"));
    assert_eq!(root_persp("1 # 4"), 4);
}

#[test]
fn an_unannotated_binop_has_no_perspective_auto_propagation() {
    // `(1 # 4) + (2 # 6)` — the `+` is unannotated, so it carries no slot and
    // no gcd; the result is the ordinary `3 : Int`.
    assert!(ok("(1 # 4) + (2 # 6)"));
}

#[test]
fn a_compound_annotation_derives_gcd_and_checks() {
    // `((1 # 4) + (2 # 6)) # 2` — slot = gcd(4, 6) = 2; check 2 ≡ 2 ✓.
    assert!(ok("((1 # 4) + (2 # 6)) # 2"));
    assert_eq!(root_persp("((1 # 4) + (2 # 6)) # 2"), 2);
}

#[test]
fn a_missing_child_reads_zero() {
    // `((1 # 4) + 2) # 4` — the unannotated `2` contributes `0`; gcd(4, 0) = 4 ✓.
    assert!(ok("((1 # 4) + 2) # 4"));
    assert_eq!(root_persp("((1 # 4) + 2) # 4"), 4);
}

#[test]
fn a_compound_annotation_rejects_a_mismatched_perspective() {
    // `((1 # 4) + (2 # 6)) # 5` — slot = 2; check 2 ≡ 5 ✗.
    assert!(!ok("((1 # 4) + (2 # 6)) # 5"));
    assert_eq!(message("((1 # 4) + (2 # 6)) # 5"), "expected 5, found 2");
}

#[test]
fn an_identity_function_accepts_a_plain_argument() {
    // `id = x => x; id 5` — param reads 0, arg 0 → 0 ≡ 0 ✓.
    assert!(ok("id = x => x; id 5"));
}

#[test]
fn an_identity_function_rejects_a_perspective_argument() {
    // `id = x => x; id (5 # 4)` — the argument has perspective 4, the param
    // reads the missing `0` → 0 ≡ 4 ✗.
    assert!(!ok("id = x => x; id (5 # 4)"));
    assert_eq!(message("id = x => x; id (5 # 4)"), "expected 0, found 4");
}

#[test]
fn an_annotated_parameter_accepts_a_matching_perspective() {
    // `f = x # 4 => x; f (5 # 4)` — the param declares 4; 4 ≡ 4 ✓.
    assert!(ok("f = x # 4 => x; f (5 # 4)"));
}

#[test]
fn an_annotated_parameter_accepts_a_uniform_argument() {
    // No perspective is the lattice top, encoded `0`; it is usable where `# 4`
    // is declared.
    assert!(ok("f = x # 4 => x; f 5"));
}

#[test]
fn a_return_annotation_applies_to_the_result() {
    // The result is `5 # 4` — the body's annotation.
    assert!(ok("g = x => (x # 4); g 5"));
}

#[test]
fn mixed_type_and_perspective_annotations() {
    // `e : T # p` — both slots fill; the value keeps its type and perspective.
    assert!(ok("1 : Int # 4"));
    assert_eq!(root_persp("1 : Int # 4"), 4);
}

// Usable where `q` is required exactly when `q | n`; `0` matches only `0`.

#[test]
fn an_annotated_parameter_accepts_a_broader_perspective() {
    // The arg is uniform over 4, the param over 2, and 2 | 4.
    assert!(ok("f = x # 2 => x; f (5 # 4)"));
}

#[test]
fn an_annotated_parameter_rejects_an_incomparable_perspective() {
    // The arg is uniform over 2, the param over 4, and 4 ∤ 2.
    assert!(!ok("f = x # 4 => x; f (5 # 2)"));
    assert_eq!(message("f = x # 4 => x; f (5 # 2)"), "expected 4, found 2");
}

#[test]
fn a_compound_annotation_accepts_a_broader_derived_perspective() {
    // The derived provider is gcd(8, 4) = 4, and 2 | 4, so `# 2` checks.
    assert!(ok("((1 # 8) + (2 # 4)) # 2"));
    assert_eq!(root_persp("((1 # 8) + (2 # 4)) # 2"), 2);
}

#[test]
fn a_compound_annotation_rejects_a_narrower_declared_perspective() {
    // The derived slot is gcd(2, 2) = 2, and 4 ∤ 2.
    assert!(!ok("((1 # 2) + (2 # 2)) # 4"));
    assert_eq!(message("((1 # 2) + (2 # 2)) # 4"), "expected 4, found 2");
}

// The annotation is the requirement; the existing attribute is the provider.

#[test]
fn a_failed_read_in_an_attribute_renders_as_none() {
    // A failed read is a concrete empty value, spelled `none` and never `?a`.
    let source = "f = x # 4 => x\nf (5 # [1,2][3])";
    assert!(!ok(source));
    assert_eq!(message(source), "expected 4, found none");
}

#[test]
fn a_requirement_subtype_annotation_replaces_the_provider() {
    // The provider is 8, the requirement 4, and 4 | 8.
    assert!(ok("(5 # 8) # 4"));
    assert_eq!(root_persp("(5 # 8) # 4"), 4);
}

#[test]
fn a_narrower_provider_rejects_a_broader_requirement() {
    // The provider is 4, the requirement 8, and 8 ∤ 4.
    assert!(!ok("(5 # 4) # 8"));
    assert_eq!(message("(5 # 4) # 8"), "expected 8, found 4");
}

#[test]
fn an_annotation_over_a_bound_perspective_replaces_the_provider() {
    // The name reference provides 8, and 4 | 8.
    assert!(ok("x = 1 # 8\nx # 4"));
    assert_eq!(root_persp("x = 1 # 8\nx # 4"), 4);
}

#[test]
fn an_annotation_over_a_bound_perspective_rejects_a_broader_requirement() {
    // `x = 1 # 8` then `x # 16` — the provider is 8; `# 16` requires uniform
    // over 16, and 16 ∤ 8 ✗.
    assert!(!ok("x = 1 # 8\nx # 16"));
    assert_eq!(message("x = 1 # 8\nx # 16"), "expected 16, found 8");
}

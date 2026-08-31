//! The typed-perspective acceptance tests — the plan's §5.2 table.
//!
//! Each program is compiled and checked; `ok()` asserts the accept/reject
//! outcome, and the perspective *value* of a successful annotated expression
//! is read from the builder's attribute slot (the schema-driven slot the
//! checker lowers to the runtime pair's tail).  The error cases assert the
//! rendered "expected X, found Y" message.
//!
//! A note on precedence: annotations (`:` / `#`) are the *loosest* operators
//! (they wrap the whole expression to the right), so a compound whose
//! operands are individually perspective-annotated is written with explicit
//! parens — `((1 # 4) + (2 # 6)) # 2` — to express the plan's intent
//! (`(1#4) + (2#6)` grouped, then `# 2` over the `+`).  The plan's bare
//! `1 # 4 + 2 # 6` string is the same semantics under that reading.

use lichen_language::compile;
use lichen_lowlevel::LowValue;
use lichen_utils::extend::AsEnum;

/// Whether the source compiles *and* passes every check with no diagnostics.
fn ok(source: &str) -> bool {
    compile(source).ok()
}

/// The first rendered diagnostic message (for the error cases).
fn message(source: &str) -> String {
    compile(source).diagnostics[0].message.clone()
}

/// The root expression's static perspective slot, evaluated to its value.
/// Only meaningful for a program whose root is itself `# p`-annotated
/// (`build.attr[root]` is the checker's lowered slot).
fn root_persp(source: &str) -> usize {
    let build = compile(source).build.expect("the program must compile clean");
    let root = build.ir.root;
    let slot = build.attr[root].expect("the root carries a perspective slot");
    let mut module = build.module;
    let value = module.evaluate_node_deep(slot, None);
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
    // `f = x # 4 => x; f 5` — the arg has NO perspective.  In GPU code that
    // is "not expressed per-thread" = uniform over all threads = the lattice
    // top, encoded `0` (the `∞` fold, since Rust integers have no infinity).
    // A uniform-over-all value is uniform over 4 too, so it is usable where
    // `# 4` is declared (`4 | 0`) ✓.
    assert!(ok("f = x # 4 => x; f 5"));
}

#[test]
fn a_return_annotation_applies_to_the_result() {
    // `g = x => (x # 4); g 5` — the apply's perspective check runs 0 ≡ 0 ✓,
    // and the result is `5 # 4` (the body's annotation).
    assert!(ok("g = x => (x # 4); g 5"));
}

#[test]
fn mixed_type_and_perspective_annotations() {
    // `e : T # p` — both slots fill; the value keeps its type and perspective.
    assert!(ok("1 : Int # 4"));
    assert_eq!(root_persp("1 : Int # 4"), 4);
}

// --- the subtype (⊑) relaxation — stage 2 --------------------------------
//
// Perspective `n` means "uniform over `n` aligned threads."  A value is
// usable where `q` is required iff uniform-`n` implies uniform-`q`, which
// holds exactly when `q | n` (an aligned `n`-group partitions into
// `q`-groups).  So the check is `declared | value`; `0` ("no perspective")
// is a distinct kind that matches only `0`.

#[test]
fn an_annotated_parameter_accepts_a_broader_perspective() {
    // `f = x # 2 => x; f (5 # 4)` — the arg is uniform over 4 threads, the
    // param declares uniform over 2.  2 | 4, so uniform-4 implies uniform-2 ✓.
    assert!(ok("f = x # 2 => x; f (5 # 4)"));
}

#[test]
fn an_annotated_parameter_rejects_an_incomparable_perspective() {
    // `f = x # 4 => x; f (5 # 2)` — the arg is uniform over 2 threads, the
    // param declares uniform over 4.  4 ∤ 2, and uniform-2 does not imply
    // uniform-4 → the value does not fit the requirement ✗.
    assert!(!ok("f = x # 4 => x; f (5 # 2)"));
    assert_eq!(message("f = x # 4 => x; f (5 # 2)"), "expected 4, found 2");
}

#[test]
fn a_compound_annotation_accepts_a_broader_derived_perspective() {
    // `((1 # 8) + (2 # 4)) # 2` — the derived slot is gcd(8, 4) = 4 (uniform
    // over 4); `# 2` declares uniform over 2, and 2 | 4, so the assertion
    // holds ✓.  The slot stays the derived value, 4.
    assert!(ok("((1 # 8) + (2 # 4)) # 2"));
    assert_eq!(root_persp("((1 # 8) + (2 # 4)) # 2"), 4);
}

#[test]
fn a_compound_annotation_rejects_a_narrower_declared_perspective() {
    // `((1 # 2) + (2 # 2)) # 4` — the derived slot is gcd(2, 2) = 2 (uniform
    // over 2); `# 4` declares uniform over 4, and 4 ∤ 2, so the assertion
    // fails ✗.
    assert!(!ok("((1 # 2) + (2 # 2)) # 4"));
    assert_eq!(message("((1 # 2) + (2 # 2)) # 4"), "expected 4, found 2");
}

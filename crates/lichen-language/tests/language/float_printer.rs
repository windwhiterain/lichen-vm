//! The printer's spelling must re-lex as the same float and the same class.
//! See floating-point.md §3.5.

use lichen_language::compile;
use lichen_language::program::LangValue;
use lichen_lowlevel::LowValue;
use lichen_render::{print_type, print_value};
use lichen_utils::extend::AsEnum;

/// The spelling `lichen-render`'s public printer gives the float `source`
/// evaluates to.
fn print_float(source: &str) -> String {
    let report = compile(source);
    assert!(report.ok(), "{source} must check: {:?}", report.diagnostics);
    let mut build = report.build.expect("a build");
    let value = build
        .module
        .evaluate_node_deep(build.root_val, None)
        .unwrap();
    assert!(
        matches!(value.as_enum(), Some(LowValue::Float(_))),
        "{source} must evaluate to a float"
    );
    print_value(&build.module, value, build.root_ty)
}

/// Compile a printed spelling and return the float it evaluates to and the
/// spelling of its type.
fn value_of(text: &str) -> (f32, String) {
    let report = compile(text);
    assert!(
        report.ok(),
        "{text} must re-check: {:?}",
        report.diagnostics
    );
    let mut build = report.build.expect("a build");
    let value = build
        .module
        .evaluate_node_deep(build.root_val, None)
        .unwrap();
    let ty = print_type(&build.module, build.root_ty);
    let Some(LowValue::Float(value)) = value.as_enum() else {
        panic!("{text} must re-check as a float, not as a different LowValue")
    };
    (value, ty)
}

#[test]
fn a_printed_float_re_lexes_and_re_checks_as_the_same_float() {
    for (source, expected) in [
        // Rust's own `to_string` would spell these as a bare integer.
        ("1.0", 1.0f32),
        ("2.0", 2.0f32),
        // The shortest decimal that is not an integer.
        ("0.1", 0.1f32),
        // An overflowing literal is an infinity (§3.3); the printer must spell it so.
        ("9999999999999999999999999999999999999999.0", f32::INFINITY),
    ] {
        let printed = print_float(source);
        assert!(
            printed.contains('.'),
            "{source} printed as {printed}, which would read back as an Int"
        );
        let (value, ty) = value_of(&printed);
        assert_eq!(
            value.to_bits(),
            expected.to_bits(),
            "{source} printed as {printed} must re-check as the same float"
        );
        assert_eq!(ty, "Float", "{source} printed as {printed}");
    }

    // The deliberate refusal: `NaN` keeps Rust's spelling and does not read back.

    // The literal syntax has no `NaN`, so the value is handed to the printer.
    let report = compile("1.5");
    let build = report.build.expect("a build");
    let nan = print_value(
        &build.module,
        LangValue::from(LowValue::Float(f32::NAN)),
        build.float_type,
    );
    assert_eq!(nan, "NaN");
    assert!(
        !compile(&nan).ok(),
        "'NaN' is a name, not a literal, so the reader must refuse it"
    );
}

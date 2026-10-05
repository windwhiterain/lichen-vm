//! The analyzer's own smoke test: a program that checks is read back, and the
//! reads agree with what the source says.

use lichen_analyze::Analysis;

#[test]
fn a_checked_literal_is_readable() {
    let mut analysis = Analysis::compile("------\n1 + 1").expect("it checks");
    analysis.evaluate();
    assert!(
        analysis.diagnostics().is_empty(),
        "{:?}",
        analysis.diagnostics()
    );
    let (term, value, ty) = analysis.root();
    for node in [term, value, ty] {
        let description = analysis.describe(node);
        assert!(!description.is_empty(), "a node describes");
    }
}

#[test]
fn every_expression_describes_its_slots() {
    let mut analysis = Analysis::compile("------\nf = (y : Int) => y + 1\nf 5").expect("it checks");
    analysis.evaluate();
    let expressions: Vec<_> = analysis.expressions().collect();
    assert!(expressions.len() > 3, "the program has expressions");
    for expression in expressions {
        let described = analysis.describe_expression(expression);
        assert!(
            described.starts_with(&format!("EXPR {}", expression.0)),
            "{described}"
        );
    }
}

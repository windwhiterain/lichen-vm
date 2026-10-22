//! Pins a parenthesised application argument. Each records behaviour `dev` already has.

use super::*;
use lichen_language_lex::lex;

/// The parse errors of `source`, in the order the parser hands them back.
fn errors_of(source: &str) -> Vec<ParseDiag> {
    let tokens = lex(source).tokens;
    parse(&tokens).errors
}

/// The tail expression of a program that parses clean.
fn clean_expr(source: &str) -> Expr {
    let tokens = lex(source).tokens;
    let Parsed { program, errors } = parse(&tokens);
    assert!(errors.is_empty(), "unexpected parse errors: {errors:?}");
    program.expr.expect("a clean program has a tail")
}

#[test]
fn a_parenthesised_value_is_an_argument_in_every_argument_position() {
    // Each pair differs in nothing but the parentheses around `s (0) - 1`.
    let positions: &[(&str, &str, &str)] = &[
        ("field", "(R _)(.at s (0) - 1)", "(R _)(.at (s (0) - 1))"),
        (
            "field",
            "(W _)(.value s (0) - 1)",
            "(W _)(.value (s (0) - 1))",
        ),
        (
            "field",
            "(R _)(.from s (0) - 1)",
            "(R _)(.from (s (0) - 1))",
        ),
        ("index", "e[s (0) - 1]", "e[(s (0) - 1)]"),
        ("application", "f (s (0) - 1)", "f ((s (0) - 1))"),
        ("native call", "$n(s (0) - 1)", "$n((s (0) - 1))"),
        ("array element", "[s (0) - 1]", "[(s (0) - 1)]"),
        (
            "one-operand keyword",
            "exp (s (0) - 1)",
            "exp ((s (0) - 1))",
        ),
        ("slot read", "(f (s (0) - 1))", "(f ((s (0) - 1)))"),
    ];
    for (position, bare, paren) in positions {
        for spelling in [bare, paren] {
            let errors = errors_of(&format!("x = {spelling}\nx"));
            assert!(
                errors.is_empty(),
                "{position}: `{spelling}` did not parse: {errors:?}"
            );
        }
    }
}

#[test]
fn a_parenthesised_field_value_and_a_bare_one_build_the_same_tree() {
    // Each field value is read at the full-expression level, so the parentheses
    // group nothing.
    for source in ["(R _)(.at s (0) - 1)", "(R _)(.at (s (0) - 1))"] {
        let Expr::StructInst { fields, .. } = clean_expr(source) else {
            panic!("expected an instantiation in {source}")
        };
        let Expr::BinOp {
            operator: BinOp::Sub,
            left,
            right,
            ..
        } = &fields[0].value
        else {
            panic!("expected a subtraction in {source}")
        };
        assert!(matches!(**left, Expr::Apply { .. }), "{source}");
        assert!(matches!(**right, Expr::Int(1, _)), "{source}");
    }
}

#[test]
fn a_two_operand_keyword_reads_atoms_so_there_the_parentheses_bind() {
    // `prefix_op` gives `max`/`min` atoms, so here the parentheses do bind.
    let Expr::Prefix { operands, .. } = clean_expr("max ((s (0) - 1)) 0") else {
        panic!("expected a prefix operator")
    };
    assert!(matches!(operands[0], Expr::BinOp { .. }));
    let Expr::BinOp {
        operator: BinOp::Sub,
        left,
        ..
    } = clean_expr("max s (0) - 1 0")
    else {
        panic!("expected a subtraction outside the prefix")
    };
    assert!(matches!(*left, Expr::Prefix { .. }));
}

#[test]
fn an_unclosed_delimiter_is_reported_where_the_parse_stops() {
    // Not at the `(`, and not at the end: at the first token after it that no
    // parse can continue from.
    let errors = errors_of("x = (1 + 2\ny");
    assert_eq!(errors[0].span, Some((2, 2)));
}

#[test]
fn a_delimiter_one_short_is_reported_past_the_line_that_lost_it() {
    // `examples/decode.lichen`'s edit, one `)` short: the unclosed `(` eats line 3.
    let body = "\
softmax_raw = (k : ParY) => {
  @loop fold = s => fold (s (0) - 1, (g ((R _)(.at (s (0) - 1))))
  total = fold (k.n, 0.0)
}
softmax_raw";
    let short = errors_of(body);
    assert_eq!(short[0].span, Some((3, 9)));
    assert!(
        short[0].message.contains("found '='"),
        "unexpected message: {}",
        short[0].message
    );
    // The second sits three lines earlier: the pair is out of source order.
    assert_eq!(short[1].span, Some((1, 29)));

    let balanced = body.replacen("))))\n", ")))))\n", 1);
    assert!(
        errors_of(&balanced).is_empty(),
        "the same source with the delimiter restored must parse"
    );
}

//! What `Div` of a literal by a zero literal canonicalizes to, pinned directly.

use lichen_fusion::{BinaryOp, Expr, normalize};

/// `left / right`, as the symbolic language writes a division.
fn division(left: Expr, right: Expr) -> Expr {
    Expr::Binary(BinaryOp::Div, Box::new(left), Box::new(right))
}

/// The canonical form of `expression`, which every one of these is within the budget.
fn canonical(expression: Expr) -> Expr {
    normalize(&expression).expect("an expression this small is within the budget")
}

/// A literal over a zero literal is the **signed infinity** the quotient denotes.
///
/// # Invariant
/// This is the one restatement the canonicalizer's `Div` arm makes, and it exists because an
/// `Expr::Constant` is a fold's only seed: a `max` fold's identity is `−∞` and a `min` fold's is
/// `+∞`. `(0.0 - 1.0) / 0.0` is the spelling the language reaches, because it has no unary minus
/// and no infinity literal.
#[test]
fn a_literal_over_a_zero_literal_folds_to_the_infinity_it_denotes() {
    assert_eq!(
        canonical(division(Expr::Constant(1.0), Expr::Constant(0.0))),
        Expr::Constant(f64::INFINITY),
        "`1.0 / 0.0` is `+∞`, which is the unit of `min`"
    );
    assert_eq!(
        canonical(division(
            Expr::Binary(
                BinaryOp::Sub,
                Box::new(Expr::Constant(0.0)),
                Box::new(Expr::Constant(1.0))
            ),
            Expr::Constant(0.0)
        )),
        Expr::Constant(f64::NEG_INFINITY),
        "`(0.0 - 1.0) / 0.0` is `−∞`, which is the unit of `max` and is what the examples and \
         `identity_from_body.rs` seed with"
    );
}

/// Every other division is left exactly as it was.
///
/// # Invariant
/// The restatement is narrow on purpose. A zero or `NaN` numerator is a `NaN` and not an infinity,
/// and a divisor that is not a literal is not a zero at all, so both keep their tree — a fold
/// seeded with either is refused as `NotAnIdentity` rather than handed a value the arithmetic
/// does not produce.
#[test]
fn every_other_division_keeps_the_canonicalizer_it_had() {
    assert_eq!(
        canonical(division(Expr::Constant(1.0), Expr::Constant(2.0))),
        Expr::Constant(0.5),
        "a division by a non-zero literal is unchanged"
    );
    assert_eq!(
        canonical(division(
            Expr::Constant(1.0),
            Expr::Binary(
                BinaryOp::Sub,
                Box::new(Expr::Constant(0.0)),
                Box::new(Expr::Constant(2.0))
            )
        )),
        Expr::Constant(-0.5),
        "and so is one whose divisor is a literal that computes to a non-zero"
    );
    assert_eq!(
        canonical(division(Expr::Constant(0.0), Expr::Constant(0.0))),
        division(Expr::Constant(0.0), Expr::Constant(0.0)),
        "`0.0 / 0.0` is a `NaN` and not an infinity, so it keeps the tree the old rule kept"
    );
    assert_eq!(
        canonical(division(
            Expr::variable(lichen_fusion::Variable::DATUM),
            Expr::Constant(0.0)
        )),
        division(
            Expr::variable(lichen_fusion::Variable::DATUM),
            Expr::Constant(0.0)
        ),
        "and a divisor that is not a literal is not a zero divisor, so `d / 0.0` keeps its tree too"
    );
}

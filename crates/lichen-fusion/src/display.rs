//! Rendering: how a derived expression is read in a run report and in the example.
//!
//! # Invariant
//! Rendering is not canonicalization: the tree is what it is, and the only rewriting here
//! is the one that makes a product of exponentials legible, because
//! `x * exp(r) * exp(-r')` and `x * exp(r - r')` are the same product and only the
//! second is what the mathematics calls it.

use std::fmt;

use crate::expr::Expr;
use crate::operators::{BinaryOp, UnaryOp};
use crate::repair::{Assignment, FusedReduction, Origin};

impl fmt::Display for Expr {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(formatter, 0)
    }
}

impl Expr {
    fn precedence(&self) -> u8 {
        match self {
            Expr::Constant(_) | Expr::Variable(_) => u8::MAX,
            Expr::Unary(operator, _) => operator.precedence(),
            Expr::Binary(operator, ..) => operator.precedence(),
        }
    }

    fn write(&self, formatter: &mut fmt::Formatter<'_>, minimum: u8) -> fmt::Result {
        let parenthesized = self.precedence() < minimum;
        if parenthesized {
            formatter.write_str("(")?;
        }
        self.write_body(formatter)?;
        if parenthesized {
            formatter.write_str(")")?;
        }
        Ok(())
    }

    fn write_body(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Constant(value) => write!(formatter, "{value}"),
            Expr::Variable(variable) => write!(formatter, "{variable}"),
            Expr::Unary(operator, operand) => match operator {
                UnaryOp::Neg => {
                    formatter.write_str("-")?;
                    operand.write(formatter, UnaryOp::Neg.precedence())
                }
                UnaryOp::Square => {
                    formatter.write_str("(")?;
                    operand.write(formatter, 0)?;
                    formatter.write_str(")^2")
                }
                other => {
                    write!(formatter, "{}(", other.name())?;
                    operand.write(formatter, 0)?;
                    formatter.write_str(")")
                }
            },
            Expr::Binary(BinaryOp::Add, left, right) => {
                write_sum_term(left, formatter, true)?;
                write_sum_term(right, formatter, false)
            }
            Expr::Binary(BinaryOp::Mul, left, right) => self.write_product(left, right, formatter),
            Expr::Binary(operator @ (BinaryOp::Max | BinaryOp::Min), left, right) => {
                write!(formatter, "{}(", operator.name())?;
                left.write(formatter, 0)?;
                formatter.write_str(", ")?;
                right.write(formatter, 0)?;
                formatter.write_str(")")
            }
            Expr::Binary(operator, left, right) => {
                left.write(formatter, operator.precedence())?;
                write!(formatter, " {} ", operator.name())?;
                right.write(formatter, operator.precedence())
            }
        }
    }

    fn write_product(
        &self,
        left: &Expr,
        right: &Expr,
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        let factors = match (left, right) {
            (Expr::Binary(BinaryOp::Mul, _, _), _) => {
                operands(left).into_iter().chain([right]).collect()
            }
            (_, Expr::Binary(BinaryOp::Mul, _, _)) => {
                operands(right).into_iter().chain([left]).collect()
            }
            _ => vec![left, right],
        };
        let (plain, exponential): (Vec<&Expr>, Vec<&Expr>) = factors
            .into_iter()
            .partition(|factor| !matches!(factor, Expr::Unary(UnaryOp::Exp, _)));
        let mut written = 0;
        for factor in plain {
            if written > 0 {
                formatter.write_str(" * ")?;
            }
            factor.write(formatter, BinaryOp::Mul.precedence())?;
            written += 1;
        }
        let mut argument: Option<Expr> = None;
        for factor in &exponential {
            let Expr::Unary(_, operand) = factor else {
                continue;
            };
            argument = Some(match argument {
                None => operand.as_ref().clone(),
                Some(previous) => BinaryOp::Add.apply(previous, operand.as_ref().clone()),
            });
        }
        if let Some(argument) = argument {
            if written > 0 {
                formatter.write_str(" * ")?;
            }
            formatter.write_str("exp(")?;
            argument.write(formatter, BinaryOp::Add.precedence())?;
            formatter.write_str(")")?;
        }
        Ok(())
    }
}

fn operands(expr: &Expr) -> Vec<&Expr> {
    match expr {
        Expr::Binary(BinaryOp::Mul, left, right) => vec![left.as_ref(), right.as_ref()],
        other => vec![other],
    }
}

/// One term of a sum, as ` + x` or ` - x`.
///
/// # Invariant
/// The sign is read off the term rather than remembered, so a sum that is printed, taken
/// apart and printed again reads the same whichever of the two orderings it was written
/// in.
fn write_sum_term(term: &Expr, formatter: &mut fmt::Formatter<'_>, first: bool) -> fmt::Result {
    match term {
        Expr::Unary(UnaryOp::Neg, magnitude) => {
            formatter.write_str(if first { "-" } else { " - " })?;
            magnitude.write(formatter, BinaryOp::Mul.precedence())
        }
        other => {
            if !first {
                formatter.write_str(" + ")?;
            }
            other.write(formatter, BinaryOp::Add.precedence())
        }
    }
}

impl fmt::Display for Assignment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} = {}", self.target, self.value)
    }
}

impl fmt::Display for FusedReduction {
    /// The derivation and the body it emits, as a run report states them.
    ///
    /// # Invariant
    /// The emitted assignments are last, in the order the rolling update runs them, so
    /// what is printed is the loop body and not a description of one, and a fusion whose
    /// repair function was handed in says so rather than printing an abstraction that
    /// never happened.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.origin {
            Origin::Derived {
                abstraction,
                inverse,
            } => {
                writeln!(formatter, "c = {}", abstraction.source)?;
                writeln!(formatter, "g'(r)(c) = {}", abstraction.template)?;
                writeln!(formatter, "g'(r)^-1(x) = {inverse}")?;
            }
            Origin::HandSpecified => {
                writeln!(
                    formatter,
                    "h was hand-specified: no abstraction, no inversion"
                )?;
            }
        }
        writeln!(formatter, "h(x, r, r') = {}", self.repair.body)?;
        for step in &self.steps {
            writeln!(formatter, "{step}")?;
        }
        Ok(())
    }
}

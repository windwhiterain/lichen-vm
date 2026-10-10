//! The inversion step: solving an element function for its abstracted parameter.
//!
//! # Invariant
//! Every operator either names its inverse or the rewrite stops there: a repair function
//! built on an assumed inverse is a wrong answer carrying a proof, so an operator with no
//! entry refuses by name instead of approximating one.

use std::fmt;

use crate::expr::{Expr, Variable};
use crate::operators::BinaryOp;

/// Why an inversion could not be taken, named so a refusal can be acted on.
#[derive(Debug, Clone, PartialEq)]
pub enum InvertRefusal {
    /// The parameter occurs more than once, so no single subexpression solves for it.
    MultipleOccurrences {
        /// How many times the parameter occurs.
        count: usize,
    },
    /// The parameter is absent from the expression that was to be inverted.
    MissingParameter {
        /// The parameter that was looked for.
        parameter: Expr,
    },
    /// An operator with no inverse entry stood between the parameter and the root.
    UnsupportedOperator {
        /// The operator that stopped the rewrite.
        operator: String,
    },
    /// An operator whose inverse exists but is not determined by the value alone.
    AmbiguousOperator {
        /// The operator that stopped the rewrite.
        operator: String,
    },
}

impl fmt::Display for InvertRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InvertRefusal::MultipleOccurrences { count } => {
                write!(
                    formatter,
                    "the parameter occurs {count} times, so nothing solves for it"
                )
            }
            InvertRefusal::MissingParameter { parameter } => {
                write!(formatter, "the parameter {parameter} does not occur")
            }
            InvertRefusal::UnsupportedOperator { operator } => {
                write!(formatter, "the operator `{operator}` has no inverse entry")
            }
            InvertRefusal::AmbiguousOperator { operator } => write!(
                formatter,
                "the operator `{operator}` is not one to one, so its inverse is not determined"
            ),
        }
    }
}

/// Solves `expr` for `parameter`, writing the solution in terms of `argument`.
///
/// # Invariant
/// The result is the substitution that makes `expr` evaluate to `argument`: the caller
/// composes it into the repair function by hand, so an equation this returns is the whole
/// obligation and not a step towards one.
pub fn invert(expr: &Expr, parameter: &Expr, argument: Variable) -> Result<Expr, InvertRefusal> {
    let occurrences = expr.occurrences(parameter);
    match occurrences {
        0 => Err(InvertRefusal::MissingParameter {
            parameter: parameter.clone(),
        }),
        1 => walk(expr, parameter, &Expr::Variable(argument)),
        count => Err(InvertRefusal::MultipleOccurrences { count }),
    }
}

/// The substitution that makes `node` evaluate to `rhs`.
fn walk(node: &Expr, parameter: &Expr, rhs: &Expr) -> Result<Expr, InvertRefusal> {
    if node == parameter {
        return Ok(rhs.clone());
    }
    match node {
        Expr::Constant(_) | Expr::Variable(_) => Err(InvertRefusal::MissingParameter {
            parameter: parameter.clone(),
        }),
        Expr::Unary(operator, operand) => {
            let inverse = operator
                .inverse()
                .ok_or_else(|| InvertRefusal::AmbiguousOperator {
                    operator: operator.name().to_string(),
                })?;
            walk(operand, parameter, &inverse.apply(rhs.clone()))
        }
        Expr::Binary(
            operator @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div),
            left,
            right,
        ) => {
            let (left, right) = (left.as_ref(), right.as_ref());
            let left_holds = left.occurrences(parameter) > 0;
            let solution = match operator {
                BinaryOp::Add => BinaryOp::Sub.apply(
                    rhs.clone(),
                    if left_holds {
                        left.clone()
                    } else {
                        right.clone()
                    },
                ),
                BinaryOp::Sub => match left_holds {
                    true => BinaryOp::Add.apply(rhs.clone(), right.clone()),
                    false => BinaryOp::Sub.apply(left.clone(), rhs.clone()),
                },
                BinaryOp::Mul => BinaryOp::Div.apply(
                    rhs.clone(),
                    if left_holds {
                        right.clone()
                    } else {
                        left.clone()
                    },
                ),
                _ => match left_holds {
                    true => BinaryOp::Mul.apply(rhs.clone(), right.clone()),
                    false => BinaryOp::Div.apply(left.clone(), rhs.clone()),
                },
            };
            walk(if left_holds { left } else { right }, parameter, &solution)
        }
        Expr::Binary(operator, _, _) => Err(InvertRefusal::UnsupportedOperator {
            operator: operator.name().to_string(),
        }),
    }
}

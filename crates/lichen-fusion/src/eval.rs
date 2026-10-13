//! Evaluating a derived expression: the cross-check, and the example's own end to end.
//!
//! # Invariant
//! Evaluation is `f64` throughout and propagates what the hardware does at a domain edge
//! rather than trapping: an emitted body runs on a device the same way, so a kernel whose
//! logarithm met a non-positive value is a wrong answer here too, and the cross-check must
//! be able to see it.

use std::collections::{BTreeMap, BTreeSet};

use crate::expr::{Expr, Variable};
use crate::operators::{BinaryOp, UnaryOp};
use crate::random::Random;

/// The values the free variables of an expression are read at.
pub type Bindings = BTreeMap<Variable, f64>;

/// The value of an expression, or `None` when it reads a variable nothing bound.
///
/// # Invariant
/// Linear in the size of the expression, and it binds nothing itself: the rolling state a
/// fused body carries is the caller's, which is the same state the emitted assignments
/// write.
pub fn evaluate(expr: &Expr, bindings: &Bindings) -> Option<f64> {
    match expr {
        Expr::Constant(value) => Some(*value),
        Expr::Variable(variable) => bindings.get(variable).copied(),
        Expr::Unary(operator, operand) => {
            let operand = evaluate(operand, bindings)?;
            Some(match operator {
                UnaryOp::Neg => -operand,
                UnaryOp::Exp => operand.exp(),
                UnaryOp::Ln => operand.ln(),
                UnaryOp::Sqrt => operand.sqrt(),
                UnaryOp::Sin => operand.sin(),
                UnaryOp::Cos => operand.cos(),
                UnaryOp::Square => operand * operand,
                UnaryOp::Relu => operand.max(0.0),
            })
        }
        Expr::Binary(operator, left, right) => {
            let left = evaluate(left, bindings)?;
            let right = evaluate(right, bindings)?;
            Some(match operator {
                BinaryOp::Add => left + right,
                BinaryOp::Sub => left - right,
                BinaryOp::Mul => left * right,
                BinaryOp::Div => left / right,
                BinaryOp::Max => left.max(right),
                BinaryOp::Min => left.min(right),
                BinaryOp::LessThan => bool_value(left < right),
                BinaryOp::GreaterThan => bool_value(left > right),
                BinaryOp::LessEqual => bool_value(left <= right),
                BinaryOp::GreaterEqual => bool_value(left >= right),
                BinaryOp::Equal => bool_value(left == right),
                BinaryOp::NotEqual => bool_value(left != right),
            })
        }
    }
}

fn bool_value(holds: bool) -> f64 {
    if holds { 1.0 } else { 0.0 }
}

/// The largest relative difference between two expressions over random points.
///
/// # Invariant
/// Every free variable of *either* expression is sampled in `[1, 4)`, where every operator
/// here is defined on both sides: a point where a logarithm is undefined is outside the
/// domain the check is about, not a disagreement between two forms. A variable only one
/// side reads is sampled too, so the two are compared at one point. A non-finite value is
/// a disagreement without bound.
pub fn max_relative_disagreement(
    left: &Expr,
    right: &Expr,
    random: &mut Random,
    samples: usize,
) -> Option<f64> {
    let variables: BTreeSet<Variable> = left
        .variables()
        .union(&right.variables())
        .copied()
        .collect();
    let mut worst: f64 = 0.0;
    let mut bindings = Bindings::new();
    for _ in 0..samples {
        for variable in &variables {
            bindings.insert(*variable, random.next_bounded(1.0, 3.0));
        }
        let (Some(left_value), Some(right_value)) =
            (evaluate(left, &bindings), evaluate(right, &bindings))
        else {
            return None;
        };
        if !left_value.is_finite() || !right_value.is_finite() {
            return Some(f64::INFINITY);
        }
        let scale = left_value.abs().max(right_value.abs()).max(1.0);
        worst = worst.max((left_value - right_value).abs() / scale);
    }
    Some(worst)
}

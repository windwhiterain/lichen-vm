//! The abstraction step: writing an element function as `g'(r_N)(c)`.
//!
//! # Invariant
//! The parameter is the outermost subexpression of the element function that does not
//! mention the producer's value, read before canonicalization and as the logic wrote it:
//! flattening first would hide `d + bias` inside a sorted sum and abstract one term of it.

use crate::expr::{Expr, Variable};
use crate::invert::InvertRefusal;
use crate::repair::FusionRefusal;

/// An element function read as a function of the producer's value, `g'(r_N)(c)`.
#[derive(Debug, Clone)]
pub struct Abstraction {
    /// The subexpression the parameter was taken from, the element's own contribution.
    pub source: Expr,
    /// The element function with `source` replaced by [`Self::PARAMETER`].
    pub template: Expr,
}

impl Abstraction {
    /// The slot the abstracted element parameter occupies, written `c`.
    pub const PARAMETER: Variable = Variable::PARAMETER;
}

/// Reads `element_function` as a function of `producer` and names the parameter it is one of.
///
/// # Invariant
/// The outermost candidate wins unconditionally, and no operator inside it is consulted:
/// the per-element repair identity is `g'(r')(g'(r)⁻¹(g'(r)(c))) = g'(r')(c)`, a composition
/// that holds for any `c`, and the consumer's rolling state is covered by the
/// distributivity check rather than by anything `c` contains. A refusal therefore comes
/// only from the chain above `c`.
pub fn abstract_element_function(
    element_function: &Expr,
    producer: &Variable,
) -> Result<Abstraction, FusionRefusal> {
    if element_function.occurrences(&Expr::Variable(*producer)) == 0 {
        return Err(FusionRefusal::ProducerValueAbsent {
            element_function: element_function.clone(),
        });
    }
    let source = outermost_producer_free(element_function, producer).ok_or(
        FusionRefusal::ProducerValueAbsent {
            element_function: element_function.clone(),
        },
    )?;
    let repeats = element_function.occurrences(&source);
    if repeats != 1 {
        return Err(FusionRefusal::NotInvertible {
            at: source,
            cause: InvertRefusal::MultipleOccurrences { count: repeats },
        });
    }
    let template = replace_once(
        element_function,
        &source,
        &Expr::Variable(Abstraction::PARAMETER),
    );
    Ok(Abstraction { source, template })
}

/// The outermost producer-free subexpression, which is the parameter.
///
/// # Invariant
/// The outermost is the choice that keeps the element's own logic inside one subexpression:
/// a narrower candidate would re-expose the custom logic the abstraction removed, and the
/// rolling update recomputes `g(d, r')` from the datum either way.
fn outermost_producer_free(element_function: &Expr, producer: &Variable) -> Option<Expr> {
    let mut candidates = Vec::new();
    collect_producer_free(element_function, producer, &mut candidates);
    candidates.into_iter().next()
}

/// Every producer-free subexpression, outermost first.
fn collect_producer_free(expr: &Expr, producer: &Variable, found: &mut Vec<Expr>) {
    if expr.occurrences(&Expr::Variable(*producer)) == 0 {
        found.push(expr.clone());
        return;
    }
    match expr {
        Expr::Constant(_) | Expr::Variable(_) => {}
        Expr::Unary(_, operand) => collect_producer_free(operand, producer, found),
        Expr::Binary(_, left, right) => {
            collect_producer_free(left, producer, found);
            collect_producer_free(right, producer, found);
        }
    }
}

/// `expr` with its one occurrence of `target` replaced by `replacement`.
fn replace_once(expr: &Expr, target: &Expr, replacement: &Expr) -> Expr {
    if expr == target {
        return replacement.clone();
    }
    match expr {
        Expr::Constant(_) | Expr::Variable(_) => expr.clone(),
        Expr::Unary(operator, operand) => Expr::Unary(
            *operator,
            Box::new(replace_once(operand, target, replacement)),
        ),
        Expr::Binary(operator, left, right) => Expr::Binary(
            *operator,
            Box::new(replace_once(left, target, replacement)),
            Box::new(replace_once(right, target, replacement)),
        ),
    }
}

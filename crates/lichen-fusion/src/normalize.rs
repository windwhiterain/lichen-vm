//! The canonicalizer: the comparison every obligation in this crate is written in.
//!
//! # Invariant
//! Two expressions that mean the same algebraic function normalize to the same tree, and
//! [`normalize`] is the only way the crate compares them: distributivity, cancellation
//! and operand order are all decided here and nowhere else.

use crate::expr::Expr;
use crate::operators::{BinaryOp, UnaryOp};

/// How many nodes a canonical form may allocate before [`normalize`] gives up.
///
/// # Invariant
/// Distribution over addition multiplies the size of a product, so an element function
/// written as a product of sums canonicalizes to a polynomial whose degree is the number
/// of factors. The bound is what keeps that from being an unbounded cost: the canonical
/// form of a kernel fragment is small, and one that is not is refused rather than grown.
pub const NORMALIZATION_BUDGET: usize = 4096;

/// The canonical form of an expression, or `None` when it exceeded
/// [`NORMALIZATION_BUDGET`].
pub fn normalize(expr: &Expr) -> Option<Expr> {
    let mut remaining = NORMALIZATION_BUDGET;
    build(expr, &mut remaining)
}

fn spend(remaining: &mut usize) -> Option<()> {
    *remaining = (*remaining).checked_sub(1)?;
    Some(())
}

fn build(expr: &Expr, remaining: &mut usize) -> Option<Expr> {
    match expr {
        Expr::Constant(value) => Some(Expr::Constant(*value)),
        Expr::Variable(variable) => Some(Expr::Variable(*variable)),
        Expr::Unary(operator, operand) => build_unary(*operator, operand, remaining),
        Expr::Binary(operator, left, right) => build_binary(*operator, left, right, remaining),
    }
}

fn build_unary(operator: UnaryOp, operand: &Expr, remaining: &mut usize) -> Option<Expr> {
    match (operator, operand) {
        (UnaryOp::Exp, Expr::Unary(UnaryOp::Ln, inner))
        | (UnaryOp::Ln, Expr::Unary(UnaryOp::Exp, inner))
        | (UnaryOp::Neg, Expr::Unary(UnaryOp::Neg, inner)) => return build(inner, remaining),
        _ => {}
    }
    let operand = build(operand, remaining)?;
    if let Some(value) = fold_constant(operator, &operand) {
        spend(remaining)?;
        return Some(Expr::Constant(value));
    }
    if operator == UnaryOp::Exp && matches!(operand, Expr::Binary(BinaryOp::Add, ..)) {
        return exponent_product(&operand, remaining);
    }
    if operator == UnaryOp::Neg
        && matches!(operand, Expr::Binary(BinaryOp::Add | BinaryOp::Mul, ..))
    {
        return negate(&operand, remaining);
    }
    spend(remaining)?;
    Some(Expr::Unary(operator, Box::new(operand)))
}

fn build_binary(
    operator: BinaryOp,
    left: &Expr,
    right: &Expr,
    remaining: &mut usize,
) -> Option<Expr> {
    match operator {
        BinaryOp::Sub => {
            let left = build(left, remaining)?;
            let right = negate(&build(right, remaining)?, remaining)?;
            build_sum(&[left, right], remaining)
        }
        BinaryOp::Add => build_sum(
            &[build(left, remaining)?, build(right, remaining)?],
            remaining,
        ),
        BinaryOp::Mul => build_product(
            &[build(left, remaining)?, build(right, remaining)?],
            remaining,
        ),
        BinaryOp::Max | BinaryOp::Min => {
            let operands = flatten(
                &[build(left, remaining)?, build(right, remaining)?],
                operator,
            );
            let mut folded = match operator {
                BinaryOp::Max => fold_constants(&operands, |left, right| Some(left.max(right)))?,
                _ => fold_constants(&operands, |left, right| Some(left.min(right)))?,
            };
            folded.sort();
            finish(folded, operator, 0.0, remaining)
        }
        BinaryOp::Div => {
            let left = build(left, remaining)?;
            let right = build(right, remaining)?;
            match (left.as_constant(), right.as_constant()) {
                (Some(value), Some(divisor)) if divisor != 0.0 => {
                    spend(remaining)?;
                    Some(Expr::Constant(value / divisor))
                }
                _ => {
                    spend(remaining)?;
                    Some(Expr::Binary(operator, Box::new(left), Box::new(right)))
                }
            }
        }
        _ => build_comparison(operator, left, right, remaining),
    }
}

fn build_comparison(
    operator: BinaryOp,
    left: &Expr,
    right: &Expr,
    remaining: &mut usize,
) -> Option<Expr> {
    let left = build(left, remaining)?;
    let right = build(right, remaining)?;
    let (operator, left, right) = canonical_comparison(operator, left, right);
    let literals = match (left.as_constant(), right.as_constant()) {
        (Some(left), Some(right)) => Some(compare(operator, left, right)),
        _ => None,
    };
    if let Some(value) = literals.or_else(|| (left == right).then(|| compare(operator, 0.0, 0.0))) {
        spend(remaining)?;
        return Some(Expr::Constant(value));
    }
    spend(remaining)?;
    Some(Expr::Binary(operator, Box::new(left), Box::new(right)))
}

/// The operand order one comparison is written in, with the operator to match it.
fn canonical_comparison(operator: BinaryOp, left: Expr, right: Expr) -> (BinaryOp, Expr, Expr) {
    match left.cmp(&right) {
        std::cmp::Ordering::Greater => (operator.mirrored(), right, left),
        _ => (operator, left, right),
    }
}

/// The `1` or `0` a comparison of two literals yields.
fn compare(operator: BinaryOp, left: f64, right: f64) -> f64 {
    let holds = match operator {
        BinaryOp::LessThan => left < right,
        BinaryOp::GreaterThan => left > right,
        BinaryOp::LessEqual => left <= right,
        BinaryOp::GreaterEqual => left >= right,
        BinaryOp::Equal => left == right,
        _ => left != right,
    };
    if holds { 1.0 } else { 0.0 }
}

/// The literal value of an operator over a literal, when the domain allows one.
fn fold_constant(operator: UnaryOp, operand: &Expr) -> Option<f64> {
    let Expr::Constant(value) = operand else {
        return None;
    };
    let folded = match operator {
        UnaryOp::Neg => -value,
        UnaryOp::Exp => value.exp(),
        UnaryOp::Ln => {
            if *value > 0.0 {
                value.ln()
            } else {
                return None;
            }
        }
        UnaryOp::Sqrt => {
            if *value >= 0.0 {
                value.sqrt()
            } else {
                return None;
            }
        }
        UnaryOp::Square => value * value,
        UnaryOp::Relu => value.max(0.0),
    };
    folded.is_finite().then_some(folded)
}

/// A relation is an operator with no inverse but an order, so a key adds into its
/// operands instead of failing to cancel against them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Relation {
    Max,
    Min,
}

impl Relation {
    fn of(operator: BinaryOp) -> Option<Relation> {
        match operator {
            BinaryOp::Max => Some(Relation::Max),
            BinaryOp::Min => Some(Relation::Min),
            _ => None,
        }
    }

    fn operator(self) -> BinaryOp {
        match self {
            Relation::Max => BinaryOp::Max,
            Relation::Min => BinaryOp::Min,
        }
    }

    /// The relation as `-x` maps it: `-max(a,b) = min(-a,-b)`.
    ///
    /// # Invariant
    /// The order reverses, which is the whole difference between the two: a negation that
    /// kept `max` would answer `-(a ∨ b)` with `(-a) ∨ (-b)`, and the signs would disagree.
    fn dual(self) -> Relation {
        match self {
            Relation::Max => Relation::Min,
            Relation::Min => Relation::Max,
        }
    }
}

/// The sum of the given terms, as one canonical node.
fn build_sum(terms: &[Expr], remaining: &mut usize) -> Option<Expr> {
    let mut signed = Vec::with_capacity(terms.len());
    for term in terms {
        match term {
            Expr::Unary(UnaryOp::Neg, magnitude)
                if matches!(
                    &**magnitude,
                    Expr::Binary(BinaryOp::Add | BinaryOp::Mul, ..)
                ) =>
            {
                signed.push(negate(magnitude, remaining)?);
            }
            other => signed.push(other.clone()),
        }
    }
    let flat = flatten(&signed, BinaryOp::Add);
    build_sum_terms(&flat, remaining, &recursive_sum)
}

/// The sum of already-flattened terms, taking the relation kind the flattened list fixes.
fn recursive_sum(terms: &[Expr], remaining: &mut usize) -> Option<Expr> {
    let mut relations: Vec<(Relation, Vec<Expr>)> = Vec::new();
    let mut keyed = Vec::new();
    for term in terms {
        match relation_parts(term) {
            Some((relation, operands)) => relations.push((relation, operands)),
            None => keyed.push(term.clone()),
        }
    }
    let Some(relation) = relations.first().map(|(relation, _)| *relation) else {
        return build_sum_terms(terms, remaining, &additive_sum);
    };
    let key = build_sum_terms(&keyed, remaining, &recursive_sum)?;
    let mut parts = Vec::new();
    for (_inner, operands) in relations {
        for operand in operands {
            parts.push(distribute_key(&operand, &key, remaining)?);
        }
    }
    build_relation(parts, relation, remaining)
}

/// A sum of terms that hold no relation: folded, sorted, its opposites cancelled.
fn additive_sum(terms: &[Expr], remaining: &mut usize) -> Option<Expr> {
    let mut kept = terms.to_vec();
    kept.sort();
    cancel_opposites(&mut kept);
    finish(kept, BinaryOp::Add, 0.0, remaining)
}

/// The sum of already-flattened canonical terms, built by `builder`.
fn build_sum_terms(
    terms: &[Expr],
    remaining: &mut usize,
    builder: &dyn Fn(&[Expr], &mut usize) -> Option<Expr>,
) -> Option<Expr> {
    let folded = fold_constants(terms, |left, right| Some(left + right))?;
    let mut kept: Vec<Expr> = folded
        .into_iter()
        .filter(|term| term.as_constant() != Some(0.0))
        .collect();
    kept.sort();
    cancel_opposites(&mut kept);
    if kept.iter().any(|term| relation_parts(term).is_some()) {
        return builder(&kept, remaining);
    }
    additive_sum(&kept, remaining)
}

/// The one builder that takes an additive key into the relations a sum holds.
///
/// # Invariant
/// It is the distributivity the fusion engine's obligation is written in: a repair function
/// that adds a value distributes over a maximum exactly when the addition is pushed into
/// every operand, so this is the form the two sides are compared on. Every step either
/// consumes a relation or adds itself to a named subterm, so the recursion terminates.
fn distribute_key(operand: &Expr, key: &Expr, remaining: &mut usize) -> Option<Expr> {
    match relation_parts(operand) {
        Some((inner, _)) => {
            let summed = build_sum(&[operand.clone(), key.clone()], remaining)?;
            apply_key_over_relation(inner, &summed, &Expr::Constant(0.0), remaining)
        }
        None => build_sum(&[operand.clone(), key.clone()], remaining),
    }
}

/// `key` added into a relation term, pushing it into each operand and then into whatever
/// relation an operand became.
fn apply_key_over_relation(
    relation: Relation,
    term: &Expr,
    key: &Expr,
    remaining: &mut usize,
) -> Option<Expr> {
    let Some((nested, operands)) = relation_parts(term) else {
        return build_sum(&[term.clone(), key.clone()], remaining);
    };
    let mut parts = Vec::with_capacity(operands.len());
    for operand in operands {
        let summed = build_sum(&[operand, key.clone()], remaining)?;
        parts.push(apply_key_over_relation(
            nested,
            &summed,
            &Expr::Constant(0.0),
            remaining,
        )?);
    }
    let _ = relation;
    build_relation(parts, nested, remaining)
}

/// A list of summed relation operands as one canonical relation node.
fn build_relation(
    mut operands: Vec<Expr>,
    relation: Relation,
    remaining: &mut usize,
) -> Option<Expr> {
    operands.retain(|operand| operand.as_constant() != Some(0.0));
    finish(operands, relation.operator(), 0.0, remaining)
}

/// `-relation`, as the dual relation over the negated operands.
fn negate_relation(relation: Relation, operands: Vec<Expr>, remaining: &mut usize) -> Option<Expr> {
    let mut negated = Vec::with_capacity(operands.len());
    for operand in operands {
        negated.push(negate(&operand, remaining)?);
    }
    negated.sort();
    finish(negated, relation.dual().operator(), 0.0, remaining)
}

/// One term seen as a relation, or `None` when it is not one.
fn relation_parts(term: &Expr) -> Option<(Relation, Vec<Expr>)> {
    let Expr::Binary(operator, left, right) = term else {
        return None;
    };
    let relation = Relation::of(*operator)?;
    let mut operands = vec![(**left).clone(), (**right).clone()];
    let mut index = 0;
    while index < operands.len() {
        let nested = match &operands[index] {
            Expr::Binary(operator, left, right) if Relation::of(*operator) == Some(relation) => {
                Some(((**left).clone(), (**right).clone()))
            }
            _ => None,
        };
        match nested {
            Some((left, right)) => {
                operands[index] = left;
                operands.insert(index + 1, right);
            }
            None => index += 1,
        }
    }
    Some((relation, operands))
}

/// The product of the given factors, as one canonical node.
fn build_product(factors: &[Expr], remaining: &mut usize) -> Option<Expr> {
    let folded = fold_constants(&flatten(factors, BinaryOp::Mul), |left, right| {
        Some(left * right)
    })?;
    let mut kept: Vec<Expr> = Vec::with_capacity(folded.len());
    for factor in folded {
        match factor.as_constant() {
            Some(0.0) => {
                spend(remaining)?;
                return Some(Expr::Constant(0.0));
            }
            Some(1.0) => {}
            _ => kept.push(factor),
        }
    }
    if let Some(index) = kept
        .iter()
        .position(|factor| matches!(factor, Expr::Binary(BinaryOp::Add, ..)))
    {
        return distribute_product(&kept, index, remaining);
    }
    kept.sort();
    finish(kept, BinaryOp::Mul, 1.0, remaining)
}

/// The sum of the products obtained by replacing one sum factor with each of its terms.
///
/// # Invariant
/// Exactly one factor is distributed per call and the recursive call sees one sum factor
/// fewer, so distribution terminates however the product was written.
fn distribute_product(factors: &[Expr], index: usize, remaining: &mut usize) -> Option<Expr> {
    let (left, right) = match &factors[index] {
        Expr::Binary(BinaryOp::Add, left, right) => ((**left).clone(), (**right).clone()),
        _ => return None,
    };
    let mut products = Vec::with_capacity(2);
    for term in [left, right] {
        let mut expanded: Vec<Expr> = factors
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != index)
            .map(|(_, factor)| factor.clone())
            .collect();
        expanded.push(term);
        products.push(build_product(&expanded, remaining)?);
    }
    build_sum(&products, remaining)
}

/// The exponential of a canonical sum as the product of the exponentials of its terms.
fn exponent_product(operand: &Expr, remaining: &mut usize) -> Option<Expr> {
    let Expr::Binary(BinaryOp::Add, left, right) = operand else {
        return None;
    };
    let factors = [
        build_unary(UnaryOp::Exp, left, remaining)?,
        build_unary(UnaryOp::Exp, right, remaining)?,
    ];
    build_product(&factors, remaining)
}

fn negate(expr: &Expr, remaining: &mut usize) -> Option<Expr> {
    match expr {
        Expr::Constant(value) => {
            spend(remaining)?;
            Some(Expr::Constant(-value))
        }
        Expr::Unary(UnaryOp::Neg, inner) => build(inner, remaining),
        Expr::Binary(operator @ (BinaryOp::Add | BinaryOp::Mul), left, right) => {
            let left = Expr::Unary(UnaryOp::Neg, left.clone());
            let right = Expr::Unary(UnaryOp::Neg, right.clone());
            build_binary(*operator, &left, &right, remaining)
        }
        Expr::Binary(operator @ (BinaryOp::Max | BinaryOp::Min), left, right) => {
            let relation = Relation::of(*operator)?;
            let (_, operands) =
                relation_parts(&Expr::Binary(*operator, left.clone(), right.clone()))?;
            negate_relation(relation, operands, remaining)
        }
        other => {
            spend(remaining)?;
            Some(Expr::Unary(
                UnaryOp::Neg,
                Box::new(build(other, remaining)?),
            ))
        }
    }
}

/// One flat operand list for an associative operator, over already canonical operands.
fn flatten(operands: &[Expr], operator: BinaryOp) -> Vec<Expr> {
    let mut flat = Vec::with_capacity(operands.len());
    for operand in operands {
        match operand {
            Expr::Binary(inner, left, right) if *inner == operator => {
                flat.push((**left).clone());
                flat.push((**right).clone());
            }
            other => flat.push(other.clone()),
        }
    }
    flat
}

/// The literals of a list folded into one, and the rest returned beside it.
///
/// # Invariant
/// The accumulator is left out when the list held no literal at all, so `x * y` does not
/// gain a `0` factor on the way to being recognised as not zero.
fn fold_constants(terms: &[Expr], combine: fn(f64, f64) -> Option<f64>) -> Option<Vec<Expr>> {
    let mut accumulated: Option<f64> = None;
    let mut rest = Vec::new();
    for term in terms {
        match term {
            Expr::Constant(value) => {
                accumulated = Some(match accumulated {
                    None => *value,
                    Some(previous) => combine(previous, *value)?,
                });
            }
            other => rest.push(other.clone()),
        }
    }
    if let Some(value) = accumulated {
        rest.push(Expr::Constant(value));
    }
    Some(rest)
}

/// Drops each term whose negation is also a term, as `x - x` is zero.
fn cancel_opposites(terms: &mut Vec<Expr>) {
    let mut index = 0;
    while index < terms.len() {
        let magnitude = match &terms[index] {
            Expr::Unary(UnaryOp::Neg, magnitude) => Some((**magnitude).clone()),
            _ => None,
        };
        let opposite = magnitude.and_then(|magnitude| {
            terms
                .iter()
                .skip(index + 1)
                .position(|term| *term == magnitude)
        });
        match opposite {
            Some(position) => {
                terms.remove(position);
                terms.remove(index);
            }
            None => index += 1,
        }
    }
}

/// One canonical node from a sorted operand list.
///
/// # Invariant
/// Every operand is in the node: a sum or a product is n-ary, and the list is folded into a
/// right-leaning chain of that many binary nodes because a binary node cannot hold them all.
fn finish(
    mut operands: Vec<Expr>,
    operator: BinaryOp,
    empty: f64,
    remaining: &mut usize,
) -> Option<Expr> {
    let mut node = match operands.pop() {
        None => return Some(Expr::Constant(empty)),
        Some(last) => last,
    };
    while let Some(left) = operands.pop() {
        spend(remaining)?;
        node = Expr::Binary(operator, Box::new(left), Box::new(node));
    }
    Some(node)
}

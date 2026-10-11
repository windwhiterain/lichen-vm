//! The derivation: the fused rolling update, or the refusal that says why there is none.
//!
//! # Invariant
//! One obligation per step and no step without one: the producer's step, the abstraction,
//! the inversion, the distributivity check, the emitted body. A region this crate cannot
//! prove is returned as a refusal rather than emitted, because the logic it came from
//! still runs on its own.

use std::fmt;

use crate::abstraction::{Abstraction, abstract_element_function};
use crate::expr::{Expr, Variable, substitute_all};
use crate::invert::{InvertRefusal, invert};
use crate::normalize::{NORMALIZATION_BUDGET, normalize};
use crate::operators::BinaryOp;

/// One side of a reduction chain: its combine and the identity that seeds it.
///
/// # Invariant
/// The identity is declared rather than derived, because the rolling update starts at it
/// and a combine with no known unit has no rolling update: `max` over no elements is not
/// a number the engine may invent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reduction {
    /// The associative combine, applied as `combine(accumulated, contribution)`.
    pub combine: BinaryOp,
    /// The unit of `combine`, the value the rolling state starts at.
    pub identity: f64,
}

/// The repair function `h`, whichever way it came.
///
/// # Invariant
/// [`Self::body`] is a function of exactly three slots — the value to repair, the
/// producer's value before the step and its value after — and of nothing else, so it can
/// be emitted into a loop body and evaluated there. [`Self::new`] is the only way to build
/// one, which is what keeps that true of a hand-written repair function as well.
#[derive(Debug, Clone)]
pub struct RepairFunction {
    /// The repair function itself, `h(x, r, r')`.
    pub body: Expr,
}

impl RepairFunction {
    /// The repair function as written, checked to read only the three slots it is written
    /// over.
    ///
    /// # Invariant
    /// Nothing else may be read: a body mentioning the datum or the consumer's own state
    /// would be a repair that recomputes rather than repairs, and no symbolic check
    /// downstream would see it — the distributivity equation is an equation in `h` alone.
    pub fn new(body: Expr) -> Result<RepairFunction, FusionRefusal> {
        let outside: Vec<Variable> = body
            .variables()
            .into_iter()
            .filter(|variable| {
                !matches!(
                    *variable,
                    Variable::REPAIR_ARGUMENT | Variable::PRODUCER | Variable::PRODUCER_NEXT
                )
            })
            .collect();
        if outside.is_empty() {
            return Ok(RepairFunction {
                body: canonicalize(body)?,
            });
        }
        Err(FusionRefusal::RepairFunctionOutsideItsSlots { reads: outside })
    }

    /// This repair function applied to one value between two producer values.
    pub fn apply(&self, value: &Expr, previous_producer: &Expr, next_producer: &Expr) -> Expr {
        substitute_all(
            &self.body,
            &[
                (Variable::REPAIR_ARGUMENT, value.clone()),
                (Variable::PRODUCER, previous_producer.clone()),
                (Variable::PRODUCER_NEXT, next_producer.clone()),
            ],
        )
    }
}

/// One assignment of the emitted loop body.
#[derive(Debug, Clone)]
pub struct Assignment {
    /// The state slot this step writes.
    pub target: Variable,
    /// What it writes.
    pub value: Expr,
}

/// Where the repair function came from.
#[derive(Debug, Clone)]
pub enum Origin {
    /// Inverted out of the element function: the abstraction it was taken over, and the
    /// inverse composed with it.
    Derived {
        /// How the element function was read as a function of the producer's value.
        abstraction: Abstraction,
        /// The element function's inverse, `g'(r)^-1(x)`.
        inverse: Expr,
    },
    /// Handed in whole, as an optimization pack may hand one.
    HandSpecified,
}

/// A fusion: the repair function and the rolling update that carries it.
///
/// # Invariant
/// The two entry points produce the same shape. Only [`Origin`] and the obligation behind
/// the repair function differ: a derived one rests on the inversion table, a hand-specified
/// one on whatever its author checked, and both rest on distributivity.
#[derive(Debug, Clone)]
pub struct FusedReduction {
    /// The producer reduction, with the identity the body starts from.
    pub producer: Reduction,
    /// The consumer reduction, with the identity the body starts from.
    pub consumer: Reduction,
    /// Whether the repair function was derived or handed in.
    pub origin: Origin,
    /// The repair function the rolling update carries.
    pub repair: RepairFunction,
    /// The loop body: the producer's step, then the consumer's.
    pub steps: Vec<Assignment>,
}

/// Why a region was not fused, named so the run report can act on it.
#[derive(Debug, Clone, PartialEq)]
pub enum FusionRefusal {
    /// The element function does not read the producer's final value, so there is nothing
    /// for a repair function to repair.
    ProducerValueAbsent {
        /// The element function as it was given.
        element_function: Expr,
    },
    /// The element function could not be solved for its parameter.
    NotInvertible {
        /// The subexpression the refusal is about.
        at: Expr,
        /// Which of the inversion's causes it was.
        cause: InvertRefusal,
    },
    /// The repair function does not distribute over the consumer's combine.
    NotDistributive {
        /// The repair function that failed the check.
        repair: Expr,
        /// The combine it failed to distribute over.
        combine: BinaryOp,
    },
    /// The repair function reads a slot it is not written over.
    RepairFunctionOutsideItsSlots {
        /// The slots it read besides the three it may read.
        reads: Vec<Variable>,
    },
    /// A canonical form grew past the normalization budget.
    NormalizationBudgetExceeded {
        /// The budget that was exceeded, in nodes.
        budget: usize,
    },
}

impl FusionRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for FusionRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FusionRefusal::ProducerValueAbsent { element_function } => write!(
                formatter,
                "the element function {element_function} does not read the producer's value, \
                 so there is nothing for a repair function to repair"
            ),
            FusionRefusal::NotInvertible { at, cause } => {
                write!(
                    formatter,
                    "the element function is not invertible: {cause}, at {at}"
                )
            }
            FusionRefusal::NotDistributive { repair, combine } => write!(
                formatter,
                "the repair function {repair} does not distribute over the consumer's combine `{}`",
                combine.name()
            ),
            FusionRefusal::RepairFunctionOutsideItsSlots { reads } => write!(
                formatter,
                "the repair function reads {}, and a repair is written over the value, the \
                 producer's value before the step and its value after, and nothing else",
                reads
                    .iter()
                    .map(|variable| format!("`{variable}`"))
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
            FusionRefusal::NormalizationBudgetExceeded { budget } => {
                write!(
                    formatter,
                    "canonicalization exceeded its budget of {budget} nodes"
                )
            }
        }
    }
}

/// Derives the fused rolling update for one producer reduction and one consumer reduction.
///
/// # Invariant
/// The abstraction reads the element function as written and the emitted body is
/// canonicalized, so the run report and the distributivity check compare one tree. The
/// template is substituted **after** the inverse is taken, which is what solves the
/// parameter at this step's own producer value. `docs/notes/graph-fusion.md` §6.
pub fn derive(
    producer: Reduction,
    element_function: Expr,
    consumer: Reduction,
) -> Result<FusedReduction, FusionRefusal> {
    let abstraction = abstract_element_function(&element_function, &Variable::PRODUCER)?;
    let parameter = Expr::Variable(Abstraction::PARAMETER);
    let inverse =
        invert(&abstraction.template, &parameter, Variable::REPAIR_ARGUMENT).map_err(|cause| {
            FusionRefusal::NotInvertible {
                at: abstraction.template.clone(),
                cause,
            }
        })?;
    let body = substitute_all(
        &abstraction.template,
        &[
            (Abstraction::PARAMETER, inverse.clone()),
            (Variable::PRODUCER, Expr::Variable(Variable::PRODUCER_NEXT)),
        ],
    );
    let repair = RepairFunction::new(body)?;
    let steps = emit(&producer, &consumer, &repair, &element_function)?;
    Ok(FusedReduction {
        producer,
        consumer,
        origin: Origin::Derived {
            abstraction,
            inverse,
        },
        repair,
        steps,
    })
}

/// Emits the rolling update around a repair function handed in whole.
///
/// # Invariant
/// Abstraction and inversion do not run, so what the author wrote is what the kernel
/// carries; what still runs is distributivity, which the rolling update rests on whatever
/// `h` is, and the three slots `h` may read. The per-element identity is the author's, and
/// a differential harness is where that claim is kept (`docs/notes/graph-fusion.md` §5).
pub fn derive_with_repair(
    producer: Reduction,
    element_function: Expr,
    consumer: Reduction,
    repair_body: Expr,
) -> Result<FusedReduction, FusionRefusal> {
    let repair = RepairFunction::new(repair_body)?;
    let steps = emit(&producer, &consumer, &repair, &element_function)?;
    Ok(FusedReduction {
        producer,
        consumer,
        origin: Origin::HandSpecified,
        repair,
        steps,
    })
}

/// The loop body both entry points emit, canonicalized.
///
/// # Invariant
/// One place, so the two entry points cannot emit different bodies for the same repair
/// function, and so the distributivity check cannot be forgotten by one of them: what
/// differs between them is where `h` came from, never what carries it.
fn emit(
    producer: &Reduction,
    consumer: &Reduction,
    repair: &RepairFunction,
    element_function: &Expr,
) -> Result<Vec<Assignment>, FusionRefusal> {
    distributivity(repair, consumer.combine)?;
    let producer_value = Expr::Variable(Variable::PRODUCER);
    let next_producer = Expr::Variable(Variable::PRODUCER_NEXT);
    let consumer_state = Expr::Variable(Variable::CONSUMER_STATE);
    let element_at_next = substitute_all(
        element_function,
        &[(Variable::PRODUCER, next_producer.clone())],
    );
    Ok(vec![
        Assignment {
            target: Variable::PRODUCER_NEXT,
            value: canonicalize(
                producer
                    .combine
                    .apply(producer_value.clone(), Expr::Variable(Variable::DATUM)),
            )?,
        },
        Assignment {
            target: Variable::CONSUMER_NEXT,
            value: canonicalize(consumer.combine.apply(
                repair.apply(&consumer_state, &producer_value, &next_producer),
                element_at_next,
            ))?,
        },
    ])
}

/// Checks that the repair function distributes over the consumer's combine.
///
/// # Invariant
/// A normalization and a comparison, not a search: the two sides are small trees, so the
/// check costs one canonical form each and any disagreement is a refusal rather than a
/// second attempt with a looser obligation.
pub fn distributivity(repair: &RepairFunction, combine: BinaryOp) -> Result<(), FusionRefusal> {
    let (repaired_combined, repaired_apart) = distributivity_sides(repair, combine);
    if canonicalize(repaired_combined)? == canonicalize(repaired_apart)? {
        Ok(())
    } else {
        Err(FusionRefusal::NotDistributive {
            repair: repair.body.clone(),
            combine,
        })
    }
}

/// The two sides of the distributivity equation, `h(f(x, y))` and `f(h(x), h(y))`.
///
/// # Invariant
/// Built once and read twice, by the symbolic check and by the numeric cross-check: an
/// example that rebuilt the equation itself would be checking its own transcription rather
/// than the one the engine proved. The two arguments are distinct slots, so neither side
/// can capture the other when they are substituted into `h`.
pub fn distributivity_sides(repair: &RepairFunction, combine: BinaryOp) -> (Expr, Expr) {
    let left = Expr::Variable(Variable::REPAIR_ARGUMENT);
    let right = Expr::Variable(SECOND_REPAIR_ARGUMENT);
    let producer_value = Expr::Variable(Variable::PRODUCER);
    let next_producer = Expr::Variable(Variable::PRODUCER_NEXT);
    let repaired_combined = repair.apply(
        &combine.apply(left.clone(), right.clone()),
        &producer_value,
        &next_producer,
    );
    let repaired_apart = combine.apply(
        repair.apply(&left, &producer_value, &next_producer),
        repair.apply(&right, &producer_value, &next_producer),
    );
    (repaired_combined, repaired_apart)
}

/// The distributivity check's second argument, apart from the first so neither captures.
const SECOND_REPAIR_ARGUMENT: Variable = Variable::new("y");

fn canonicalize(expr: Expr) -> Result<Expr, FusionRefusal> {
    normalize(&expr).ok_or(FusionRefusal::NormalizationBudgetExceeded {
        budget: NORMALIZATION_BUDGET,
    })
}

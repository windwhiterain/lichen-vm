//! The operators of the symbolic scalar language, and how expressions are written.

use std::ops::{Add, Div, Mul, Neg, Sub};

use crate::expr::{Expr, Variable};

/// A binary operator of the symbolic scalar language.
///
/// # Invariant
/// `Add`, `Sub`, `Mul`, `Div`, `Max` and `Min` are the operators a reduction's `combine`
/// and an element function are written in; the comparisons are here because masking and
/// clipping are ordinary attention logic, and a language that cannot name them would
/// refuse a masked kernel. `Div` on a zero divisor folds nowhere: the canonicalizer
/// keeps the tree rather than invent an infinity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BinaryOp {
    /// Addition, the combine of a sum reduction.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division, undefined on a zero divisor.
    Div,
    /// The larger of two values, the combine of a maximum reduction.
    Max,
    /// The smaller of two values, the combine of a minimum reduction.
    Min,
    /// `left < right`, yielding `1` or `0`.
    LessThan,
    /// `left > right`, yielding `1` or `0`.
    GreaterThan,
    /// `left <= right`, yielding `1` or `0`.
    LessEqual,
    /// `left >= right`, yielding `1` or `0`.
    GreaterEqual,
    /// `left == right`, yielding `1` or `0`.
    Equal,
    /// `left != right`, yielding `1` or `0`.
    NotEqual,
}

impl BinaryOp {
    /// The operator as it is written and read in a diagnostic.
    ///
    /// # Invariant
    /// One spelling per operator, used by refusals and by rendering alike: a run report
    /// that named an operator two ways could not be searched for.
    pub const fn name(self) -> &'static str {
        match self {
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::Max => "max",
            BinaryOp::Min => "min",
            BinaryOp::LessThan => "<",
            BinaryOp::GreaterThan => ">",
            BinaryOp::LessEqual => "<=",
            BinaryOp::GreaterEqual => ">=",
            BinaryOp::Equal => "==",
            BinaryOp::NotEqual => "!=",
        }
    }

    /// The operator applied to two operands, uncanonicalized.
    pub fn apply(self, left: Expr, right: Expr) -> Expr {
        Expr::Binary(self, Box::new(left), Box::new(right))
    }

    /// Whether a fold written with this operator may be re-associated.
    ///
    /// # Invariant
    /// This is the **only** licence a tiled fold rests on
    /// (`docs/notes/shared-memory-and-barriers.md` §5): re-association answers the fold's own value
    /// for `+`, `*`, `max` and `min` and for nothing else in the language.
    pub const fn associative(self) -> bool {
        matches!(
            self,
            BinaryOp::Add | BinaryOp::Mul | BinaryOp::Max | BinaryOp::Min
        )
    }

    /// The order relation that reads the other way round.
    ///
    /// # Invariant
    /// Defined for the order relations alone, because only those are re-spelled by
    /// swapping their operands; anything else would have to change meaning, which is
    /// what the canonicalizer's operand sort would then have done.
    pub const fn mirrored(self) -> BinaryOp {
        match self {
            BinaryOp::LessThan => BinaryOp::GreaterThan,
            BinaryOp::GreaterThan => BinaryOp::LessThan,
            BinaryOp::LessEqual => BinaryOp::GreaterEqual,
            BinaryOp::GreaterEqual => BinaryOp::LessEqual,
            other => other,
        }
    }

    pub(crate) const fn precedence(self) -> u8 {
        match self {
            BinaryOp::LessThan
            | BinaryOp::GreaterThan
            | BinaryOp::LessEqual
            | BinaryOp::GreaterEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual => 1,
            BinaryOp::Add | BinaryOp::Sub => 2,
            BinaryOp::Mul | BinaryOp::Div => 3,
            BinaryOp::Max | BinaryOp::Min => 4,
        }
    }
}

/// A unary operator of the symbolic scalar language.
///
/// # Invariant
/// `Exp` and `Ln` are inverses of one another over the reals and `Neg` is its own
/// inverse, so all three are invertible; `Square`, `Relu` and `Sqrt` are not, which is
/// why an element function containing them is refused rather than repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UnaryOp {
    /// Arithmetic negation.
    Neg,
    /// The exponential.
    Exp,
    /// The natural logarithm, undefined on a non-positive operand.
    Ln,
    /// The square root, undefined on a negative operand.
    Sqrt,
    /// The square.
    Square,
    /// The rectified linear unit, `max(operand, 0)`.
    Relu,
}

impl UnaryOp {
    /// The operator as it is written and read in a diagnostic.
    pub const fn name(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Exp => "exp",
            UnaryOp::Ln => "ln",
            UnaryOp::Sqrt => "sqrt",
            UnaryOp::Square => "square",
            UnaryOp::Relu => "relu",
        }
    }

    /// The operator applied to one operand, uncanonicalized.
    pub fn apply(self, operand: Expr) -> Expr {
        Expr::Unary(self, Box::new(operand))
    }

    /// The operator read as an inverse of `self`, or `None` when there is none.
    ///
    /// # Invariant
    /// Only involutions pair here (`Neg`, `Exp` with `Ln`), and the absence is the refusal:
    /// `Sqrt` inverts on a half-line and `Square` and `Relu` do not invert at all, so an
    /// approximation of any of them is a wrong answer carrying a proof attached to it.
    pub const fn inverse(self) -> Option<UnaryOp> {
        match self {
            UnaryOp::Neg => Some(UnaryOp::Neg),
            UnaryOp::Exp => Some(UnaryOp::Ln),
            UnaryOp::Ln => Some(UnaryOp::Exp),
            UnaryOp::Sqrt | UnaryOp::Square | UnaryOp::Relu => None,
        }
    }

    pub(crate) const fn precedence(self) -> u8 {
        match self {
            UnaryOp::Neg => 5,
            UnaryOp::Exp | UnaryOp::Ln | UnaryOp::Sqrt | UnaryOp::Relu => 6,
            UnaryOp::Square => 7,
        }
    }
}

/// The exponential of an expression.
pub fn exp(operand: Expr) -> Expr {
    UnaryOp::Exp.apply(operand)
}

/// The natural logarithm of an expression.
pub fn ln(operand: Expr) -> Expr {
    UnaryOp::Ln.apply(operand)
}

/// The square root of an expression.
pub fn sqrt(operand: Expr) -> Expr {
    UnaryOp::Sqrt.apply(operand)
}

/// The square of an expression.
pub fn square(operand: Expr) -> Expr {
    UnaryOp::Square.apply(operand)
}

/// The rectified linear unit of an expression.
pub fn relu(operand: Expr) -> Expr {
    UnaryOp::Relu.apply(operand)
}

impl Add for Expr {
    type Output = Expr;
    fn add(self, right: Expr) -> Expr {
        BinaryOp::Add.apply(self, right)
    }
}

impl Sub for Expr {
    type Output = Expr;
    fn sub(self, right: Expr) -> Expr {
        BinaryOp::Sub.apply(self, right)
    }
}

impl Mul for Expr {
    type Output = Expr;
    fn mul(self, right: Expr) -> Expr {
        BinaryOp::Mul.apply(self, right)
    }
}

impl Div for Expr {
    type Output = Expr;
    fn div(self, right: Expr) -> Expr {
        BinaryOp::Div.apply(self, right)
    }
}

impl Neg for Expr {
    type Output = Expr;
    fn neg(self) -> Expr {
        UnaryOp::Neg.apply(self)
    }
}

impl From<f64> for Expr {
    fn from(value: f64) -> Expr {
        Expr::Constant(value)
    }
}

impl From<Variable> for Expr {
    fn from(variable: Variable) -> Expr {
        Expr::Variable(variable)
    }
}

//! Symbolic scalar expressions: the trees the fusion engine reads, rewrites and refuses.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::operators::{BinaryOp, UnaryOp};

/// A named free value an expression reads.
///
/// # Invariant
/// The name is the whole identity: there is no scope, no binding and no type, so an
/// expression tree is alpha-normal only in the sense that two spellings of one variable
/// are the same variable when they are spelled the same. The well-known constants below
/// are the slots the derivation reads and writes; a caller that needs a slot of its own
/// names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Variable(&'static str);

impl Variable {
    /// The element datum of the step being fused.
    pub const DATUM: Variable = Variable("d");
    /// The launch extent `n` the fused step dispatches over — `KernelFragment::runtime_scalars`
    /// is what leaves before it.
    ///
    /// # Invariant
    /// It is **the ABI's first scalar**, one number for the whole launch and one for every lane
    /// of it, so a projection reading it is still one value per fused step rather than one per
    /// element: hoisting it out of the step loop changes no lane's answer.
    pub const EXTENT: Variable = Variable("n");
    /// The invocation index `i` the fused step is running — `KernelFragment::scalar_leaves`
    /// is the leaf it arrives as.
    ///
    /// # Invariant
    /// It is **the domain's last leaf**, and it is the fused step's own: composing binds the
    /// consumer's parameters to the producer's position for position
    /// (`compose_fan_in`'s `ParameterCountsDiffer`), so a marked expression that reads it reads
    /// the index of the step that runs it, not of the step that was replaced.
    pub const INVOCATION: Variable = Variable("i");
    /// The producer reduction's value `r_N`, the parameter the element function is a
    /// function of.
    ///
    /// # Invariant
    /// It is [`Self::producer`]'s `0`, because the name is the whole identity: a consumer of one
    /// producer reads the slot it always has, so a pair's canonical form does not move.
    pub const PRODUCER: Variable = Variable("r");
    /// The consumer reduction's rolling state `t`, the slot the loop carries.
    pub const CONSUMER_STATE: Variable = Variable("t");
    /// The repair function's own first argument `x`.
    pub const REPAIR_ARGUMENT: Variable = Variable("x");
    /// The producer's value after the step, `r_(N+1)`.
    pub const PRODUCER_NEXT: Variable = Variable("r'");
    /// The consumer's state after the step, `t'_(N+1)`.
    pub const CONSUMER_NEXT: Variable = Variable("t'");
    /// The abstracted element parameter `c` the inversion is taken with respect to.
    pub const PARAMETER: Variable = Variable("c");
    /// A per-element term of custom logic, as attention's bias or slope would be.
    pub const BIAS: Variable = Variable("bias");

    /// A slot named by the caller.
    pub const fn new(name: &'static str) -> Variable {
        Variable(name)
    }

    /// The name this slot is written and read with.
    pub const fn name(self) -> &'static str {
        self.0
    }

    /// The value the `ordinal`-th absorbed input's producer forwards, or `None` past the table.
    ///
    /// # Invariant
    /// **One slot per absorbed input ordinal**, and `0` is [`Self::PRODUCER`] — the pair's own
    /// spelling — so the shape widens without moving anything a chain already read. `None` past
    /// [`FAN_IN_INPUTS`] rather than a name the table does not hold.
    pub const fn producer(ordinal: usize) -> Option<Variable> {
        if ordinal < PRODUCERS.len() {
            Some(PRODUCERS[ordinal])
        } else {
            None
        }
    }
}

/// The producer slots a fan-in's consumer reads its inputs as.
///
/// # Invariant
/// [`Variable::PRODUCER`] is the first, so one producer is the chain's own spelling, and the width
/// is [`FAN_IN_INPUTS`]: a region reader refuses a wider consumer by name rather than naming a slot
/// this table does not hold.
const PRODUCERS: [Variable; FAN_IN_INPUTS] = [
    Variable::PRODUCER,
    Variable("r1"),
    Variable("r2"),
    Variable("r3"),
    Variable("r4"),
    Variable("r5"),
    Variable("r6"),
    Variable("r7"),
];

/// How many of a fan-in consumer's inputs the symbolic language names.
///
/// # Invariant
/// It is the width of [`PRODUCERS`], and it is a capability bound rather than a licence: a region
/// that reads more producers than this is refused by name, never admitted with two of them read as
/// one.
pub const FAN_IN_INPUTS: usize = 8;

impl std::fmt::Display for Variable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

/// A scalar expression: variables, constants, and operators over them.
///
/// # Invariant
/// A tree, never canonicalized on construction: an element function's custom logic is
/// visible only in the tree the logic wrote, and a constructor that flattened it would
/// hide `d + bias` inside a sorted sum before anything had read it. [`normalize`] is the
/// one place that canonicalizes, and every comparison in this crate goes through it.
#[derive(Debug, Clone)]
pub enum Expr {
    /// A literal.
    Constant(f64),
    /// A named free value.
    Variable(Variable),
    /// A binary operator over two operands.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// A unary operator over one operand.
    Unary(UnaryOp, Box<Expr>),
}

impl Expr {
    /// A literal.
    pub fn constant(value: f64) -> Expr {
        Expr::Constant(value)
    }

    /// A reference to a named free value.
    pub fn variable(variable: Variable) -> Expr {
        Expr::Variable(variable)
    }

    /// The expression as a literal, when it is one.
    pub fn as_constant(&self) -> Option<f64> {
        match self {
            Expr::Constant(value) => Some(*value),
            _ => None,
        }
    }

    /// How many times `target` occurs in this expression.
    ///
    /// # Invariant
    /// Counts occurrences, not variables, and counts a whole subtree as one occurrence: an
    /// inversion is refused for a parameter that occurs twice however the two occurrences
    /// were spelled, so a caller cannot make a repeated parameter look like a single one by
    /// re-spelling it.
    pub fn occurrences(&self, target: &Expr) -> usize {
        let here = usize::from(self == target);
        match self {
            Expr::Constant(_) | Expr::Variable(_) => here,
            Expr::Unary(_, operand) => here + operand.occurrences(target),
            Expr::Binary(_, left, right) => {
                here + left.occurrences(target) + right.occurrences(target)
            }
        }
    }

    /// Every free variable this expression reads, in name order.
    ///
    /// # Invariant
    /// Each variable once: this is what the numeric cross-check samples over, and a
    /// repeated variable in the list would sample it twice and report two points where
    /// the agreement it measured is one.
    pub fn variables(&self) -> BTreeSet<Variable> {
        let mut found = BTreeSet::new();
        self.collect_variables(&mut found);
        found
    }

    /// This expression with every variable in `bindings` replaced by its bound value.
    ///
    /// # Invariant
    /// Simultaneous, never recursive: a binding's value is substituted as written, so an
    /// expression cannot grow by substituting a variable into itself, which is what makes
    /// this total on every pair of expressions.
    pub fn substitute(&self, bindings: &BTreeMap<Variable, Expr>) -> Expr {
        match self {
            Expr::Constant(value) => Expr::Constant(*value),
            Expr::Variable(variable) => bindings
                .get(variable)
                .cloned()
                .unwrap_or_else(|| Expr::Variable(*variable)),
            Expr::Unary(operator, operand) => {
                Expr::Unary(*operator, Box::new(operand.substitute(bindings)))
            }
            Expr::Binary(operator, left, right) => Expr::Binary(
                *operator,
                Box::new(left.substitute(bindings)),
                Box::new(right.substitute(bindings)),
            ),
        }
    }

    fn collect_variables(&self, found: &mut BTreeSet<Variable>) {
        match self {
            Expr::Constant(_) => {}
            Expr::Variable(variable) => {
                found.insert(*variable);
            }
            Expr::Unary(_, operand) => operand.collect_variables(found),
            Expr::Binary(_, left, right) => {
                left.collect_variables(found);
                right.collect_variables(found);
            }
        }
    }
}

/// Substituted with each pair in turn, so a caller need not build the map.
pub fn substitute_all(expr: &Expr, bindings: &[(Variable, Expr)]) -> Expr {
    let map: BTreeMap<Variable, Expr> = bindings.iter().cloned().collect();
    expr.substitute(&map)
}

fn rank(expr: &Expr) -> u8 {
    match expr {
        Expr::Constant(_) => 0,
        Expr::Variable(_) => 1,
        Expr::Unary(..) => 2,
        Expr::Binary(..) => 3,
    }
}

impl PartialEq for Expr {
    /// Structural, with literals compared by [`f64::total_cmp`].
    ///
    /// # Invariant
    /// Agrees with [`Ord`] on every pair, which is what lets the canonicalizer sort
    /// operands and the refusals render one: `0.0` and `-0.0` are different expressions,
    /// and a `NaN` literal equals itself rather than comparing unequal to everything.
    fn eq(&self, other: &Expr) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Expr {}

impl PartialOrd for Expr {
    fn partial_cmp(&self, other: &Expr) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Expr {
    /// A total order over the shape, used only to sort commutative operands.
    ///
    /// # Invariant
    /// Total and structural, so a canonical form is a function of the tree alone: two
    /// runs on two machines must sort the same operands the same way, which is the
    /// determinism discipline the schema hash also rests on.
    fn cmp(&self, other: &Expr) -> Ordering {
        match (self, other) {
            (Expr::Constant(left), Expr::Constant(right)) => left.total_cmp(right),
            (Expr::Variable(left), Expr::Variable(right)) => left.cmp(right),
            (Expr::Unary(left_op, left), Expr::Unary(right_op, right)) => {
                left_op.cmp(right_op).then_with(|| left.cmp(right))
            }
            (
                Expr::Binary(left_op, left_left, left_right),
                Expr::Binary(right_op, right_left, right_right),
            ) => left_op
                .cmp(right_op)
                .then_with(|| left_left.cmp(right_left))
                .then_with(|| left_right.cmp(right_right)),
            (left, right) => rank(left).cmp(&rank(right)),
        }
    }
}

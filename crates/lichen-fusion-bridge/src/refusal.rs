//! Why a marked subexpression did not become one symbolic expression.

use std::fmt;

use lichen_kernel_ir::{KernelBin, KernelId, ScalarClass, ValueId};

use crate::operators::operator_spelling;

/// The form of a marked subexpression the symbolic language does not carry.
///
/// # Invariant
/// One variant per form that is refused, and the variant is the whole of the *why*: a
/// refusal names the instruction it met or the position it met it at. The subset that *is*
/// read has no variant here — its coverage is the operator table and the read roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeRefusal {
    /// The marked value is not a value of the body the fragment carries.
    NotAValue {
        /// The marked value.
        marked: ValueId,
    },
    /// The marked value reads the fragment's own domain — the lane index.
    ReadsTheDomain {
        /// The parameter leaf the projection read.
        index: u32,
    },
    /// The marked value reads a buffer in a projection whose slots are its parameters.
    ReadsABuffer,
    /// The projection reads two different carried-state slots, which one symbolic slot
    /// cannot stand for.
    AmbiguousState {
        /// The carried-state slots it read.
        slots: Vec<u32>,
    },
    /// The marked value computes in the machine-integer class, which `f64` does not model.
    MachineInteger {
        /// The value computed in the integer class.
        value: ValueId,
    },
    /// The marked value applies an operator the symbolic language has no counterpart for.
    UnsupportedOperator {
        /// The operator, as the lowered kernel IR names it.
        operator: KernelBin,
    },
    /// The marked value reads an input buffer whose element position cannot be read as a
    /// constant.
    UnknownReadPosition,
    /// The marked value writes an output buffer.
    BufferWrite {
        /// The write's element position.
        position: usize,
    },
    /// The marked value reaches a shared slot, which a symbolic expression cannot stage.
    SharedAccess {
        /// The shared slot's ordinal in the fragment's slot set.
        slot: usize,
    },
    /// The marked value calls another kernel.
    Call {
        /// The callee's registry id.
        kernel: KernelId,
    },
    /// The marked value selects between two values.
    Select,
    /// The marked value converts between the two scalar classes.
    Conversion {
        /// The class the operand is.
        from: ScalarClass,
        /// The class the conversion leaves behind.
        to: ScalarClass,
    },
    /// The marked value narrows a value to the condition width a select reads.
    BooleanNarrowing,
    /// The marked value floors a number, which the symbolic language cannot name: no
    /// inverse and no algebraic law.
    Floor,
    /// The body does not pass its own well-formedness gate.
    MalformedBody {
        /// What [`lichen_kernel_ir::KernelBody::validate`] reported.
        reason: String,
    },
    /// The expression the mark lowers to outgrew the walk's node budget.
    ExpressionTooLarge {
        /// The budget, in expression nodes.
        budget: usize,
    },
}

impl BridgeRefusal {
    /// The refusal as one sentence, for a run report.
    pub fn message(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for BridgeRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeRefusal::NotAValue { marked } => write!(
                formatter,
                "the mark names {marked:?}, which is not a value of the body it was read from"
            ),
            BridgeRefusal::ReadsTheDomain { index } => write!(
                formatter,
                "the projection reads parameter leaf {index} of the kernel's own domain — the \
                 lane index — and a projection is one function of the fused step's slots"
            ),
            BridgeRefusal::ReadsABuffer => write!(
                formatter,
                "the projection reads a buffer, and the roles it was read under name the \
                 fragment's parameter leaves as its slots: a scalar projection has no buffers"
            ),
            BridgeRefusal::AmbiguousState { slots } => write!(
                formatter,
                "the projection reads carried-state slots {slots:?}, and one symbolic slot \
                 cannot stand for two: a projection that must tell them apart is not \
                 expressible here"
            ),
            BridgeRefusal::MachineInteger { value } => write!(
                formatter,
                "the projection computes {value:?} in the machine-integer class, and the \
                 symbolic language is over real numbers: `Int` wraps and `f64` does not hold \
                 every value"
            ),
            BridgeRefusal::UnsupportedOperator { operator } => write!(
                formatter,
                "the projection applies `{}`, and the symbolic language has no counterpart \
                 for it",
                operator_spelling(*operator)
            ),
            BridgeRefusal::UnknownReadPosition => write!(
                formatter,
                "the projection reads an input buffer whose position is not a constant, and a \
                 read's slot is the position it names"
            ),
            BridgeRefusal::BufferWrite { position } => write!(
                formatter,
                "the projection writes output buffer {position}, and a symbolic expression has \
                 no side effects"
            ),
            BridgeRefusal::SharedAccess { slot } => write!(
                formatter,
                "the projection accesses shared slot {slot}, and a symbolic expression has no \
                 staging: a value one lane leaves for another is not a term it can carry \
                 (`docs/notes/shared-memory-and-barriers.md` §5)"
            ),
            BridgeRefusal::Call { kernel } => write!(
                formatter,
                "the projection calls kernel {kernel}, and the symbolic language has no calls"
            ),
            BridgeRefusal::Select => write!(
                formatter,
                "the projection selects between two values, and the symbolic language has no \
                 conditional"
            ),
            BridgeRefusal::Conversion { from, to } => write!(
                formatter,
                "the projection converts {from:?} to {to:?}, and the symbolic language is one \
                 arithmetic class"
            ),
            BridgeRefusal::BooleanNarrowing => write!(
                formatter,
                "the projection narrows a value to a condition, and a symbolic expression has \
                 no condition to narrow"
            ),
            BridgeRefusal::Floor => write!(
                formatter,
                "the projection computes a floor, and the symbolic language has no counterpart \
                 for it: a floor is not invertible and names no law"
            ),
            BridgeRefusal::MalformedBody { reason } => write!(
                formatter,
                "the fragment's body does not pass its own well-formedness gate: {reason}"
            ),
            BridgeRefusal::ExpressionTooLarge { budget } => write!(
                formatter,
                "the projection outgrew the bridge's budget of {budget} nodes, so a shared \
                 body value would be duplicated past what a report can hold"
            ),
        }
    }
}

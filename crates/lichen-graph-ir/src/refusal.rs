//! Why a graph could not be run, with each refusal naming its own cause.

use std::fmt;

/// Why a graph could not be built or run.
///
/// # Invariant
/// Every variant names what was wrong, not merely that something was: the two refusals
/// that are not about a backend — an edge before its producer, an output count that
/// disagrees with its node — are mistakes in the *graph*, and saying so is the difference
/// between a bug someone fixes and one someone argues about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphRefusal {
    /// A node read a value the graph has not produced yet.
    ///
    /// # Invariant
    /// What a cycle looks like when a graph is built by appending in evaluation order: an
    /// edge can only point backwards. Refused at build time, because a graph that could
    /// hold one would make every run a question about cycles.
    EdgeBeforeItsProducer {
        node: usize,
        value: usize,
        defined: usize,
    },
    /// A node was pushed with an output count its own body does not agree with.
    ///
    /// # Invariant
    /// A kernel produces `fragment.outputs`; another number is asking for value numbers
    /// that will mean something else for the rest of the graph.
    OutputCount {
        node: usize,
        declared: usize,
        claimed: usize,
    },
    /// A node read a value number the graph does not have.
    ///
    /// # Invariant
    /// Distinct from [`Self::EdgeBeforeItsProducer`], which is caught while the graph is
    /// built; this one is a value table that came from a caller.
    UnknownValue { node: usize, value: usize },
    /// The source function's return was recorded twice.
    ///
    /// # Invariant
    /// Letting the second opinion win is a wrong answer that still runs; `recorded` says
    /// how many values the first call recorded, which is what tells them apart.
    ReturnAlreadyRecorded { recorded: usize },
    /// The run was handed the wrong number of arguments for the graph.
    ///
    /// # Invariant
    /// Distinct from [`Self::InputArity`] and node-less, because it is about the run
    /// rather than any one dispatch: a graph's inputs are its first values, and a
    /// different number names a different graph. The repair differs too — this one is
    /// fixed at the call.
    RunArity { wanted: usize, got: usize },
    /// A node's inputs did not match the number of buffers its fragment reads.
    ///
    /// # Invariant
    /// The count comes from [`KernelFragment::inputs`], not `param_shape`, because a
    /// parallel fragment's shape is `(config, index)` however many buffers it reads: the
    /// buffers are bound, not passed. A different length is not a run with fewer
    /// arguments, but a different program.
    InputArity {
        node: usize,
        wanted: usize,
        got: usize,
    },
    /// A node's inputs were not all the same length.
    ///
    /// # Invariant
    /// A dispatch over `[0, count)` reads `count` elements of every input, so unequal
    /// lengths mean a lane reads past the shorter one; the graph is where the message can
    /// name both lengths.
    RaggedInputs {
        node: usize,
        first: usize,
        other: usize,
    },
    /// A dispatch was given a number where it wanted a buffer.
    ///
    /// # Invariant
    /// The mirror of [`Self::CountNotANumber`] and separate for the same reason: reading
    /// the number as a one-element host vector would run a kernel nobody wrote. It names
    /// the node and the value, because a sentence saying only what the value was leaves a
    /// reader of a forty-node graph to find which demand was wrong.
    NotBufferData {
        node: usize,
        value: usize,
        found: &'static str,
    },
    /// A dispatch's count resolved to something that is not a number.
    ///
    /// # Invariant
    /// A count is an extent, so the refusal is about the value rather than the role. It
    /// carries the node and the value like [`Self::NotBufferData`], for the same reason:
    /// a count edge is one of a node's two roles.
    CountNotANumber {
        node: usize,
        value: usize,
        found: &'static str,
    },
    /// A count was negative.
    ///
    /// # Invariant
    /// `i64` in the value type so this is reportable rather than wrapped: a `usize` would
    /// have turned `-1` into an enormous extent. This is the one refusal with no node,
    /// because a number is asked for as an extent by a node *or* handed back as a return
    /// value nobody asked for; naming one would invent a node where the mistake is in none.
    CountNegative { number: i64 },
    /// The policy asked for is not something a backend can do.
    ///
    /// # Invariant
    /// Named rather than approximated: running the policy as a slower one reports a
    /// number for a schedule nobody asked about, and the gap is a speedup nobody can
    /// account for.
    PolicyUnsupported {
        policy: &'static str,
        reason: &'static str,
    },
    /// The backend refused. `what` names the call, because "the backend said no"
    /// is not a cause.
    Backend { what: &'static str, reason: String },
}

impl fmt::Display for GraphRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphRefusal::EdgeBeforeItsProducer {
                node,
                value,
                defined,
            } => write!(
                f,
                "node {node} reads value {value}, but the graph has only defined {defined} \
                 value(s) at that point, so it names a value that does not exist yet. Nodes are \
                 appended in evaluation order and an edge can only point backwards, so this is \
                 how a cycle would be written."
            ),
            GraphRefusal::OutputCount {
                node,
                declared,
                claimed,
            } => write!(
                f,
                "node {node} was pushed claiming {claimed} output(s) while its own body \
                 declares {declared}, so the value numbers after it would be misaligned by {}.",
                claimed.abs_diff(*declared)
            ),
            GraphRefusal::UnknownValue { node, value } => write!(
                f,
                "node {node} reads value {value}, which the graph does not have."
            ),
            GraphRefusal::ReturnAlreadyRecorded { recorded } => write!(
                f,
                "this graph's source function already had its return recorded ({recorded} \
                 value(s)), and it has been recorded a second time. Which values a function \
                 returns is one answer, so the two cannot be merged — and letting the second \
                 one win would be a wrong answer that still runs."
            ),
            GraphRefusal::RunArity { wanted, got } => write!(
                f,
                "this graph takes {wanted} argument(s) and was run with {got}. A different \
                 number of arguments is a different graph: its first {wanted} value(s) are \
                 the ones the recorded dispatches read."
            ),
            GraphRefusal::InputArity { node, wanted, got } => write!(
                f,
                "node {node}'s fragment reads {wanted} input buffer(s) but {got} value(s) were \
                 given it, which is a different program rather than a run with fewer arguments."
            ),
            GraphRefusal::RaggedInputs { node, first, other } => write!(
                f,
                "node {node}'s inputs are not all the same length: {first} and {other}. A \
                 dispatch over [0, count) reads every input to `count`, so a lane would read \
                 past the shorter one."
            ),
            GraphRefusal::NotBufferData { node, value, found } => write!(
                f,
                "node {node} was given value {value} as a buffer, and that value is {found}. A \
                 number is not a one-element buffer, and reading it as one would run the \
                 dispatch against data nobody wrote."
            ),
            GraphRefusal::CountNotANumber { node, value, found } => write!(
                f,
                "node {node} reads its count from value {value}, and that value is {found}. A \
                 count is an extent, so the only thing that can be one is a number."
            ),
            GraphRefusal::CountNegative { number } => {
                write!(f, "a count is {number}, and a count cannot be negative.")
            }
            GraphRefusal::PolicyUnsupported { policy, reason } => {
                write!(f, "the {policy} schedule is not available: {reason}")
            }
            GraphRefusal::Backend { what, reason } => {
                write!(f, "the backend failed {what}: {reason}")
            }
        }
    }
}

impl std::error::Error for GraphRefusal {}

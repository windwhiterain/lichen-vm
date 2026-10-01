//! Why a graph could not be run, with each refusal naming its own cause.

use std::fmt;

/// Why a graph could not be built or run.
///
/// Every variant names what was wrong, not merely that something was. The
/// alternative is a `String` and a caller who has to guess, and the two refusals
/// that are not about a backend at all — an edge before its producer, an output
/// count that disagrees with its node — are mistakes in the *graph*, and saying
/// so is the difference between a bug someone fixes and a bug someone argues
/// about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphRefusal {
    /// A node read a value the graph has not produced yet.
    ///
    /// This is what a cycle looks like when a graph is built by appending in
    /// evaluation order: an edge can only point backwards, so the only way to
    /// name a later value is to name one that does not exist. Refused at the
    /// point of building, because a graph that could hold one would make every
    /// run of it a question about cycles instead of a walk of a list.
    EdgeBeforeItsProducer {
        node: usize,
        value: usize,
        defined: usize,
    },
    /// A node was pushed with an output count its own body does not agree with.
    ///
    /// A kernel produces `fragment.outputs`. A caller passing a different number
    /// is asking for value numbers that will mean something else for the rest of
    /// the graph, so it is refused here rather than producing a value table that
    /// is quietly misaligned.
    OutputCount {
        node: usize,
        declared: usize,
        claimed: usize,
    },
    /// A node read a value number the graph does not have.
    ///
    /// Distinct from [`Self::EdgeBeforeItsProducer`]: that one is caught while
    /// the graph is built, and this one is a value table that came from a caller
    /// rather than from [`crate::Graph::push`].
    UnknownValue { node: usize, value: usize },
    /// The source function's return was recorded twice.
    ///
    /// A lowering that answers "what does this function return" twice has two
    /// opinions, and letting the second one win is a wrong answer that still runs.
    /// `recorded` is how many values the first call recorded, because the number
    /// is usually what tells the two opinions apart.
    ReturnAlreadyRecorded { recorded: usize },
    /// A node's inputs did not resolve to the number of buffers its fragment
    /// declares.
    ///
    /// The fragment says how many input slots it has, and a slot list of another
    /// length is not a run with fewer arguments — it is a different program.
    InputArity {
        node: usize,
        wanted: usize,
        got: usize,
    },
    /// A node's inputs were not all the same length.
    ///
    /// A dispatch over `[0, count)` reads `count` elements of every input, so
    /// two inputs of different lengths means one lane would read past the
    /// shorter one. The backend would refuse the run, but the graph is where the
    /// mistake is, and the message here can name both lengths.
    RaggedInputs {
        node: usize,
        first: usize,
        other: usize,
    },
    /// Something wanted host data from a value that is not host data.
    ///
    /// Named with what the value *is*, because the two cases have different
    /// fixes: a value still pending needs a wait, and one already waited for
    /// needs a fetch, and telling a caller only that it is "not host data" hands
    /// them the first when they needed the second.
    ///
    /// **This is the caller's refusal, not a node's.** A dispatch never reads a
    /// value on the host, so nothing inside a run can ask for it; the only
    /// reader is whoever takes a result home, and telling that reader "not host
    /// data" is a genuine answer rather than a sign of a hole.
    NotHostData { found: &'static str },
    /// A dispatch was given a number where it wanted a buffer.
    ///
    /// The mirror of [`Self::CountNotANumber`], and separate for the same
    /// reason: the two are different mistakes in different positions, and a
    /// caller told only "wrong shape" has to work out which one they hit. There
    /// is a repair that looks reasonable here — read the number as a one-element
    /// host vector — and taking it would run a kernel nobody wrote.
    NotBufferData { found: &'static str },
    /// A dispatch's count resolved to something that is not a number.
    ///
    /// A count is an extent, so the only thing that can be one is a number, and
    /// this refusal is about the *value* rather than about the role — the role was
    /// right and the value was not. Naming what the value is what tells a caller
    /// whether the edge points at the wrong value or the builder wrote a number
    /// into a slot meant for data.
    CountNotANumber { found: &'static str },
    /// A dispatch's count was negative.
    ///
    /// `i64` in the value type so this is reportable rather than wrapped: a
    /// `usize` would have turned `-1` into an enormous extent and dispatched
    /// over memory nobody owns.
    CountNegative { number: i64 },
    /// The policy asked for is not something a backend can do.
    ///
    /// Named rather than approximated, and the reason is that the two
    /// approximations are both wrong in a way that is hard to see: running the
    /// policy as a slower one reports a number for a schedule that is not the
    /// one being asked about, and the gap is a *speedup* nobody can account for.
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
            GraphRefusal::InputArity { node, wanted, got } => write!(
                f,
                "node {node}'s fragment declares {wanted} input buffer(s) but {got} value(s) \
                 were given it, which is a different program rather than a run with fewer \
                 arguments."
            ),
            GraphRefusal::RaggedInputs { node, first, other } => write!(
                f,
                "node {node}'s inputs are not all the same length: {first} and {other}. A \
                 dispatch over [0, count) reads every input to `count`, so a lane would read \
                 past the shorter one."
            ),
            GraphRefusal::NotHostData { found } => write!(
                f,
                "host data was asked of a value that is {found}: one that has not been waited \
                 for needs a wait first, and one that has needs a fetch first."
            ),
            GraphRefusal::NotBufferData { found } => write!(
                f,
                "a buffer was asked of a value that is {found}. A number is not a one-element \
                 buffer, and reading it as one would run the dispatch against data nobody wrote."
            ),
            GraphRefusal::CountNotANumber { found } => write!(
                f,
                "a dispatch's count was read from a value that is {found}. A count is an \
                 extent, so the only thing that can be one is a number."
            ),
            GraphRefusal::CountNegative { number } => write!(
                f,
                "a dispatch's count is {number}, and a count cannot be negative."
            ),
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

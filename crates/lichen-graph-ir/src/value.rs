//! What a graph node hands to the next one, and what it costs to be ready.

use std::fmt;
use std::sync::{Arc, Mutex};

use lichen_kernel_ir::{BufferSlot, Pending, ResidentId};

use crate::GraphRefusal;

/// The one submission that several output values of the same node share.
///
/// A dispatch with three outputs is **one** submission with three ids, and the
/// submission is waited for once. So the values cannot each hold it — `Box<dyn
/// Pending>` is consumed by the wait, deliberately, because a second wait would
/// prove whatever the slot is running by then rather than this submission. They
/// share an `Arc` instead, and `settle` takes the submission out from under the
/// lock, so the second value to be settled finds nothing to wait for and is
/// correct rather than dangerous.
type Shared<'backend> = Arc<Mutex<Option<Box<dyn Pending + 'backend>>>>;

/// A value a graph node produces.
///
/// # The three states, and why the middle one is its own case
///
/// A dispatch can be in three situations, and the difference between them is not
/// one of degree:
///
/// - **not yet asked of the device**;
/// - **submitted, and the device may not be finished** — the contents are
///   whatever it has written so far;
/// - **waited for** — the contents are what the kernel computed.
///
/// The middle one is worth naming separately rather than as a buffer whose
/// freshness you have to track. It is the state a value is in for exactly as
/// long as the host has other work to do, and **spending that window well is the
/// entire reason for running a graph this way**. A representation that folded it
/// into "a buffer" would either lie about the contents or make every reader
/// responsible for knowing whether it had to wait first.
///
/// # So the difference is in the type, not in a rule
///
/// A kernel node can consume a pending value: recording a dispatch against a
/// buffer only *names* it, and by the time the device reads it the producer has
/// been recorded ahead of it. That is not a rule the runner chooses to follow. It
/// is what the match arms are.
///
/// **It was an asymmetry, once.** A host call could not do the same, because it
/// reads the data, so matching on [`Self::Pending`] was what a demand point
/// *is*. With one kind of node there is nothing on the other side of the
/// asymmetry, and the runner has no demand point in the middle of a run at all —
/// it settles every submission at the end. The pending state survives because the
/// chain still needs it, not because anything branches on it.
///
/// # And a number is a fourth case, not a shorter buffer
///
/// [`Self::Int`] is here because a dispatch's extent is a number and a number is
/// not `Vec<i64>`. It is **never pending**, because a device does not produce
/// one, which is what lets a count edge be read without a wait and lets the two
/// roles of a value — a count and a buffer — be asked separately and refused
/// separately.
///
/// **A number is the whole of the not-a-buffer case, and that is not an
/// oversight to be tidied up later.** Whether a value is a buffer is a *role*,
/// not a type category, and the roles are already explicit: [`Self::slot`] is
/// the filter that keeps the buffers out of a dispatch's edge list and
/// [`Self::as_count`] is the one that keeps a number in. A jit'd function may be
/// handed arbitrary lichen values, and a recording sorts them into roles by
/// asking; a value table that also carried "some other host value" would be a
/// third place the same question is answered, and one that could disagree with
/// the other two. Nothing produces one yet — a node that *computes* a number is
/// a compiled kernel like any other, and that is the node set this crate has.
pub enum Value<'backend> {
    /// Submitted, and the device may not be done with it.
    Pending {
        submission: Shared<'backend>,
        id: ResidentId,
        count: usize,
    },
    /// Waited for: the device has written it and the contents are there.
    Device { id: ResidentId, count: usize },
    /// Host data. What a caller passes for a graph's own input.
    Host(Vec<i64>),
    /// A number.
    ///
    /// **`i64` rather than `usize` because a graph is a program and a program's
    /// count can be negative**, which is a mistake to be told about by name
    /// rather than one to be wrapped around into an enormous unsigned number.
    Int(i64),
}

impl fmt::Debug for Value<'_> {
    /// By hand, because a value can hold a submission and a `Box<dyn Pending>`
    /// is neither `Debug` nor printable — and the state is the part worth
    /// printing anyway, since whether a value is ready is the question every
    /// caller of a failure message wants answered.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Pending { id, count, .. } => {
                write!(f, "Pending(buffer {:?}, {count} element(s))", id.0)
            }
            Value::Device { id, count } => {
                write!(f, "Device(buffer {:?}, {count} element(s))", id.0)
            }
            Value::Host(host) => write!(f, "Host({} element(s))", host.len()),
            Value::Int(number) => write!(f, "Int({number})"),
        }
    }
}

impl<'backend> Value<'backend> {
    /// A device value the caller already waited for — what a graph's own inputs
    /// are, and what a settled value becomes.
    pub fn device(id: ResidentId, count: usize) -> Self {
        Value::Device { id, count }
    }

    /// Host data the caller already holds.
    pub fn host(data: Vec<i64>) -> Self {
        Value::Host(data)
    }

    /// A number, for a count edge or a graph's own argument.
    pub fn int(number: i64) -> Self {
        Value::Int(number)
    }

    /// The outputs of one submission, none of them waited for.
    pub(crate) fn pending_all(
        submission: Box<dyn Pending + 'backend>,
        ids: Vec<ResidentId>,
        count: usize,
    ) -> Vec<Self> {
        let shared: Shared<'backend> = Arc::new(Mutex::new(Some(submission)));
        ids.into_iter()
            .map(|id| Value::Pending {
                submission: Arc::clone(&shared),
                id,
                count,
            })
            .collect()
    }

    /// The outputs of one run that was already waited for.
    pub(crate) fn device_all(ids: Vec<ResidentId>, count: usize) -> Vec<Self> {
        ids.into_iter()
            .map(|id| Value::Device { id, count })
            .collect()
    }

    /// The buffer this names, for a dispatch that only records against it.
    ///
    /// **A pending value is a buffer.** That is the point: a dispatch against it
    /// does not read the data, it names where the data will be, and the
    /// submission producing it was recorded first. Refusing a pending value here
    /// would rule out the one shape that pays.
    ///
    /// **A number is refused rather than borrowed.** The tempting repair is to
    /// treat it as a one-element host vector, and that produces a run that
    /// succeeds on a kernel nobody wrote: the count is not a buffer, so this is a
    /// different mistake from a count that is data, and it gets its own message.
    ///
    /// **This is the filter, and it is why there is no "not a buffer" category
    /// to add kinds to.** A jit'd function may be handed arbitrary lichen
    /// values, and a recording sorts them into roles by asking each one. The
    /// answers live here and in [`Self::as_count`], one method per role, rather
    /// than in a type that claims to know which role a value has before anyone
    /// has asked.
    pub fn slot(&self) -> Result<BufferSlot<'_>, GraphRefusal> {
        match self {
            Value::Host(host) => Ok(BufferSlot::Host(host)),
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(BufferSlot::Resident(*id)),
            Value::Int(_) => Err(GraphRefusal::NotBufferData {
                found: self.state(),
            }),
        }
    }

    /// This value as a count, for a dispatch's extent.
    ///
    /// The mirror of [`Self::slot`], and separate for the same reason: a value
    /// asked for one role and refused is a different mistake from a value asked
    /// for the other and refused, and a caller who is told only "wrong shape"
    /// has to work out which of the two they hit.
    ///
    /// **A number is the only native value a count can be**, and the other kinds
    /// say so by name rather than by being a `None` somewhere upstream. A count
    /// is an extent, so refusing a pointer here is about the value, not about the
    /// role — the role was right and the value was not.
    pub fn as_count(&self) -> Result<usize, GraphRefusal> {
        match self {
            Value::Int(number) => usize::try_from(*number)
                .map_err(|_| GraphRefusal::CountNegative { number: *number }),
            other => Err(GraphRefusal::CountNotANumber {
                found: other.state(),
            }),
        }
    }

    /// How many elements this holds, or `None` for a number.
    ///
    /// `None` rather than `0` because a number has no length and reporting
    /// `0` for it would be a length a caller could act on.
    pub fn count(&self) -> Option<usize> {
        match self {
            Value::Host(host) => Some(host.len()),
            Value::Device { count, .. } | Value::Pending { count, .. } => Some(*count),
            Value::Int(_) => None,
        }
    }

    /// Whether this value's contents are there yet.
    pub fn is_ready(&self) -> bool {
        !matches!(self, Value::Pending { .. })
    }

    /// Wait for the submission behind this value, if there is one.
    ///
    /// **This is a demand point**, and it is the only way a pending value becomes
    /// readable. Waiting a submission shared by several values happens once; the
    /// rest find it already gone and are correct rather than wrong.
    pub fn settle(&mut self) -> Result<(), GraphRefusal> {
        let Value::Pending {
            submission,
            id,
            count,
            ..
        } = self
        else {
            return Ok(());
        };
        let (id, count) = (*id, *count);
        // Taken under the lock, so two values of one node cannot both wait. The
        // lock is not held across the wait itself: another thread must be able to
        // settle a different submission while this one blocks on the device.
        let waiting = submission.lock().unwrap().take();
        if let Some(waiting) = waiting {
            waiting.wait().map_err(|reason| GraphRefusal::Backend {
                what: "waiting for a submission",
                reason,
            })?;
        }
        *self = Value::Device { id, count };
        Ok(())
    }

    /// What this value currently is, for a refusal that has to say.
    ///
    /// **The readiness and the kind are two different facts, and both are here.**
    /// "A submission that has not been waited for" and "a device buffer that was
    /// waited for" are the same kind with different fixes, so a refusal that named
    /// only the kind would hand a caller the first when they needed the second.
    fn state(&self) -> &'static str {
        match self {
            Value::Host(_) => "host data",
            Value::Device { .. } => "a device buffer that was waited for",
            Value::Pending { .. } => "a submission that has not been waited for",
            Value::Int(_) => "a number",
        }
    }
}

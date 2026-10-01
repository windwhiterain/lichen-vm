//! What a graph node hands to the next one, and what it costs to be ready.

use std::fmt;
use std::sync::{Arc, Mutex};

use lichen_kernel_ir::{BufferSlot, ParallelBackend, Pending, ResidentId};

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
/// been recorded ahead of it. A **native node cannot**, because a host call
/// reads the data — and matching on [`Self::Pending`] is what a demand point
/// *is*.
///
/// That asymmetry is not a rule the runner chooses to follow. It is what the
/// match arms are.
pub enum Value<'backend> {
    /// Submitted, and the device may not be done with it.
    Pending {
        submission: Shared<'backend>,
        id: ResidentId,
        count: usize,
    },
    /// Waited for: the device has written it and the contents are there.
    Device { id: ResidentId, count: usize },
    /// Host data. What a native node produces, and what it can read.
    Host(Vec<i64>),
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
    pub fn slot(&self) -> Result<BufferSlot<'_>, GraphRefusal> {
        match self {
            Value::Host(host) => Ok(BufferSlot::Host(host)),
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(BufferSlot::Resident(*id)),
        }
    }

    /// How many elements this holds.
    pub fn count(&self) -> usize {
        match self {
            Value::Host(host) => host.len(),
            Value::Device { count, .. } | Value::Pending { count, .. } => *count,
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

    /// Settle this value and, if it is on a device, bring it home.
    ///
    /// A transfer as well as a wait, which is the point worth writing down: a
    /// host call cannot read device memory, so **every native node that reads a
    /// kernel's output pays a download for it.** That is the price of host logic
    /// in the middle of a data path, and it is why the shape that pays is a
    /// native node whose inputs are already host-side.
    pub fn to_host(&mut self, backend: &dyn ParallelBackend) -> Result<(), GraphRefusal> {
        self.settle()?;
        let Value::Device { id, count } = *self else {
            return Ok(());
        };
        let fetched = backend
            .fetch(id, count)
            .map_err(|reason| GraphRefusal::Backend {
                what: "fetching a value a native node reads",
                reason,
            })?;
        *self = Value::Host(fetched);
        Ok(())
    }

    /// The host data, assuming [`Self::to_host`] has been called.
    pub fn as_host(&self) -> Result<&[i64], GraphRefusal> {
        match self {
            Value::Host(host) => Ok(host),
            _ => Err(GraphRefusal::NotHostData {
                found: self.state(),
            }),
        }
    }

    /// What this value currently is, for a refusal that has to say.
    fn state(&self) -> &'static str {
        match self {
            Value::Host(_) => "host data",
            Value::Device { .. } => "a device buffer that was waited for",
            Value::Pending { .. } => "a submission that has not been waited for",
        }
    }
}

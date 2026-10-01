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

/// A slot in a registry of values the host owns.
///
/// **A number, not a reference, and the trace walk is what makes that safe.** A
/// value reached through this slot is reached because whoever holds the graph
/// names the slot, and that walk is **recursive**: it asks the node it reaches,
/// and then whatever *that* node holds, and so on. So a pointer into a registry
/// keeps a whole transitive environment alive without the graph enumerating any
/// of it — a closure that captured `x` means whoever captured the closure has
/// captured `x`, and nothing in the graph has to know that.
///
/// **It means nothing outside the process that issued it**, and it is not
/// comparable across two of them, exactly like a kernel id.
pub type NativeValueId = usize;

/// A value the table holds that is neither a buffer nor device memory.
///
/// # Why this is a kind rather than one more variant of a number
///
/// [`Value::Native`] began as a number, because a dispatch's extent is a number
/// and a number is not `Vec<i64>`. That is a true reason and it is the wrong
/// amount of surface, because what was added is not *a count can be dynamic* —
/// it is **the table can hold something that is not a buffer at all**. A number
/// is the first inhabitant of that, and the kind is named now rather than when
/// the second one lands because the second one is known: a lichen closure the
/// graph will call later.
///
/// # What every inhabitant has in common, and why it is worth writing
///
/// **A native value is never pending.** The host made it, so there is no
/// submission behind it and no wait that could be owed. That is what lets
/// [`Value::slot`] and [`Value::as_count`] answer without a submission arm, and
/// it is why a new inhabitant is a compile error in each of them rather than a
/// silent pass-through.
///
/// **It has no extent.** [`Value::count`] is `None` for all of them, because a
/// `0` would be a length a caller could act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Native {
    /// A number, held inline.
    ///
    /// **`i64` rather than `usize` because a graph is a program and a program's
    /// count can be negative**, which is a mistake to be told about by name
    /// rather than one to be wrapped around into an enormous unsigned number.
    Int(i64),
    /// A value the host owns, named by a registry slot.
    ///
    /// **The slot is the whole of it.** A pointer is enough rather than a list
    /// of captures, and the reason is [`NativeValueId`]: the environment is
    /// reached by a recursive walk, so it needs no declaration here. A graph
    /// that named its captures would be a second copy of a fact the trace walk
    /// already holds, and one that could disagree with it.
    Pointer(NativeValueId),
}

impl Native {
    /// What this is, for a refusal that has to say.
    fn state(&self) -> &'static str {
        match self {
            Native::Int(_) => "a number",
            Native::Pointer(_) => "a value the host owns",
        }
    }
}

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
///
/// # And a native value is a fourth case, not a shorter buffer
///
/// [`Self::Native`] is here because a dispatch's extent is a number and a number
/// is not `Vec<i64>`. It is **never pending**, because a device does not produce
/// one, which is what lets a count edge be read without a wait and lets the two
/// roles of a value — a count and a buffer — be asked separately and refused
/// separately. The reason it is a kind of its own rather than an `i64` is
/// [`Native`]: a number is the first thing the table can hold that is not a
/// buffer, and the second is a closure the graph will call later.
///
/// **Nothing inside a graph produces one yet.** A node that *computes* a value
/// the host owns is a host call, and which way out of that is still open. So
/// today a native value enters a graph as an argument and that is the whole of
/// it, and the kind says so rather than leaving the reader to infer it from the
/// absence of an arm.
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
    /// A value that is not a buffer: a number, or a pointer to one the host owns.
    Native(Native),
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
            Value::Native(native) => write!(f, "Native({native:?})"),
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
        Value::Native(Native::Int(number))
    }

    /// A pointer to a value the host owns, for a graph that will call it later.
    ///
    /// **The slot is all this takes**, and that is the point rather than a
    /// shortcut: the value behind it is reached by a recursive trace walk, so its
    /// environment needs no declaration here. See [`NativeValueId`].
    pub fn native_pointer(slot: NativeValueId) -> Self {
        Value::Native(Native::Pointer(slot))
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
    /// **A native value is refused rather than borrowed.** The tempting repair is
    /// to read a number as a one-element host vector, and that produces a run
    /// that succeeds on a kernel nobody wrote: the count is not a buffer, so this
    /// is a different mistake from a count that is data, and it gets its own
    /// message. Naming *what kind of native value* it is rather than only that it
    /// is one is the difference between a caller who knows they passed a number
    /// and one who has to go looking.
    pub fn slot(&self) -> Result<BufferSlot<'_>, GraphRefusal> {
        match self {
            Value::Host(host) => Ok(BufferSlot::Host(host)),
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(BufferSlot::Resident(*id)),
            Value::Native(native) => Err(GraphRefusal::NotBufferData {
                found: native.state(),
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
            Value::Native(Native::Int(number)) => usize::try_from(*number)
                .map_err(|_| GraphRefusal::CountNegative { number: *number }),
            other => Err(GraphRefusal::CountNotANumber {
                found: other.state(),
            }),
        }
    }

    /// How many elements this holds, or `None` for a value with no extent.
    ///
    /// `None` rather than `0` because a native value has no length and reporting
    /// `0` for it would be a length a caller could act on.
    pub fn count(&self) -> Option<usize> {
        match self {
            Value::Host(host) => Some(host.len()),
            Value::Device { count, .. } | Value::Pending { count, .. } => Some(*count),
            Value::Native(_) => None,
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
    ///
    /// **It names the kind, not the category.** "A value the host owns" tells a
    /// caller they passed something that was never a buffer; "a number" tells
    /// them which of the native values it was, and the two have different fixes.
    fn state(&self) -> &'static str {
        match self {
            Value::Host(_) => "host data",
            Value::Device { .. } => "a device buffer that was waited for",
            Value::Pending { .. } => "a submission that has not been waited for",
            Value::Native(native) => native.state(),
        }
    }
}

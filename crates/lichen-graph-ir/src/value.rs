//! What a graph node hands to the next one, and what it costs to be ready.

use std::fmt;
use std::sync::{Arc, Mutex};

use lichen_kernel_ir::{BufferSlot, Pending, ResidentId, ScalarClass};

use crate::GraphRefusal;

/// The one submission that several output values of the same node share.
///
/// # Invariant
/// A dispatch with three outputs is one submission with three ids, waited for once, so
/// the values share an `Arc` rather than each holding a `Box<dyn Pending>` — which the
/// wait consumes, deliberately. `settle` takes the submission out from under the lock,
/// so the second value to settle finds nothing to wait for.
type Shared<'backend> = Arc<Mutex<Option<Box<dyn Pending + 'backend>>>>;

/// The packed payload of `words`, each element at `class`'s width.
///
/// # Invariant
/// The width comes from [`ScalarClass::byte_width`] and nowhere else: host data crosses
/// as bytes because that is what the ABI carries.
fn pack(class: ScalarClass, words: &[i64]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(words.len() * class.byte_width());
    for word in words {
        match class {
            ScalarClass::Int => bytes.extend_from_slice(&word.to_le_bytes()),
            ScalarClass::Float => bytes.extend_from_slice(&(*word as u32).to_le_bytes()),
        }
    }
    bytes
}

/// The elements of a packed payload, one word each — the inverse of [`pack`].
///
/// # Invariant
/// A float element is its bits in the low 32 of the word; the width the bytes were
/// packed at comes from the class, not from a constant here.
fn elements(class: ScalarClass, bytes: &[u8]) -> Vec<i64> {
    bytes
        .chunks_exact(class.byte_width())
        .map(|element| match class {
            ScalarClass::Int => i64::from_le_bytes(element.try_into().unwrap_or_default()),
            ScalarClass::Float => {
                i64::from(u32::from_le_bytes(element.try_into().unwrap_or_default()))
            }
        })
        .collect()
}

/// A value a graph node produces.
///
/// What a value in a graph's table is.
///
/// # Invariant
/// Three states, and the difference is in the type: a buffer not yet asked of the device,
/// one submitted whose contents the device may still be writing, and one waited for. The
/// middle state is named separately because it is the window in which the host has other
/// work — the whole reason to run a graph this way. A number is never pending, so a count
/// edge is read without a wait.
pub enum Value<'backend> {
    /// Submitted, and the device may not be done with it.
    Pending {
        submission: Shared<'backend>,
        id: ResidentId,
        count: usize,
        /// The class the elements are read as, kept across the wait.
        class: ScalarClass,
    },
    /// Waited for: the device has written it and the contents are there.
    Device {
        id: ResidentId,
        count: usize,
        /// The class the elements are to be read as.
        class: ScalarClass,
    },
    /// Host data. What a caller passes for a graph's own input.
    ///
    /// # Invariant
    /// The class is here for the same reason it is on a device value: the payload is those
    /// elements packed at [`ScalarClass::byte_width`] bytes each, so dropping the class
    /// would hand a float buffer's four-byte elements to a backend that reads eight.
    Host { data: Vec<u8>, class: ScalarClass },
    /// A number.
    ///
    /// # Invariant
    /// `i64` rather than `usize`, because a count can be negative and that is a mistake to
    /// be named rather than wrapped into an enormous extent. A count is always an `Int`:
    /// an extent has no class to carry.
    Int(i64),
}

impl fmt::Debug for Value<'_> {
    /// By hand: a `Box<dyn Pending>` is neither `Debug` nor printable, and the state is
    /// the part worth printing.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Pending { id, count, .. } => {
                write!(f, "Pending(buffer {:?}, {count} element(s))", id.0)
            }
            Value::Device { id, count, .. } => {
                write!(f, "Device(buffer {:?}, {count} element(s))", id.0)
            }
            Value::Host { data, class } => {
                write!(f, "Host({} element(s))", data.len() / class.byte_width())
            }
            Value::Int(number) => write!(f, "Int({number})"),
        }
    }
}

impl<'backend> Value<'backend> {
    /// A device value the caller already waited for — what a graph's own inputs
    /// are, and what a settled value becomes.
    pub fn device(id: ResidentId, count: usize, class: ScalarClass) -> Self {
        Value::Device { id, count, class }
    }

    /// Host data the caller already holds, read as `Int` elements.
    ///
    /// # Invariant
    /// The argument is the elements themselves, each packed at `Int`'s width;
    /// [`Self::host_data`] is the same for a class that is not `Int`.
    pub fn host(data: Vec<i64>) -> Self {
        Value::Host {
            data: pack(ScalarClass::Int, &data),
            class: ScalarClass::Int,
        }
    }

    /// Host data of a class the caller names, as that class's packed bytes.
    pub fn host_data(class: ScalarClass, data: Vec<u8>) -> Self {
        Value::Host { data, class }
    }

    /// A number, for a count edge or a graph's own argument.
    pub fn int(number: i64) -> Self {
        Value::Int(number)
    }

    /// The class this value's elements are to be read as, or `None` for a number
    /// (which has no elements).
    pub fn class(&self) -> Option<ScalarClass> {
        match self {
            Value::Host { class, .. }
            | Value::Device { class, .. }
            | Value::Pending { class, .. } => Some(*class),
            Value::Int(_) => None,
        }
    }

    /// The outputs of one submission, none of them waited for.
    ///
    /// # Invariant
    /// `classes` is the fragment's declared class per ordinal; a shorter list falls back
    /// to the ABI's integer default rather than panicking in a result path.
    pub(crate) fn pending_all(
        submission: Box<dyn Pending + 'backend>,
        ids: Vec<ResidentId>,
        count: usize,
        classes: &[ScalarClass],
    ) -> Vec<Self> {
        let shared: Shared<'backend> = Arc::new(Mutex::new(Some(submission)));
        ids.into_iter()
            .enumerate()
            .map(|(ordinal, id)| Value::Pending {
                submission: Arc::clone(&shared),
                id,
                count,
                class: classes.get(ordinal).copied().unwrap_or(ScalarClass::Int),
            })
            .collect()
    }

    /// The outputs of one run that was already waited for.
    pub(crate) fn device_all(
        ids: Vec<ResidentId>,
        count: usize,
        classes: &[ScalarClass],
    ) -> Vec<Self> {
        ids.into_iter()
            .enumerate()
            .map(|(ordinal, id)| Value::Device {
                id,
                count,
                class: classes.get(ordinal).copied().unwrap_or(ScalarClass::Int),
            })
            .collect()
    }

    /// The buffer this names, for a dispatch that only records against it.
    ///
    /// # Invariant
    /// A pending value is a buffer: a dispatch against it names where the data will be,
    /// and the producing submission was recorded first. A number is refused rather than
    /// borrowed — a one-element host vector would run a kernel nobody wrote. The failure
    /// is this value's kind, not a refusal: a `GraphRefusal` here could name no node.
    pub fn slot(&self) -> Result<BufferSlot<'_>, &'static str> {
        match self {
            Value::Host { data, .. } => Ok(BufferSlot::Host(data)),
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(BufferSlot::Resident(*id)),
            Value::Int(_) => Err(self.state()),
        }
    }

    /// The elements of a host value, one word each, or `None` for anything else.
    ///
    /// # Invariant
    /// A device value has no elements on this side, so this is the host half of the split
    /// [`Self::slot`] makes, and it decodes at the value's own class.
    pub fn elements(&self) -> Option<Vec<i64>> {
        match self {
            Value::Host { data, class } => Some(elements(*class, data)),
            _ => None,
        }
    }

    /// The number this holds, for a dispatch's count.
    ///
    /// # Invariant
    /// The mirror of [`Self::slot`], and separate for the same reason: a caller told only
    /// "wrong shape" has to work out which role they hit. A count is an extent, so
    /// refusing a pointer is about the value, not the role. The number comes back as `i64`
    /// and the extent is the caller's to decide — negative counts are reported, not wrapped.
    pub fn as_number(&self) -> Result<i64, &'static str> {
        match self {
            Value::Int(number) => Ok(*number),
            other => Err(other.state()),
        }
    }

    /// How many elements this holds, or `None` for a number.
    ///
    /// # Invariant
    /// `None` rather than `0`, because a number has no length and a `0` would be a length
    /// a caller could act on. A host value's length is its payload over its class's width.
    pub fn count(&self) -> Option<usize> {
        match self {
            Value::Host { data, class } => Some(data.len() / class.byte_width()),
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
    /// # Invariant
    /// The one demand point, and the only way a pending value becomes readable; waiting a
    /// submission shared by several values happens once.
    pub fn settle(&mut self) -> Result<(), GraphRefusal> {
        let Value::Pending {
            submission,
            id,
            count,
            class,
        } = self
        else {
            return Ok(());
        };
        let (id, count, class) = (*id, *count, *class);
        // Taken under the lock, so two values of one node cannot both wait.

        // The lock is not held across the wait: another thread must be able to settle a
        // different submission.
        let waiting = submission.lock().unwrap().take();
        if let Some(waiting) = waiting {
            waiting.wait().map_err(|reason| GraphRefusal::Backend {
                what: "waiting for a submission",
                reason,
            })?;
        }
        *self = Value::Device { id, count, class };
        Ok(())
    }

    /// What this value currently is, which is what a role it is not in answers with.
    ///
    /// # Invariant
    /// Readiness and kind are two different facts: "a submission not yet waited for" and
    /// "a device buffer that was waited for" are the same kind with different fixes, so a
    /// refusal naming only the kind would hand a caller the wrong one.
    fn state(&self) -> &'static str {
        match self {
            Value::Host { .. } => "host data",
            Value::Device { .. } => "a device buffer that was waited for",
            Value::Pending { .. } => "a submission that has not been waited for",
            Value::Int(_) => "a number",
        }
    }
}

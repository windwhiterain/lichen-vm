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
            // A bf16 is the top half of the float it rounds to, so the word keeps its high twice.
            ScalarClass::BFloat16 => {
                bytes.extend_from_slice(&((*word as u32) >> 16).to_le_bytes()[..2])
            }
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
            // The half is the float's high bits, so the word is its pattern lifted back into place.
            ScalarClass::BFloat16 => {
                i64::from(u16::from_le_bytes(element.try_into().unwrap_or_default()) as u32) << 16
            }
        })
        .collect()
}

/// Why a value could not be read as the one word a launch's scalar leaf holds.
///
/// # Invariant
/// The two are separate arms because they are different mistakes: a value that is not a
/// number at all is a role nobody asked it for, while a number of the other class is a
/// re-reading of bits this crate refuses to perform, and the two have different repairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitsRefusal {
    /// Not a number; `found` is what the value is.
    NotANumber(&'static str),
    /// A number, of the class named rather than the one asked for.
    Class(ScalarClass),
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
    /// Shared, so a caller holding a weight hands it over without a copy.
    Host {
        data: std::sync::Arc<[u8]>,
        class: ScalarClass,
    },
    /// A number.
    ///
    /// # Invariant
    /// `i64` rather than `usize`, because a count can be negative and that is a mistake to
    /// be named rather than wrapped into an enormous extent. A count is always an `Int`:
    /// an extent has no class to carry.
    Int(i64),
    /// A machine float, a number of the class [`ScalarClass::Float`].
    ///
    /// # Invariant
    /// Its own arm rather than a tagged [`Self::Int`], because the class travels with the
    /// value and that is what keeps a launch from re-reading one class's bits as the
    /// other's. [`Self::bits_at`] is where the two are held apart, and an extent still
    /// refuses it: an extent has no class to carry.
    Float(f32),
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
            Value::Float(number) => write!(f, "Float({number})"),
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
            data: pack(ScalarClass::Int, &data).into(),
            class: ScalarClass::Int,
        }
    }

    /// Host data of a class the caller names, as that class's packed bytes.
    pub fn host_data(class: ScalarClass, data: Vec<u8>) -> Self {
        Value::Host {
            data: data.into(),
            class,
        }
    }

    /// The same, sharing bytes the caller keeps: a weight handed over here is not copied.
    pub fn host_shared(class: ScalarClass, data: std::sync::Arc<[u8]>) -> Self {
        Value::Host { data, class }
    }

    /// A number, for a count edge or a graph's own argument.
    pub fn int(number: i64) -> Self {
        Value::Int(number)
    }

    /// A float, for a launch value or a graph's own argument.
    pub fn float(number: f32) -> Self {
        Value::Float(number)
    }

    /// The class at which this value's word is read, or `None` for an integer.
    ///
    /// # Invariant
    /// A buffer answers the class its elements are packed at and a float answers its own;
    /// an integer answers `None` because it is the ABI's default width, not because it is a
    /// role — a demand is [`Self::slot`], [`Self::device_buffer`] or [`Self::bits_at`].
    pub fn class(&self) -> Option<ScalarClass> {
        match self {
            Value::Host { class, .. }
            | Value::Device { class, .. }
            | Value::Pending { class, .. } => Some(*class),
            Value::Float(_) => Some(ScalarClass::Float),
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
    /// borrowed — a one-element host vector would run a kernel nobody wrote, and a float
    /// would be the same mistake in the other class. The failure is this value's kind, not
    /// a refusal: a `GraphRefusal` here could name no node.
    pub fn slot(&self) -> Result<BufferSlot<'_>, &'static str> {
        match self {
            Value::Host { data, .. } => Ok(BufferSlot::Host(data)),
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(BufferSlot::Resident(*id)),
            Value::Int(_) | Value::Float(_) => Err(self.state()),
        }
    }

    /// The device buffer this names, for an extent only the device holds.
    ///
    /// # Invariant
    /// The mirror of [`Self::slot`] and separate for the same reason: the two disagree about
    /// what a caller is asking — one wants the data, the other wants the address the device
    /// will count over. `Err` names what this value is, so a refused role is a refusal the
    /// caller can read.
    pub fn device_buffer(&self) -> Result<ResidentId, &'static str> {
        match self {
            Value::Device { id, .. } | Value::Pending { id, .. } => Ok(*id),
            Value::Host { .. } | Value::Int(_) | Value::Float(_) => Err(self.state()),
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
    /// refusing a pointer is about the value, not the role, and a float is refused too — an
    /// extent has no class to carry. The number comes back as `i64`, negative counts
    /// reported rather than wrapped.
    pub fn as_number(&self) -> Result<i64, &'static str> {
        match self {
            Value::Int(number) => Ok(*number),
            other => Err(other.state()),
        }
    }

    /// The one word this value gives a launch's scalar leaf, read at `class`.
    ///
    /// # Invariant
    /// The class is the **value's own**, never the leaf's: an integer read at
    /// [`ScalarClass::Float`] and a float read at [`ScalarClass::Int`] are each the other's
    /// bits, and a launch reads a leaf at the class its parameter declared, so each is
    /// refused by name rather than answered.
    pub fn bits_at(&self, class: ScalarClass) -> Result<i64, BitsRefusal> {
        match (self, class) {
            (Value::Int(number), ScalarClass::Int) => Ok(*number),
            (Value::Float(number), ScalarClass::Float) => Ok(number.to_bits() as i64),
            (Value::Int(_), ScalarClass::Float) => Err(BitsRefusal::Class(ScalarClass::Int)),
            (Value::Float(_), ScalarClass::Int) => Err(BitsRefusal::Class(ScalarClass::Float)),
            _ => Err(BitsRefusal::NotANumber(self.state())),
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
            Value::Int(_) | Value::Float(_) => None,
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
    pub(crate) fn state(&self) -> &'static str {
        match self {
            Value::Host { .. } => "host data",
            Value::Device { .. } => "a device buffer that was waited for",
            Value::Pending { .. } => "a submission that has not been waited for",
            Value::Int(_) => "a number",
            Value::Float(_) => "a float",
        }
    }
}

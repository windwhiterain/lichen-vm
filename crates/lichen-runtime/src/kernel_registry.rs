//! The kernel registry: compiled fragments, and the claims they were compiled for.
//! See docs/notes/graph-fusion.md §3.
//!
//! # Invariant
//! Every key here is a [`KernelId`] and every value is IR, so a backend that wants a kernel's
//! body reaches it without a `Module`. What **fills** it is the compiler's: a caller registers
//! the fragment a body compiled to, and never this crate's business again.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use lichen_kernel_ir::{KernelClaim, KernelFragment, KernelId, fragment_digest};

/// The process kernel registry: compiled fragments and the claims they were compiled
/// for, keyed by [`KernelId`].
///
/// # Invariant
/// Kernels are immutable artifacts shared across modules; the fragment is the
/// durable JIT output and the module bytes are derived on demand at launch. The claim is
/// **read at compile time and stored here**, never on the kernel value: a runtime kernel is
/// a `Copy` id, so a claim that rode it would be a second, mutable copy of what the
/// registry already knows.
static KERNELS: OnceLock<Mutex<HashMap<KernelId, KernelEntry>>> = OnceLock::new();

/// The registry itself, for a caller that reads several entries under one lock.
pub fn kernels() -> &'static Mutex<HashMap<KernelId, KernelEntry>> {
    KERNELS.get_or_init(Default::default)
}

/// One registered kernel: its compiled fragment, and the interface instance it was
/// compiled for.
///
/// # Invariant
/// The two are interned **together**: they are one kernel, so two kernels that differ only
/// in the interface they declare are two entries and never share one id. The claim is
/// `None` for a kernel no claim named, which is every kernel in an ordinary program.
#[derive(Debug, Clone, PartialEq)]
pub struct KernelEntry {
    /// What the kernel's body compiled to.
    pub fragment: KernelFragment,
    /// The interface the kernel claims, or `None`.
    pub claim: Option<KernelClaim>,
}

/// The fallback id allocator — monotone, so an id minted here never aliases.
static NEXT_KERNEL_ID: AtomicUsize = AtomicUsize::new(0);

/// Mint the next id no other kernel holds.
fn alloc_kernel_id() -> KernelId {
    NEXT_KERNEL_ID.fetch_add(1, Ordering::Relaxed)
}

/// The content index: an entry's digest → the id it is registered under.
///
/// # Invariant
/// Ids are content-addressed (`D15`): the same source recompiled yields the same
/// id, which the derived-module cache is keyed on. A digest is not an identity —
/// the index is an intern table, [`intern_kernel`] verifies the entry, and a
/// collision falls back to a unique id, costing a recompile and never a wrong
/// entry.
static KERNEL_INDEX: OnceLock<Mutex<HashMap<u64, KernelId>>> = OnceLock::new();

/// The content index itself.
fn kernel_index() -> &'static Mutex<HashMap<u64, KernelId>> {
    KERNEL_INDEX.get_or_init(Default::default)
}

/// The digest an entry is interned under: the fragment's own, folded with the claim.
///
/// # Invariant
/// The claim is **not** a fragment field, so the fragment's digest says what the body is
/// while this says what the kernel is; folding the two together is what keeps two kernels
/// with the same body and different claims from sharing an id.
fn entry_digest(entry: &KernelEntry) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    fragment_digest(&entry.fragment).hash(&mut hasher);
    format!("{:?}", entry.claim).hash(&mut hasher);
    hasher.finish()
}

/// Register `fragment` under the interface it `claim`s, and return its id.
///
/// # Invariant
/// The lookup and the insert are one decision, so both registries are held
/// across it: otherwise a concurrent intern of the same digest can land its own
/// id after this caller read the entry, handing out a second id for one
/// kernel. Lock order is `kernels()` before `kernel_index()`.
pub fn intern_kernel(fragment: KernelFragment, claim: Option<KernelClaim>) -> KernelId {
    let entry = KernelEntry { fragment, claim };
    let digest = entry_digest(&entry);
    // Lock order: `kernels()` then `kernel_index()`, as the doc says.
    let mut registry = kernels().lock().unwrap();
    let mut index = kernel_index().lock().unwrap();
    if let Some(id) = index.get(&digest).copied()
        && registry.get(&id) == Some(&entry)
    {
        return id;
    }
    let id = alloc_kernel_id();
    registry.insert(id, entry);
    index.insert(digest, id);
    id
}

/// The fragment a registered kernel names, or `None` when no kernel is registered under it.
///
/// # Invariant
/// The registry is the compiler's own and the fragment is immutable, so this is the read a
/// host that holds a kernel *value* — not the artifact `jit` built from it — needs to reach
/// what the kernel compiled to. See docs/notes/graph-fusion.md §3.
pub fn kernel_fragment(id: KernelId) -> Option<KernelFragment> {
    kernels()
        .lock()
        .unwrap()
        .get(&id)
        .map(|entry| entry.fragment.clone())
}

/// The interface instance a registered kernel claims, or `None` when it claims none.
///
/// # Invariant
/// Read from the registry beside the fragment, so a host never reads a claim off a runtime
/// value: the claim was read once, when the kernel was compiled.
pub fn kernel_claim(id: KernelId) -> Option<KernelClaim> {
    kernels().lock().unwrap().get(&id)?.claim.clone()
}

/// Every registered kernel that claims an interface, as its id, claim and fragment.
///
/// # Invariant
/// The registry is append-only — an entry is interned once and never removed — so this answers
/// the same set on every call, and a host that reads it needs no kernel *value* to reach a
/// kernel's claim. A kernel no claim named is absent, because a claim is what the join names.
pub fn claimed_kernels() -> Vec<(KernelId, KernelClaim, KernelFragment)> {
    let registry = kernels().lock().unwrap();
    let mut claimed: Vec<(KernelId, KernelClaim, KernelFragment)> = registry
        .iter()
        .filter_map(|(id, entry)| {
            let claim = entry.claim.clone()?;
            Some((*id, claim, entry.fragment.clone()))
        })
        .collect();
    // In id order, because the map iterates in no order and a report read twice must read
    // the same.
    claimed.sort_by_key(|(id, _, _)| *id);
    claimed
}

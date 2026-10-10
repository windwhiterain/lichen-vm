//! The device `ModuleKey`: a compact index into the shared device registry.

/// The device key naming a compiled module in the shared registry.
///
/// # Invariant
///
/// A monotonically increasing index, so a module has the same key in every process sharing the
/// registry. A key is reclaimed only when its entry is removed and no surviving entry names it,
/// so an allocation that never published (a crashed `alloc`, or a `virtual:` source) holds its
/// key for the registry's life: the space is bounded by the file IDs it has held.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleKey(u64);

impl ModuleKey {
    /// The key's compact index value.
    pub const fn as_raw(self) -> u64 {
        self.0
    }
    /// Build a key from its compact index value — the device registry's
    /// allocation unit.
    pub const fn from_raw(index: u64) -> Self {
        ModuleKey(index)
    }
}

impl std::fmt::Debug for ModuleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ModuleKey({})", self.0)
    }
}

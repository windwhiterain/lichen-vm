//! The device `ModuleKey`: a compact index into the shared device registry.

/// The device key naming a compiled module in the lowlevel runtime registry
/// (the disk shape is `crate::device::DeviceRegistry`).  A monotonically
/// increasing index allocated by the device registry (the persistent store
/// that maps keys to artifact content hashes), so the same module has the same
/// key in every process sharing the registry — keys are stable across
/// processes and are reclaimed (reused) when a module is removed from the
/// registry, so the key space stays bounded.  Refs (node, function, handle)
/// carry the key of their home module, so refs are absolute from birth: an
/// importer stores them verbatim and resolves the key through the shared
/// registry — no per-importer retarget, no re-based copies, and the same
/// payload is shared by every importer.
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

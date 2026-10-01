//! Retained cells: a `cache`d binding's value, kept across a build.
//!
//! The design is `docs/notes/incremental-update.md`.  A cell is identified by its
//! **occurrence path** in the AST (`lichen_language_parser::path`) — never by a
//! content hash, which is the requirement the whole design serves — and what is
//! kept is a **frozen** reference: the artifact lives in a shared registry, so a
//! later build lowers the binding to `ExprKind::Static` and reads the value in
//! place instead of lowering, checking and evaluating its body.
//!
//! The store is deliberately small: it holds references, not values.  Who owns an
//! artifact and when it is evicted is the registry's business, and releasing what
//! an artifact owns is the artifact's own `Drop` (see
//! `lichen_lowlevel::Release`).

use std::collections::HashMap;

use lichen_language_parser::path::Path;
use lichen_lowlevel::{ModuleKey, StaticNodeId};

/// The retained cells of one compilation context, keyed by occurrence path.
#[derive(Default)]
pub struct CellStore {
    cells: HashMap<Path, StaticNodeId>,
    next_key: u64,
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The frozen value of a cell, or `None` when the binding must be compiled.
    ///
    /// A `None` is the honest answer for both "never retained" and "dirty": the
    /// caller decides dirtiness (the edit names the file it changed), and a cell
    /// whose file is dirty is simply not consulted.
    pub fn reference(&self, path: &Path) -> Option<StaticNodeId> {
        self.cells.get(path).copied()
    }

    /// Record a freshly frozen cell under its path.
    pub fn record(&mut self, path: Path, reference: StaticNodeId) {
        self.cells.insert(path, reference);
    }

    /// A device key for the next artifact.
    ///
    /// A **counter**, because a cell's identity is its path: the key only has to
    /// be distinct, and nothing may be derived from content (the requirement) or
    /// reused across artifacts (the registry asserts a key is filed once).
    pub fn allocate_key(&mut self) -> ModuleKey {
        self.next_key += 1;
        ModuleKey::from_raw(self.next_key)
    }

    /// How many cells are retained.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

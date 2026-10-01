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

/// The name of the source a cell came from — a file identity the caller names
/// (a path, a URI), **never** anything derived from content: the whole design
/// serves a requirement that no key decides reuse, and a cell's identity is its
/// path plus this name.
pub type SourceId = String;

/// One retained cell.
struct Cell {
    reference: StaticNodeId,
    /// The source that produced it — what [`CellStore::invalidate`] drops.
    source: SourceId,
}

/// The retained cells of one compilation context, keyed by occurrence path.
#[derive(Default)]
pub struct CellStore {
    cells: HashMap<Path, Cell>,
    next_key: u64,
}

impl CellStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// The frozen value of a cell, or `None` when the binding must be compiled.
    ///
    /// A `None` is the honest answer for both "never retained" and "dirty": the
    /// caller decides dirtiness (the edit names the file it changed) and drops a
    /// dirty source's cells with [`Self::invalidate`], so this only ever answers
    /// for a cell the caller still believes in.
    pub fn reference(&self, path: &Path) -> Option<StaticNodeId> {
        self.cells.get(path).map(|cell| cell.reference)
    }

    /// Record a freshly frozen cell under its path and the source that produced
    /// it.
    pub fn record(&mut self, path: Path, source: &str, reference: StaticNodeId) {
        self.cells.insert(
            path,
            Cell {
                reference,
                source: source.to_string(),
            },
        );
    }

    /// Drop every cell that came from `source`, returning the artifacts they
    /// named.
    ///
    /// This is the **dirty input**, and it is deliberately the caller's: an edit
    /// names the file it changed, so the store never has to detect anything — and
    /// the caller drops the cells of every file that *imports* the changed one too
    /// (the reverse import closure), which is the file granularity the design chose
    /// for source edits.  A dropped cell is not consulted again, so its binding is
    /// compiled and re-frozen on the next build.
    ///
    /// The returned keys are the artifacts that just became unreachable.  The store
    /// does **not** evict them: a static ref is a raw handle into the artifact's
    /// arena, so eviction is sound only once every module that could still hold one
    /// is gone ([`Registry::evict`]'s precondition), and only the caller knows that.
    pub fn invalidate(&mut self, source: &str) -> Vec<ModuleKey> {
        let mut dropped = Vec::new();
        self.cells.retain(|_, cell| {
            let keep = cell.source != source;
            if !keep {
                dropped.push(cell.reference.module);
            }
            keep
        });
        dropped
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

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
//! `lichen_lowlevel::Release`).  The key an artifact is filed under is the
//! **registry's** to allocate ([`lichen_lowlevel::Registry::allocate_cell_key`])
//! — several sessions share one registry — in a key space disjoint from the
//! device's, so a cell and the imported packages its closure reads are filed side
//! by side.

use std::collections::{HashMap, HashSet};

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
    /// This is the **coarse** dirty input, and it is deliberately the caller's:
    /// an edit names the file it changed, so the store never has to detect
    /// anything — and the caller drops the cells of every file that *imports* the
    /// changed one too (the reverse import closure), which is the file
    /// granularity the design chose for source edits.  A dropped cell is not
    /// consulted again, so its binding is compiled and re-frozen on the next
    /// build.
    ///
    /// [`Self::invalidate_paths`] is the fine cut under it: an edit to a file
    /// this store *does* hold cells for need not drop them all — dirty
    /// propagation names the positions it reached (`crate::dirty`).
    ///
    /// The returned keys are the artifacts that just became unreachable.  The store
    /// does **not** evict them: a static ref is a raw handle into the artifact's
    /// arena, so eviction is sound only once every module that could still hold one
    /// is gone ([`Registry::evict`]'s precondition), and only the caller knows that.
    pub fn invalidate_source(&mut self, source: &str) -> Vec<ModuleKey> {
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

    /// Drop the cells at `paths` — the positions an edit dirtied.
    ///
    /// The **fine cut**: a cell is dropped because dirty propagation reached its
    /// identity, not because its file was touched, so an edit that leaves a
    /// marked binding's inputs alone keeps its artifact.  The returned keys are
    /// what became unreachable, and the store still does not evict them.
    pub fn invalidate_paths(&mut self, paths: &HashSet<Path>) -> Vec<ModuleKey> {
        let mut dropped = Vec::new();
        self.cells.retain(|path, cell| {
            let keep = !paths.contains(path);
            if !keep {
                dropped.push(cell.reference.module);
            }
            keep
        });
        dropped
    }

    /// Drop every cell whose path is not in `marked` — the positions the program
    /// about to be built marks.
    ///
    /// A cell outliving its mark would be read for a binding that no longer asks
    /// to be retained, and would never be compiled again: the mark is the whole
    /// of a cell's promise, so it is also the whole of its warrant.  A position
    /// that moved (a rename) falls out here too, which is what keeps a store
    /// keyed by identity from accumulating paths nothing resolves.
    pub fn retain_marked(&mut self, marked: &HashSet<Path>) -> Vec<ModuleKey> {
        let mut dropped = Vec::new();
        self.cells.retain(|path, cell| {
            let keep = marked.contains(path);
            if !keep {
                dropped.push(cell.reference.module);
            }
            keep
        });
        dropped
    }

    /// How many cells are retained.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

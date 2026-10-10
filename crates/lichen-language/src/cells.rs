//! Retained cells: a `cache`d binding's value, kept across a build.
//!
//! # Invariant
//!
//! A cell is identified by its occurrence path in the AST, never by a content
//! key, and a retained cell holds a frozen reference, not a value.
//! See docs/notes/incremental-update.md §3.

use std::collections::{HashMap, HashSet};

use lichen_language_parser::path::Path;
use lichen_lowlevel::{ModuleKey, StaticNodeId};

/// The name of the source a cell came from — a caller-named file identity.
///
/// # Invariant
///
/// Never derived from content: a cell's identity is its path plus this name.
pub type SourceId = String;

/// One retained cell.
struct Cell {
    reference: StaticNodeId,
    /// The source that produced it — what [`CellStore::invalidate_source`] drops.
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
    /// # Invariant
    ///
    /// Answers only for a cell the caller still believes in: the caller drops a
    /// dirty source's cells with [`Self::invalidate_source`].
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

    /// Drop every cell that came from `source`, returning the artifacts they named.
    ///
    /// # Invariant
    ///
    /// The caller owns this coarse cut, because an edit names the file it changed.
    /// The store never evicts: a static ref is a raw handle, so eviction is sound
    /// only once no module can still hold one.
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
    /// # Invariant
    ///
    /// The fine cut: a cell drops because propagation reached its identity, not
    /// because its file was touched.
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
    /// # Invariant
    ///
    /// A cell outliving its mark would never be compiled again, and a position
    /// that moved falls out here too.
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

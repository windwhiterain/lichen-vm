//! The packs a command line names, read before any program is compiled.
//! See docs/notes/graph-fusion.md §5.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use lichen_language::placement_pack::{PackRefusal, PlacementPack, read_placement_pack};

/// Why a named pack could not be installed.
///
/// # Invariant
/// The path is named beside the pack's own refusal, so a run that stops says **which file** and
/// what about it: a pack is a text a human wrote, and a refusal that named neither leaves nothing
/// to fix.
#[derive(Debug)]
pub enum PackLoadRefusal {
    /// The path is not a file this process could read.
    Unreadable {
        /// The path the command line named.
        path: PathBuf,
        /// What the filesystem said.
        source: io::Error,
    },
    /// The text is not the pack it names.
    Refused {
        /// The path the command line named.
        path: PathBuf,
        /// The reader's own refusal, carried whole.
        refusal: PackRefusal,
    },
}

impl fmt::Display for PackLoadRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, source } => write!(
                formatter,
                "cannot read the placement pack `{}`: {source}",
                path.display()
            ),
            Self::Refused { path, refusal } => write!(
                formatter,
                "cannot use the placement pack `{}`: {refusal}",
                path.display()
            ),
        }
    }
}

/// Read every path as the pack it names, in the order the command line named them.
///
/// # Invariant
/// **All or nothing**: the first pack that cannot be read, or is not one, ends the read, so a run
/// never installs some of the packs it was handed. An **inert** pack is not a refusal here: a pack
/// that matches no node is well formed, and one that names nothing at all is the reader's own
/// `EmptyPlan`.
pub fn load_packs(paths: &[PathBuf]) -> Result<Vec<PlacementPack>, PackLoadRefusal> {
    paths.iter().map(|path| load_one(path)).collect()
}

/// Read one path as a pack, or name why it is not one.
fn load_one(path: &Path) -> Result<PlacementPack, PackLoadRefusal> {
    let text = std::fs::read_to_string(path).map_err(|source| PackLoadRefusal::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    read_placement_pack(&text).map_err(|refusal| PackLoadRefusal::Refused {
        path: path.to_path_buf(),
        refusal,
    })
}

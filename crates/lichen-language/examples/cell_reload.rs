//! Temporary: a session with an empty registry reuses cells a previous one stored.
//! See heavy-value-cache.md §9.13.

use std::sync::{Arc, RwLock};

use lichen_language::program::LangProgram;
use lichen_language::session::BufferSession;
use lichen_lowlevel::{AnyNodeId, Registry};

const SOURCE: &str = "\
cache a = 1 + 2
cache b = a + 3
a + b
";
const DIR: &str = "target/cell-cache-probe";

fn root(report: &lichen_language::session::SessionReport<LangProgram>) -> Option<String> {
    let build = report.build.as_ref()?;
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))?;
    Some(format!("{value:?}"))
}

fn session(source: &str) -> BufferSession<LangProgram> {
    let registry = Arc::new(RwLock::new(Registry::<LangProgram>::new()));
    BufferSession::<LangProgram>::with_registry(source, "cell.lichen", registry)
        .with_cell_cache(DIR)
}

fn main() {
    let _ = std::fs::remove_dir_all(DIR);

    let first = session(SOURCE).compile();
    println!(
        "first  (cold): frozen {}, reused {}, root {:?}",
        first.cells.frozen,
        first.cells.reused,
        root(&first)
    );

    let second = session(SOURCE).compile();
    println!(
        "second (fresh registry): frozen {}, reused {}, dropped {}, root {:?}",
        second.cells.frozen,
        second.cells.reused,
        second.cells.dropped,
        root(&second)
    );

    // An edit to `a`'s constant: both cells' keys move, so nothing may be reused.
    let edited = SOURCE.replace("1 + 2", "1 + 4");
    let mut third = session(&edited);
    third.set_source(&edited);
    let report = third.compile();
    println!(
        "third  (edited): frozen {}, reused {}, dropped {}, root {:?}",
        report.cells.frozen,
        report.cells.reused,
        report.cells.dropped,
        root(&report)
    );

    // And a fourth over the edited source: the third stored the new cells.
    let fourth = session(&edited).compile();
    println!(
        "fourth (fresh registry, edited): frozen {}, reused {}, root {:?}",
        fourth.cells.frozen,
        fourth.cells.reused,
        root(&fourth)
    );
    println!("cache files: {} (two cells)", files());

    // A rename moves `a`'s path, so the cell is dropped and its file must be reclaimed.
    let renamed = edited
        .replace("cache a =", "cache a2 =")
        .replace("a + 3", "a2 + 3")
        .replace("a + b", "a2 + b");
    let report = session(&renamed).compile();
    println!(
        "renamed: frozen {}, reused {}, root {:?}; cache files: {}",
        report.cells.frozen,
        report.cells.reused,
        root(&report),
        files()
    );
}

/// The cell files under the cache directory, counted through its one source folder.
fn files() -> usize {
    let Ok(entries) = std::fs::read_dir(DIR) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .map(|inner| inner.flatten().count())
        .sum()
}

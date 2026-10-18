//! Temporary: the world generator's edit loop against fresh compiles (§9.17).
//!
//! # Invariant
//!
//! Every row compares the session's whole collected map — relief, biomes and tiles,
//! element for element — with the same source's fresh compile, so a fast row that
//! answered differently is reported rather than counted as a win.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use lichen_compute::language::{LangProgram, new_store};
use lichen_language::package::PackageStore;
use lichen_language::preprocess::preprocess;
use lichen_language::session::{BufferSession, SessionReport};
use lichen_lowlevel::AnyNodeId;

/// The application this probe drives: the example program, not a copy of it.
const APP: &str = "worldgen.lichen";

/// The cache directory a restart reads back.
const CELLS: &str = "target/worldgen-cells";

/// The grid the application is driven at: the example's own, or `SIZE` for a heavier run.
fn grid() -> usize {
    std::env::var("SIZE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(128)
}

fn app_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(APP)
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// The application's whole map, in place of the example's sampled digest.
///
/// # Invariant
///
/// The last line of the example is its printed digest; replacing only that line keeps
/// every cached binding the example declares and compares what the map holds.
fn whole_map(source: &str) -> String {
    let head = source.trim_end().rsplit_once('\n').map(|(head, _)| head);
    let head = head.expect("the application ends with its digest expression");
    if std::env::var("WHOLE_CELLS").is_ok() {
        return format!("{head}\n(relief, biomes, tiles)\n");
    }
    let elements = grid() * grid();
    format!(
        "{head}\n(compute.collect relief : array{{Float, {elements}}}, \
         compute.collect biomes : array{{Int, {elements}}}, \
         compute.collect tiles : array{{Int, {elements}}})\n"
    )
}

fn fnv(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn session_value(report: &SessionReport<LangProgram>) -> Option<String> {
    let build = report.build.as_ref()?;
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))?;
    Some(lichen_language::render::print_value_lang(
        &build.module,
        value,
        build.root_ty,
    ))
}

struct Row {
    label: String,
    fresh_ms: f64,
    session_ms: f64,
    reused: usize,
    frozen: usize,
    dropped: usize,
    same_value: bool,
    digest: u64,
}

/// Preprocess `source` against `store`, answering the view its session is handed.
fn prepare(
    store: &mut PackageStore<LangProgram>,
    source: &str,
) -> (
    String,
    u32,
    Vec<lichen_language::preprocess::ResolvedImport>,
) {
    let (preprocessed, diagnostics) = preprocess(source, None, store);
    if !diagnostics.is_empty() {
        println!("  preprocess diagnostics: {diagnostics:?}");
    }
    (
        preprocessed.code.to_string(),
        preprocessed.code_base,
        preprocessed.imports,
    )
}

fn main() {
    let base = std::fs::read_to_string(app_path()).expect("the application is in the tree");
    let base = base.replace("size = 128", &format!("size = {}", grid()));
    let mut store = new_store(None).expect("the embedded compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "worldgen.lichen", store.registry());

    let first_source = whole_map(&base);
    let (code, offset, imports) = prepare(&mut store, &first_source);
    let starts = lichen_language::lex::line_starts(&first_source);
    session.set_view(code, offset, &starts, &imports);
    let start = Instant::now();
    let first = session.compile();
    println!(
        "== worldgen: {} elements per map, first compile {:.2} ms, frozen {}, ok {} ==",
        grid() * grid(),
        ms(start.elapsed()),
        first.cells.frozen,
        first.build.as_ref().is_some_and(|build| build.ok),
    );
    for diagnostic in first.diagnostics.iter().take(6) {
        println!("  diagnostic: {diagnostic:?}");
    }

    let edits: Vec<(&str, fn(&str) -> String)> = vec![
        ("same source again", |source| source.to_string()),
        ("whitespace only", |source| {
            source.replace("cache octave_a", "cache  octave_a")
        }),
        ("biome sea 0.35 -> 0.40", |source| {
            source.replace(".sea 0.35", ".sea 0.40")
        }),
        ("octave scale 32 -> 40", |source| {
            source.replace(".scale 32", ".scale 40")
        }),
        ("mix weight 0.25 -> 0.35", |source| {
            source.replace(".w 0.25", ".w 0.35")
        }),
        ("moisture seed +3 -> +5", |source| {
            source.replace("(seed + 3)", "(seed + 5)")
        }),
        ("append a smoothing pass", |source| {
            let inserted = "cache weathered = (compute.plrun smooth ((ASmooth)(.n count, \
                            .cols size, .I InField(.f relief))) : OutField).f\ncache biomes";
            source
                .replace("cache biomes", inserted)
                .replace(".height relief", ".height weathered")
        }),
        ("seed 7 -> 11", |source| {
            source.replace("seed = 7", "seed = 11")
        }),
    ];

    let mut rows: Vec<Row> = Vec::new();
    let mut source = base;
    for (label, edit) in edits {
        source = edit(&source);
        rows.push(measure(
            &mut store,
            &mut session,
            &source,
            label.to_string(),
        ));
    }

    println!(
        "\n{:<22} {:>9} {:>10} {:>8} {:>7} {:>7} {:>8} {:>6} {:>18}",
        "edit",
        "fresh ms",
        "session ms",
        "speedup",
        "reused",
        "frozen",
        "dropped",
        "value",
        "map digest"
    );
    println!("{}", "-".repeat(104));
    for row in &rows {
        println!(
            "{:<22} {:>9.2} {:>10.2} {:>7.1}x {:>7} {:>7} {:>8} {:>6} {:>18}",
            row.label,
            row.fresh_ms,
            row.session_ms,
            row.fresh_ms / row.session_ms.max(0.001),
            row.reused,
            row.frozen,
            row.dropped,
            if row.same_value { "same" } else { "DIFF" },
            format!("{:016x}", row.digest),
        );
    }
    let fresh_total: f64 = rows.iter().map(|row| row.fresh_ms).sum();
    let session_total: f64 = rows.iter().map(|row| row.session_ms).sum();
    println!(
        "\n{} edits: fresh total {fresh_total:.2} ms, session total {session_total:.2} ms, \
         {:.1}x cheaper",
        rows.len(),
        fresh_total / session_total.max(0.001),
    );
    let disagreed = rows.iter().filter(|row| !row.same_value).count();
    println!(
        "value agreement: {} of {} edits answer exactly what a fresh compile answers",
        rows.len() - disagreed,
        rows.len(),
    );

    // A restart: an empty registry over the cache directory the session above stored.
    let settled = whole_map(&source);
    let _ = std::fs::remove_dir_all(CELLS);
    let start = Instant::now();
    let cold = restart(&settled);
    let cold_ms = ms(start.elapsed());
    let start = Instant::now();
    let warm = restart(&settled);
    let warm_ms = ms(start.elapsed());
    let start = Instant::now();
    let mut fresh_store = new_store(None).expect("the embedded compute package registers");
    let (code, offset, imports) = prepare(&mut fresh_store, &settled);
    let starts = lichen_language::lex::line_starts(&settled);
    let mut fresh =
        BufferSession::<LangProgram>::with_registry("", "fresh", fresh_store.registry());
    fresh.set_view(code, offset, &starts, &imports);
    let fresh_report = fresh.compile();
    let fresh_ms = ms(start.elapsed());
    let fresh_map = session_value(&fresh_report);
    println!(
        "\nrestart on the settled source: fresh {fresh_ms:.2} ms; cold {cold_ms:.2} ms \
         (frozen {}, reused {}); warm {warm_ms:.2} ms (frozen {}, reused {}); files {}",
        cold.2.frozen(),
        cold.2.reused(),
        warm.2.frozen(),
        warm.2.reused(),
        files(),
    );
    println!(
        "restart values: cold {}, warm {}, fresh {}",
        cold.0 == fresh_map,
        warm.0 == fresh_map,
        fresh_map.is_some(),
    );
}

/// A restart: a session over an empty registry whose cache directory is `CELLS`.
fn restart(source: &str) -> (Option<String>, f64, Report) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let mut session =
        BufferSession::<LangProgram>::with_registry("", "worldgen.lichen", store.registry())
            .with_cell_cache(CELLS);
    let (code, offset, imports) = prepare(&mut store, source);
    let starts = lichen_language::lex::line_starts(source);
    session.set_view(code, offset, &starts, &imports);
    let start = Instant::now();
    let report = session.compile();
    let elapsed = ms(start.elapsed());
    let value = session_value(&report);
    let cells = report.cells;
    (value, elapsed, Report(cells))
}

/// The three cell totals, kept apart from the report a session drops.
struct Report(lichen_language::session::CellEvents);

impl Report {
    fn frozen(&self) -> usize {
        self.0.frozen
    }

    fn reused(&self) -> usize {
        self.0.reused
    }
}

fn measure(
    store: &mut PackageStore<LangProgram>,
    session: &mut BufferSession<LangProgram>,
    source: &str,
    label: String,
) -> Row {
    let whole = whole_map(source);
    let (code, offset, imports) = prepare(store, &whole);
    let starts = lichen_language::lex::line_starts(&whole);

    // The fresh arm first, so neither arm runs against a state the other warmed.
    let start = Instant::now();
    let mut fresh = BufferSession::<LangProgram>::with_registry("", "fresh", store.registry());
    fresh.set_view(code.clone(), offset, &starts, &imports);
    let fresh_report = fresh.compile();
    let fresh_ms = ms(start.elapsed());
    let fresh_map = session_value(&fresh_report);

    session.set_view(code, offset, &starts, &imports);
    let start = Instant::now();
    let report = session.compile();
    let session_ms = ms(start.elapsed());
    let session_map = session_value(&report);

    let same_value = fresh_map.is_some() && fresh_map == session_map;
    if !same_value {
        for diagnostic in report.diagnostics.iter().take(4) {
            println!("  {label}: diagnostic {diagnostic:?}");
        }
        if let Some(build) = report.build.as_ref() {
            for entry in build.module.extension_diagnostics.iter().take(4) {
                println!("  {label}: extension {}: {}", entry.category, entry.message);
            }
        }
        let fresh_text = fresh_map.as_deref().unwrap_or("<none>");
        let session_text = session_map.as_deref().unwrap_or("<none>");
        let first = fresh_text
            .chars()
            .zip(session_text.chars())
            .position(|(left, right)| left != right);
        println!(
            "  {label}: fresh {} chars {:?} .. session {} chars {:?} .. first difference {:?}",
            fresh_text.len(),
            fresh_text.chars().take(60).collect::<String>(),
            session_text.len(),
            session_text.chars().take(60).collect::<String>(),
            first,
        );
    }
    Row {
        label,
        fresh_ms,
        session_ms,
        reused: report.cells.reused,
        frozen: report.cells.frozen,
        dropped: report.cells.dropped,
        same_value,
        digest: session_map.as_deref().map(fnv).unwrap_or(0),
    }
}

/// The cell files under the cache directory, counted through its one source folder.
fn files() -> usize {
    let Ok(entries) = std::fs::read_dir(CELLS) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| std::fs::read_dir(entry.path()).ok())
        .map(|inner| inner.flatten().count())
        .sum()
}

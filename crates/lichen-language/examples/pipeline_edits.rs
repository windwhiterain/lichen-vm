//! Temporary: a pipeline under an agent's edits, against fresh compiles.
//! See docs/notes/heavy-value-cache.md §9.
//!
//! # Invariant
//!
//! Every row compares the session's root value with the same source's fresh compile, so a
//! fast row that answers differently is reported rather than counted as a win.

use std::time::{Duration, Instant};

use lichen_language::compile;
use lichen_language::program::LangProgram;
use lichen_language::session::BufferSession;
use lichen_lowlevel::AnyNodeId;

/// A branching recurrence: cost is exponential in depth, the recursion stays shallow.
const STEP: &str = "step = x => [step (x - 1) + step (x - 2), x][x <= 1]";

/// The appended stage's depth, kept out of the edit sequence so only structure moves.
const APPEND_DEPTH: usize = 6;

/// A generator pipeline: each stage reads the one before it, and the tail reads all.
struct Pipeline {
    /// One depth per stage; an edit changes one entry.
    depths: Vec<usize>,
    /// Append one more stage before the tail.
    append: bool,
    /// An **unmarked** binding the tail reads, so an edit to it changes the build but no cell.
    bias: usize,
}

impl Pipeline {
    fn source(&self) -> String {
        let mut source = String::from(STEP);
        source.push('\n');
        for (i, &depth) in self.depths.iter().enumerate() {
            let upstream = match i {
                0 => String::new(),
                n => format!("s{} + ", n - 1),
            };
            source.push_str(&format!("@cache s{i} = {upstream}step {depth}\n"));
        }
        if self.append {
            let n = self.depths.len();
            source.push_str(&format!("@cache s{n} = s{} + step {APPEND_DEPTH}\n", n - 1));
        }
        source.push_str(&format!("bias = {}\n", self.bias));
        let mut terms: Vec<String> = (0..self.depths.len()).map(|i| format!("s{i}")).collect();
        if self.append {
            terms.push(format!("s{}", self.depths.len()));
        }
        terms.push("bias".to_string());
        source.push_str(&terms.join(" + "));
        source.push('\n');
        source
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// The root value a report carries, rendered for comparison. `None` when undecided.
fn fresh_value(report: &lichen_language::Report<LangProgram>) -> Option<String> {
    let build = report.build.as_ref()?;
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))?;
    Some(format!("{value:?}"))
}

fn session_value(report: &lichen_language::session::SessionReport<LangProgram>) -> Option<String> {
    let build = report.build.as_ref()?;
    let value = build
        .module
        .node_value(AnyNodeId::Dynamic(build.root_val))?;
    Some(format!("{value:?}"))
}

struct Row {
    label: String,
    fresh_ms: f64,
    session_ms: f64,
    reused: usize,
    frozen: usize,
    dropped: usize,
    same_value: bool,
}

fn main() {
    let stages = env_usize("STAGES", 6);
    let base = env_usize("BASE", 12);
    let mut pipeline = Pipeline {
        depths: (0..stages).map(|i| base.saturating_sub(i)).collect(),
        append: false,
        bias: 7,
    };
    let mut session =
        BufferSession::<LangProgram>::with_source_id(pipeline.source(), "pipeline.lichen");
    // `READ_ONLY=1` opts every stage's cell into the small freeze; no other row changes.
    let read_only = std::env::var("READ_ONLY").is_ok();
    if read_only {
        session.set_read_only((0..stages).map(|i| format!("s{i}")).collect::<Vec<_>>());
    }
    let start = Instant::now();
    let first = session.compile();
    println!(
        "== pipeline: {stages} stages, base depth {base}, freeze {}, first compile {:.2} ms, {} frozen, ok {}",
        if read_only { "ReadOnly" } else { "Portable" },
        ms(start.elapsed()),
        first.cells.frozen,
        first.build.as_ref().is_some_and(|b| b.ok),
    );

    // The fresh arm in every row is a fresh compile; this is its drift control.
    println!("== drift: one unchanged source compiled five times ==");
    let unchanged = pipeline.source();
    for round in 0..5 {
        let start = Instant::now();
        let _ = compile(&unchanged);
        println!("  round {round}: {:.2} ms", ms(start.elapsed()));
    }

    let mut rows: Vec<Row> = Vec::new();

    // The upstream-most stage: every later stage reads it, so the whole chain is dirty.
    pipeline.depths[0] = base + 1;
    rows.push(measure(
        &mut session,
        &pipeline.source(),
        "edit upstream s0".into(),
    ));

    // A middle stage: it and everything after it, and nothing before it.
    let middle = stages / 2;
    pipeline.depths[middle] = base;
    rows.push(measure(
        &mut session,
        &pipeline.source(),
        format!("edit middle s{middle}"),
    ));

    // The leaf stage: nothing downstream of it exists, so one cell moves.
    pipeline.depths[stages - 1] = base + 1;
    rows.push(measure(
        &mut session,
        &pipeline.source(),
        format!("edit leaf s{}", stages - 1),
    ));

    // A structural append: every retained stage is reused, one new cell is frozen.
    pipeline.append = true;
    rows.push(measure(
        &mut session,
        &pipeline.source(),
        "append a stage".into(),
    ));

    // Whitespace inside a cell *and* a real change to the unmarked binding: no cell moved.
    pipeline.bias = 8;
    let combined = pipeline.source().replace("@cache s0", "@cache  s0");
    rows.push(measure(
        &mut session,
        &combined,
        "combined: cell + bias".into(),
    ));

    // A whitespace-only edit: no resolved structure moves at all.
    let spaced = pipeline.source().replace("@cache s0", "@cache  s0");
    rows.push(measure(&mut session, &spaced, "whitespace only".into()));

    println!(
        "\n{:<20} {:>9} {:>10} {:>8} {:>7} {:>7} {:>8} {:>6}",
        "edit", "fresh ms", "session ms", "speedup", "reused", "frozen", "dropped", "value"
    );
    println!("{}", "-".repeat(84));
    for row in &rows {
        let speedup = row.fresh_ms / row.session_ms.max(0.001);
        println!(
            "{:<20} {:>9.2} {:>10.2} {:>7.1}x {:>7} {:>7} {:>8} {:>6}",
            row.label,
            row.fresh_ms,
            row.session_ms,
            speedup,
            row.reused,
            row.frozen,
            row.dropped,
            if row.same_value { "same" } else { "DIFF" },
        );
    }

    let fresh_total: f64 = rows.iter().map(|row| row.fresh_ms).sum();
    let session_total: f64 = rows.iter().map(|row| row.session_ms).sum();
    println!(
        "\nfive edits: fresh total {fresh_total:.2} ms, session total {session_total:.2} ms, {:.1}x cheaper",
        fresh_total / session_total.max(0.001),
    );
    let mismatched = rows.iter().filter(|row| !row.same_value).count();
    println!(
        "value agreement: {} of {} edits answered what a fresh compile answers",
        rows.len() - mismatched,
        rows.len(),
    );

    // Restart: a session with an empty registry over a cache the earlier one stored.
    let settled = pipeline.source();
    let dir = "target/pipeline-cells";
    let _ = std::fs::remove_dir_all(dir);
    let start = Instant::now();
    let cold = session_with_cache(&settled, dir).compile();
    let cold_ms = ms(start.elapsed());
    let start = Instant::now();
    let warm = session_with_cache(&settled, dir).compile();
    let warm_ms = ms(start.elapsed());
    let start = Instant::now();
    let loaded = session_with_loader(&settled, dir).compile();
    let loaded_ms = ms(start.elapsed());
    let start = Instant::now();
    let _ = compile(&settled);
    let fresh_ms = ms(start.elapsed());
    println!(
        "\nrestart on the same source: fresh {fresh_ms:.2} ms; cold {cold_ms:.2} ms (frozen {}); \
         warm {warm_ms:.2} ms (reused {}, frozen {}); load-only {loaded_ms:.2} ms (reused {})",
        cold.cells.frozen, warm.cells.reused, warm.cells.frozen, loaded.cells.reused,
    );
}

/// A session over an empty registry that also writes the cache back.
fn session_with_cache(source: &str, dir: &str) -> BufferSession<LangProgram> {
    use std::sync::{Arc, RwLock};

    let registry = Arc::new(RwLock::new(lichen_lowlevel::Registry::<LangProgram>::new()));
    BufferSession::<LangProgram>::with_registry(source, "pipeline.lichen", registry)
        .with_cell_cache(dir)
}

/// A session over an empty registry that only reads the cache.
fn session_with_loader(source: &str, dir: &str) -> BufferSession<LangProgram> {
    use std::sync::{Arc, RwLock};

    let registry = Arc::new(RwLock::new(lichen_lowlevel::Registry::<LangProgram>::new()));
    BufferSession::<LangProgram>::with_registry(source, "pipeline.lichen", registry)
        .with_cell_cache_loading(dir)
}

fn measure(session: &mut BufferSession<LangProgram>, source: &str, label: String) -> Row {
    // The fresh arm first, so neither arm runs against a state the other warmed.
    let start = Instant::now();
    let fresh = compile(source);
    let fresh_ms = ms(start.elapsed());

    session.set_source(source);
    let start = Instant::now();
    let report = session.compile();
    let session_ms = ms(start.elapsed());

    Row {
        label,
        fresh_ms,
        session_ms,
        reused: report.cells.reused,
        frozen: report.cells.frozen,
        dropped: report.cells.dropped,
        same_value: fresh_value(&fresh) == session_value(&report),
    }
}

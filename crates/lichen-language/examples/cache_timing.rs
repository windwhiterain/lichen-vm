//! Temporary: session rebuild against a fresh compile. Prints, asserts nothing.
//! See docs/notes/heavy-value-cache.md §9.
//!
//! # Invariant
//!
//! Every reading is wall clock from one process, and each row's fresh arm compiles
//! the same source the session arm then compiles, so the two are comparable per row.

use std::time::{Duration, Instant};

use lichen_language::program::LangProgram;
use lichen_language::session::BufferSession;
use lichen_language::{compile, frontend};

/// One naive `fib`; `[a, b][cond]` is the conditional, so one cell is measurable.
const HEAVY_FN: &str = "fib = x => [fib (x - 1) + fib (x - 2), x][x <= 1]";

/// The program a reading is taken on; every edit is a different `Shape`.
struct Shape {
    cheap: usize,
    heavy: usize,
    /// The depth every heavy cell uses except the first.
    depth: usize,
    /// The first heavy cell's own depth, so "one heavy node" is one node.
    h0_depth: usize,
    /// What `c0` adds: the cheap-cone edit changes this alone.
    seed: usize,
    /// Insert one more cached statement before the tail.
    extra: bool,
}

impl Shape {
    fn base(cheap: usize, heavy: usize, depth: usize) -> Self {
        Self {
            cheap,
            heavy,
            depth,
            h0_depth: depth,
            seed: 0,
            extra: false,
        }
    }

    fn source(&self) -> String {
        let mut source = String::from(HEAVY_FN);
        source.push('\n');
        for i in 0..self.cheap {
            if i == 0 {
                source.push_str(&format!("cache c0 = 1 + {}\n", self.seed));
            } else {
                source.push_str(&format!("cache c{i} = c{} + 1\n", i - 1));
            }
        }
        for i in 0..self.heavy {
            let cell_depth = if i == 0 { self.h0_depth } else { self.depth };
            source.push_str(&format!("cache h{i} = fib {cell_depth}\n"));
        }
        if self.extra {
            source.push_str("cache extra = fib 5\n");
        }
        // The tail reads every retained binding, so dropping any cell is observable.
        let mut terms: Vec<String> = Vec::new();
        if self.cheap > 0 {
            terms.push(format!("c{}", self.cheap - 1));
        }
        for i in 0..self.heavy {
            terms.push(format!("h{i}"));
        }
        if self.extra {
            terms.push("extra".into());
        }
        source.push_str(&terms.join(" + "));
        source.push('\n');
        source
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// A count from the environment, so a reading can be retaken without a rebuild.
fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

struct Reading {
    label: &'static str,
    fresh_ms: f64,
    session_ms: f64,
    reused: bool,
    cells_reused: usize,
    cells_frozen: usize,
    cells_dropped: usize,
    retained: usize,
}

fn main() {
    // 2 000 cumulative applications per build; `fib 10` is ~177, so this fits.
    let cheap = env_usize("CHEAP", 8);
    let heavy = env_usize("HEAVY", 4);
    let depth = env_usize("DEPTH", 10);

    println!("== base: {cheap} cheap cells, {heavy} heavy cells (fib {depth}+) ==");

    // Frontend is ~1.6 % of a build, so a key fold competes with almost nothing.
    let base = Shape::base(cheap, heavy, depth).source();
    let t0 = Instant::now();
    let front = frontend(&base);
    let frontend_ms = ms(t0.elapsed());
    let t1 = Instant::now();
    let report = compile(&base);
    let compile_ms = ms(t1.elapsed());
    let t2 = Instant::now();
    let _second = compile(&base);
    let compile_ms_second = ms(t2.elapsed());
    println!("fresh compile {compile_ms:8.2} ms  (second run {compile_ms_second:8.2} ms)",);
    println!("frontend      {frontend_ms:8.2} ms  (lex + parse + lower + resolve)",);
    println!(
        "check         {:8.2} ms  (the checker, where evaluation happens)",
        compile_ms - frontend_ms,
    );
    println!(
        "frontend ir {:?}, diagnostics {}, build ok {}\n",
        front.ir.is_some(),
        report.diagnostics.len(),
        report.build.as_ref().is_some_and(|b| b.ok),
    );
    for diagnostic in report.diagnostics.iter().take(5) {
        println!("  diagnostic: {diagnostic:?}");
    }

    // The session arm: one session across the edit sequence, one row per edit.
    let mut session = BufferSession::<LangProgram>::with_source_id(base.clone(), "probe.lichen");
    let t = Instant::now();
    let first = session.compile();
    println!(
        "session first compile {:8.2} ms (reused {}, frozen {})\n",
        ms(t.elapsed()),
        first.reused,
        first.cells.frozen,
    );

    let mut rows: Vec<Reading> = Vec::new();

    // Edit 1 — the cheap cell's seed: the heavy cone must not be recomputed.
    let shape = Shape {
        seed: 1,
        ..Shape::base(cheap, heavy, depth)
    };
    rows.push(measure(&mut session, &shape.source(), "cheap seed c0"));

    // Edit 2 — the first heavy node's depth: that node and its readers only.
    let shape = Shape {
        h0_depth: depth + 1,
        seed: 1,
        ..Shape::base(cheap, heavy, depth)
    };
    rows.push(measure(&mut session, &shape.source(), "heavy h0 depth+1"));

    // Edit 3 — append a cached statement.
    let shape = Shape {
        h0_depth: depth + 1,
        seed: 1,
        extra: true,
        ..Shape::base(cheap, heavy, depth)
    };
    rows.push(measure(
        &mut session,
        &shape.source(),
        "append a cached stmt",
    ));
    let with_extra = shape.source();

    // Edit 4 — whitespace only: no resolved structure changes, so no cell moves.
    let spaced = with_extra.replace("cache c0", "cache  c0");
    rows.push(measure(&mut session, &spaced, "whitespace only"));

    // Edit 5 — the tail only: the consumer changes, no cell does.
    let tail_edit = format!("{} + 1\n", spaced.trim_end());
    rows.push(measure(&mut session, &tail_edit, "tail only"));

    println!(
        "{:<22} {:>9} {:>10} {:>6} {:>7} {:>7} {:>8} {:>8}",
        "edit", "fresh ms", "session ms", "build", "reused", "frozen", "dropped", "retained"
    );
    println!("{}", "-".repeat(86));
    for r in &rows {
        println!(
            "{:<22} {:>9.2} {:>10.2} {:>6} {:>7} {:>7} {:>8} {:>8}",
            r.label,
            r.fresh_ms,
            r.session_ms,
            if r.reused { "reuse" } else { "new" },
            r.cells_reused,
            r.cells_frozen,
            r.cells_dropped,
            r.retained
        );
    }

    println!("\n== scaling: one heavy edit against the heavy-node count ==");
    println!(
        "{:<8} {:>10} {:>10} {:>7} {:>7} {:>8}",
        "heavy", "session ms", "fresh ms", "reused", "frozen", "dropped"
    );
    for heavy_count in [1usize, 2, 4] {
        let before = Shape::base(cheap, heavy_count, depth).source();
        let after = Shape {
            h0_depth: depth + 1,
            ..Shape::base(cheap, heavy_count, depth)
        }
        .source();
        let mut session = BufferSession::<LangProgram>::with_source_id(before, "probe.lichen");
        let _ = session.compile();
        let tf = Instant::now();
        let fresh = compile(&after);
        let fresh_ms = ms(tf.elapsed());
        session.set_source(after);
        let ts = Instant::now();
        let report = session.compile();
        let session_ms = ms(ts.elapsed());
        println!(
            "{heavy_count:<8} {session_ms:>10.2} {fresh_ms:>10.2} {:>7} {:>7} {:>8}   ok {}",
            report.cells.reused,
            report.cells.frozen,
            report.cells.dropped,
            fresh.build.as_ref().is_some_and(|b| b.ok),
        );
    }

    // `fib 16` is ~3 193 applies, which the 2 000 bound refused. Opt in: it takes minutes.
    if std::env::var("HEAVY16").is_ok() {
        println!("\n== heavy: {heavy} cells of fib 16, past the removed 2 000 apply budget ==");
        let heavy_source = Shape::base(cheap, heavy, 16).source();
        let mut session =
            BufferSession::<LangProgram>::with_source_id(heavy_source.clone(), "probe.lichen");
        let t = Instant::now();
        let report = session.compile();
        let session_ms = ms(t.elapsed());
        let tf = Instant::now();
        let _fresh = compile(&heavy_source);
        let fresh_ms = ms(tf.elapsed());
        println!(
            "session {session_ms:8.2} ms, fresh {fresh_ms:8.2} ms, build ok {}, diagnostics {}, frozen {}, retained {}",
            report.build.as_ref().is_some_and(|b| b.ok),
            report.diagnostics.len(),
            report.cells.frozen,
            session.retained_cells(),
        );
    }
}

fn measure(session: &mut BufferSession<LangProgram>, source: &str, label: &'static str) -> Reading {
    // The fresh arm first, so neither arm runs against a cache the other warmed.
    let tf = Instant::now();
    let _fresh = compile(source);
    let fresh_ms = ms(tf.elapsed());

    session.set_source(source);
    let ts = Instant::now();
    let report = session.compile();
    let session_ms = ms(ts.elapsed());

    Reading {
        label,
        fresh_ms,
        session_ms,
        reused: report.reused,
        cells_reused: report.cells.reused,
        cells_frozen: report.cells.frozen,
        cells_dropped: report.cells.dropped,
        retained: session.retained_cells(),
    }
}

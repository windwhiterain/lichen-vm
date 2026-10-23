//! Temporary: where a program's compile-and-run time and bytes go. Prints, asserts nothing.
//!
//! # Invariant
//!
//! Every reading is taken from one process: the allocator counts bytes this binary
//! requested, so they are attributable to the pipeline alone and to no other tenant.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use lichen_compute::language::new_store;
use lichen_language::run::evaluate_raw;

struct Counting;

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, new_size) };
        if !moved.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(new_size, Ordering::Relaxed);
            if new_size >= layout.size() {
                let live = LIVE.fetch_add(new_size - layout.size(), Ordering::Relaxed)
                    + (new_size - layout.size());
                PEAK.fetch_max(live, Ordering::Relaxed);
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Ordering::Relaxed);
            }
        }
        moved
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

struct Reading {
    name: String,
    ms: f64,
    allocs: usize,
    kib: f64,
    peak_kib: f64,
    ms_store: f64,
    allocs_store: usize,
    kib_store: f64,
    ok: bool,
    check_ms: f64,
    check_allocs: usize,
    check_ok: bool,
    nodes: usize,
    blocks: usize,
}

fn take(name: &str, file: &PathBuf, source: &str) -> Reading {
    let allocs_before = ALLOCS.load(Ordering::Relaxed);
    let bytes_before = BYTES.load(Ordering::Relaxed);
    PEAK.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    let started = Instant::now();
    let mut store = new_store(None).expect("the embedded compute package registers");
    let ms_store = started.elapsed().as_secs_f64() * 1.0e3;
    let allocs_store = ALLOCS.load(Ordering::Relaxed) - allocs_before;
    let bytes_store = BYTES.load(Ordering::Relaxed) - bytes_before;
    let answer = evaluate_raw(source, Some(file), &mut store);
    let ms = started.elapsed().as_secs_f64() * 1.0e3;
    let allocs = ALLOCS.load(Ordering::Relaxed) - allocs_before;
    let bytes = BYTES.load(Ordering::Relaxed) - bytes_before;
    let peak = PEAK.load(Ordering::Relaxed);
    // The check alone, over the file's own code, so the run's share is the difference.
    let (_interior, code) = lichen_language::preprocess::split_block(source);
    let check_allocs_before = ALLOCS.load(Ordering::Relaxed);
    let check_started = Instant::now();
    let report = lichen_language::compile(code);
    let check_ms = check_started.elapsed().as_secs_f64() * 1.0e3;
    let check_allocs = ALLOCS.load(Ordering::Relaxed) - check_allocs_before;
    let check_ok = report.ok();
    Reading {
        name: name.to_string(),
        ms,
        allocs,
        kib: bytes as f64 / 1024.0,
        peak_kib: peak as f64 / 1024.0,
        ms_store,
        allocs_store,
        kib_store: bytes_store as f64 / 1024.0,
        ok: answer.is_ok(),
        check_ms,
        check_allocs,
        check_ok,
        nodes: report
            .build
            .as_ref()
            .map_or(0, |build| build.module.nodes.len()),
        blocks: report
            .build
            .as_ref()
            .map_or(0, |build| build.module.blocks.len()),
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let repeats: usize = std::env::var("REPEATS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let only = std::env::var("ONLY").ok();

    let mut files: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("examples/ is readable")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "lichen")
        })
        .filter(|path| {
            only.as_ref().is_none_or(|only| {
                path.file_stem()
                    .is_some_and(|stem| stem.to_string_lossy() == *only.as_str())
            })
        })
        .collect();
    files.sort();

    // The store's own stages, so the fixed per-run cost is attributed rather than
    // measured whole.
    if std::env::var("STAGES").is_ok() {
        fn stage(label: &str, started: Instant, allocs: usize, bytes: usize) {
            println!(
                "{label:<24} {:>7.2} ms {:>8} allocs {:>9.1} KiB",
                started.elapsed().as_secs_f64() * 1.0e3,
                ALLOCS.load(Ordering::Relaxed) - allocs,
                (BYTES.load(Ordering::Relaxed) - bytes) as f64 / 1024.0,
            );
        }
        let mut store =
            lichen_language::package::PackageStore::<lichen_compute::language::LangProgram>::new();
        let (allocs, bytes) = (
            ALLOCS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
        );
        let started = Instant::now();
        lichen_compute::language::register_compute(&mut store).expect("the wrapper compiles");
        stage("register_compute", started, allocs, bytes);
        let (allocs, bytes) = (
            ALLOCS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
        );
        let started = Instant::now();
        let import = store.prelude_import();
        stage("prelude_import", started, allocs, bytes);
        println!("  (prelude ok {})", import.is_ok());
    }

    let mut readings: Vec<Reading> = Vec::new();
    for _ in 0..repeats {
        for file in &files {
            let source = std::fs::read_to_string(file).expect("a program is readable");
            let name = file.file_name().unwrap().to_string_lossy().to_string();
            readings.push(take(&name, file, &source));
        }
    }

    println!(
        "{:<26} {:>9} {:>9} {:>10} {:>9} {:>9} {:>8} {:>9} {:>7} {:>7} {:>6}",
        "example",
        "ms",
        "check ms",
        "allocs",
        "check al",
        "alloc KiB",
        "peak KiB",
        "store ms",
        "nodes",
        "blocks",
        "ok"
    );
    println!("{}", "-".repeat(115));
    for reading in &readings {
        println!(
            "{:<26} {:>9.2} {:>9.2} {:>10} {:>9} {:>10.1} {:>9.1} {:>9.2} {:>7} {:>7} {:>6}",
            reading.name,
            reading.ms,
            reading.check_ms,
            reading.allocs,
            reading.check_allocs,
            reading.kib,
            reading.peak_kib,
            reading.ms_store,
            reading.nodes,
            reading.blocks,
            reading.ok
        );
    }

    // The corpus' total, so a change's aggregate is one number rather than a median.
    let ms: f64 = readings.iter().map(|reading| reading.ms).sum();
    let check_ms: f64 = readings.iter().map(|reading| reading.check_ms).sum();
    let allocs: usize = readings.iter().map(|reading| reading.allocs).sum();
    let check_allocs: usize = readings.iter().map(|reading| reading.check_allocs).sum();
    let kib: f64 = readings.iter().map(|reading| reading.kib).sum();
    let peak: f64 = readings
        .iter()
        .map(|reading| reading.peak_kib)
        .fold(0.0, f64::max);
    let store_ms: f64 = readings.iter().map(|reading| reading.ms_store).sum();
    let nodes: usize = readings.iter().map(|reading| reading.nodes).sum();
    let blocks: usize = readings.iter().map(|reading| reading.blocks).sum();
    println!("{}", "-".repeat(115));
    println!(
        "{:<26} {:>9.2} {:>9.2} {:>10} {:>9} {:>10.1} {:>9.1} {:>9.2} {:>7} {:>7}",
        "total (peak = max)",
        ms,
        check_ms,
        allocs,
        check_allocs,
        kib,
        peak,
        store_ms,
        nodes,
        blocks
    );
}

//! Probe for the `gc.lichen` panic: separates a stale key (state carried across
//! applications) from a foreign one, and isolates which part of the runner's
//! setup the panic needs. See docs/notes/examples-panic-diagnosis.md.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use lichen_compute::language::LangProgram;

const GC_SOURCE: &str = "add = x => y => x + y\ndouble = n => @gc { a = n + 1\n b = add a a\n b }\n@gc [double 3, double 7]";

/// The panic's own first line, so a diagnostic `assert!` cannot be read as the SlotMap panic.
fn message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn report(label: &str, run: Result<String, Box<dyn std::any::Any + Send>>) {
    match run {
        Ok(output) => println!("PROBE: {label}: OK {output}"),
        Err(payload) => {
            let text = message(payload.as_ref());
            let first = text.lines().next().unwrap_or("").to_string();
            println!("PROBE: {label}: PANICKED {first}");
        }
    }
}

fn gc_file() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/gc.lichen")
}

fn guarded(label: &str, body: impl FnOnce() -> String) {
    report(label, catch_unwind(AssertUnwindSafe(body)));
}

#[test]
fn gc_variants() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let file = gc_file();
    let source_on_disk = std::fs::read_to_string(&file).unwrap_or_default();
    let disk_source = source_on_disk
        .lines()
        .filter(|line| !line.trim_start().starts_with("---"))
        .collect::<Vec<_>>()
        .join("\n");
    println!("PROBE: disk source = {disk_source:?}");

    guarded("file-on-disk-store-base", || {
        lichen_language::testing::run_at::<LangProgram>(
            &source_on_disk,
            Some(file.as_path()),
            &mut lichen_compute::language::new_store(None).unwrap(),
        );
        "ran".to_string()
    });
    guarded("inline-store-base", || {
        lichen_language::testing::run_at::<LangProgram>(
            GC_SOURCE,
            Some(file.as_path()),
            &mut lichen_compute::language::new_store(None).unwrap(),
        );
        "ran".to_string()
    });
    guarded("inline-store-no-base", || {
        lichen_language::testing::run_at::<LangProgram>(
            GC_SOURCE,
            None,
            &mut lichen_compute::language::new_store(None).unwrap(),
        );
        "ran".to_string()
    });
    guarded("no-store-no-base", || {
        lichen_language::testing::evaluate::<LangProgram>(GC_SOURCE);
        "ran".to_string()
    });
    guarded("bare-store-base", || {
        let mut store = lichen_language::package::PackageStore::<LangProgram>::new();
        lichen_language::testing::run_at::<LangProgram>(
            GC_SOURCE,
            Some(file.as_path()),
            &mut store,
        );
        "ran".to_string()
    });
    guarded("file-on-disk-no-store-no-base", || {
        lichen_language::testing::evaluate::<LangProgram>(&source_on_disk);
        "ran".to_string()
    });
    std::panic::set_hook(previous);
}

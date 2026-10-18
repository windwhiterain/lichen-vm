//! The kernel disk cache, measured over three processes against one directory.
//! See docs/notes/kernel-disk-cache.md.

use std::path::{Path, PathBuf};

use lichen_compute::language::{LangProgram, new_store};
use lichen_compute::{module_cache_disk_hits, module_cache_disk_writes, module_cache_misses};
use lichen_language::package::PackageStore;

/// One fixed directory, so the three processes are three runs against one cache.
fn probe_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/kernel-cache-probe")
}

/// A kernel that cross-calls another, so the launch set is two fragments.
const CROSS_KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
k0 = compute.jit (v : Int => v + 1)
k1 = compute.jit (v : Int => compute.call k0 v + 1)
compute.launch k1 40
"#;

/// A second, independent root: its own file beside the first one's.
const SECOND_ROOT: &str = r#"
--- compute = import "compute.lichen" ---
k2 = compute.jit (v : Int => v + v)
compute.launch k2 11
"#;

/// Every file under `root`, as `(relative name, bytes)`, in name order.
fn listing(root: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(len) = entry.metadata().map(|metadata| metadata.len()) {
                let name = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, len));
            }
        }
    }
    out.sort();
    out
}

/// The cached module files under the `kernels` directory, in name order.
fn cached_modules(root: &Path) -> Vec<PathBuf> {
    let mut modules: Vec<PathBuf> = listing(root)
        .into_iter()
        .filter(|(name, _)| name.ends_with(".wasm"))
        .map(|(name, _)| root.join(name))
        .collect();
    modules.sort();
    modules
}

/// Overwrite one cached payload with junk, past the header its hash covers.
fn corrupt_one(root: &Path) {
    let Some(path) = cached_modules(root).into_iter().next() else {
        println!("heal: no cached module to corrupt");
        return;
    };
    let payload = std::fs::read(&path).expect("the cached module reads");
    let mut junk = payload[..32].to_vec();
    junk.extend(std::iter::repeat_n(0xa5u8, payload.len() - 32));
    std::fs::write(&path, &junk).expect("the cached module rewrites");
    println!(
        "heal: {} bytes of payload in {} are junk now",
        payload.len() - 32,
        path.display()
    );
}

fn evaluate(store: &mut PackageStore<LangProgram>, source: &str) -> String {
    lichen_language::run::evaluate_raw(source, None, store)
        .unwrap_or_else(|diags| format!("<refused: {:?}>", diags.first().map(|d| &d.message)))
}

fn report(root: &Path, step: &str, value: &str) {
    let files = listing(root);
    let total: u64 = files.iter().map(|(_, len)| len).sum();
    let modules = files
        .iter()
        .filter(|(name, _)| name.ends_with(".wasm"))
        .count();
    println!(
        "{step:<26} value {value:<6} misses {}  disk hits {}  disk writes {}  files {} \
         ({modules} module(s), {total} bytes)",
        module_cache_misses(),
        module_cache_disk_hits(),
        module_cache_disk_writes(),
        files.len(),
    );
    for (name, len) in &files {
        println!("    {len:>8}  {name}");
    }
}

fn main() {
    let mode = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "warm".to_string());
    let root = probe_root();
    match mode.as_str() {
        "cold" => {
            let _ = std::fs::remove_dir_all(&root);
            println!("cold: emptied {}", root.display());
        }
        "heal" => corrupt_one(&root),
        "reverse" => println!("reverse: the same two programs, compiled the other way round"),
        other => println!("{other}: reusing {}", root.display()),
    }

    let mut store = new_store(Some(root.clone())).expect("the compute package registers");
    let configured = lichen_runtime::kernel_disk_cache()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| "<none>".to_string());
    println!("run {mode}: kernel disk cache at {configured}");

    if mode == "reverse" {
        let second = evaluate(&mut store, SECOND_ROOT);
        report(&root, "1. the second root", &second);
        let first = evaluate(&mut store, CROSS_KERNEL);
        report(&root, "2. cross-kernel kernel", &first);
        return;
    }

    let first = evaluate(&mut store, CROSS_KERNEL);
    report(&root, "1. cross-kernel kernel", &first);
    let again = evaluate(&mut store, CROSS_KERNEL);
    report(&root, "2. the same kernel again", &again);
    let second = evaluate(&mut store, SECOND_ROOT);
    report(&root, "3. a second root", &second);
    let second_again = evaluate(&mut store, SECOND_ROOT);
    report(&root, "4. the second root again", &second_again);
}

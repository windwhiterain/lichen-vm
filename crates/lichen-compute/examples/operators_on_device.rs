//! One row per operator example on a device. See `docs/notes/device-operator-sweep.md`.
//!
//! # Invariant
//! One example per process: the kernel registry and the installed backend are
//! process-global, so a second program in one process would read state the first
//! filled. Each child runs the source unchanged as the control, then the same source
//! with only `"cpu"` replaced by `"gpu"`, so the backend literal is the one edit.

use std::path::{Path, PathBuf};
use std::process::Command;

use lichen_tools::readme;

/// What one example's device run said.
enum Verdict {
    /// Bit for bit, read off the two rendered answers.
    Agrees,
    /// Low bits moved; the distance is printed by [`one`].
    Differs,
    /// A named refusal, which is an outcome rather than a failure.
    Refused,
    /// The control itself failed, so there is nothing to compare against.
    Failed,
}

/// Run one program, rendered exactly as the CLI renders it.
fn run(file: &Path, source: &str) -> Result<String, Vec<String>> {
    let mut store =
        lichen_compute::language::new_store(None).expect("the embedded compute package registers");
    lichen_language::run::evaluate_raw(source, Some(file), &mut store)
        .map_err(|diagnostics| diagnostics.into_iter().map(|d| d.message).collect())
}

/// The examples that dispatch a kernel, in the order the example suite reads them.
fn operators() -> Vec<(String, PathBuf)> {
    readme::example_files()
        .expect("the examples directory is readable")
        .into_iter()
        .filter(|(_, path)| readme::read_normalized(path).contains("compute.parallel"))
        .collect()
}

/// Every number a rendered value spells, skipping the type's own lengths.
fn numbers(rendered: &str) -> Vec<f32> {
    let rendered = rendered
        .rsplit_once(": ")
        .map_or(rendered, |(value, _)| value);
    let bytes = rendered.as_bytes();
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if rendered[at..].starts_with("NaN") {
            found.push(f32::NAN);
            at += 3;
        } else if rendered[at..].starts_with("-inf") {
            found.push(f32::NEG_INFINITY);
            at += 4;
        } else if rendered[at..].starts_with("inf") {
            found.push(f32::INFINITY);
            at += 3;
        } else if bytes[at].is_ascii_digit()
            || (bytes[at] == b'-' && bytes.get(at + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = at;
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_digit() || bytes[at] == b'.') {
                at += 1;
            }
            if let Ok(value) = rendered[start..at].parse() {
                found.push(value);
            }
        } else {
            at += 1;
        }
    }
    found
}

/// Two floats' distance in units in the last place, over the IEEE total order.
fn distance(left: f32, right: f32) -> u32 {
    if left == right {
        return 0;
    }
    if left.is_nan() || right.is_nan() {
        return u32::MAX;
    }
    fn ordered(value: f32) -> i64 {
        let bits = value.to_bits() as i32;
        if bits < 0 {
            i32::MIN as i64 - bits as i64
        } else {
            bits as i64
        }
    }
    (ordered(left) - ordered(right))
        .unsigned_abs()
        .min(u32::MAX as u64) as u32
}

/// The two rendered answers, compared element by element.
fn compare(control: &str, device: &str) -> Option<(usize, usize, u32)> {
    let left = numbers(control);
    let right = numbers(device);
    if left.len() != right.len() {
        return None;
    }
    let mut differing = 0;
    let mut worst = 0;
    for (left, right) in left.iter().zip(&right) {
        let span = distance(*left, *right);
        if span > 0 {
            differing += 1;
            worst = worst.max(span);
        }
    }
    Some((left.len(), differing, worst))
}

/// One example: the declared output, the two backends' answers, and the verdict.
fn one(file: &Path) -> Verdict {
    let source = readme::read_normalized(file);
    let declared = readme::declared_output(&source);
    let control = match run(file, &source) {
        Ok(answer) => answer,
        Err(refusals) => {
            println!("  cpu refused:");
            for reason in refusals {
                println!("    {reason}");
            }
            return Verdict::Failed;
        }
    };
    let context = match lichen_compute_gpu::GpuContext::new() {
        Ok(context) => context,
        Err(error) => {
            println!("  no device: {error}");
            return Verdict::Failed;
        }
    };
    println!("  device:   {}", context.device_name());
    lichen_compute_gpu::install(context);
    let device_run = run(file, &source.replace("\"cpu\"", "\"gpu\""));
    lichen_compute_gpu::uninstall();

    match declared {
        Some(ref declared) => {
            let drift = if *declared == control {
                ""
            } else {
                "  (cpu drifted)"
            };
            println!("  declared: {declared}{drift}");
        }
        None => println!("  declared: (none)"),
    }
    println!("  cpu:      {control}");
    let answer = match device_run {
        Ok(answer) => answer,
        Err(refusals) => {
            println!("  gpu:      refused by name");
            for reason in refusals {
                println!("    {reason}");
            }
            return Verdict::Refused;
        }
    };
    println!("  gpu:      {answer}");
    match compare(&control, &answer) {
        Some((elements, 0, _)) => {
            println!("  verdict:  agrees, {elements} of {elements} element(s)");
            Verdict::Agrees
        }
        Some((elements, differing, worst)) => {
            println!(
                "  verdict:  differs, {differing} of {elements} element(s), worst {worst} ulp"
            );
            Verdict::Differs
        }
        None => {
            println!("  verdict:  differs in shape");
            Verdict::Failed
        }
    }
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|flag| flag == "--one") {
        let path = PathBuf::from(&arguments[1]);
        println!("{}", path.display());
        let measured = !matches!(one(&path), Verdict::Failed);
        std::process::exit(if measured { 0 } else { 1 });
    }

    let executable = std::env::current_exe().expect("the example's own binary path");
    let mut rows: Vec<(String, bool)> = Vec::new();
    for (name, path) in operators() {
        let status = Command::new(&executable)
            .arg("--one")
            .arg(&path)
            .status()
            .expect("the example can re-execute itself");
        rows.push((name, status.success()));
    }
    println!("\n=== one line per example ===");
    for (name, measured) in rows {
        let verdict = if measured { "measured" } else { "EXAMINE" };
        println!("{verdict:>9}  {name}");
    }
}

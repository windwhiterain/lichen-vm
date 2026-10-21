//! Probe: where a program's cost goes, phase by phase.
//!
//! # Invariant
//!
//! Print-only. The phases are the three the language's own session runs: the
//! preprocess (which parses), the compile (which checks and lowers), and the
//! evaluation (which runs the program).

use std::time::Instant;

use super::common;

use lichen_compute::language::new_store;

/// One program of the shape the suite's tests use: a parallel kernel and a run.
const PROGRAM: &str = r#"
---
  compute = import "compute.lichen"
---
In  = struct{.a Int}
Out = struct{.z (compute.Buf _)}
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value i + i))
}
k = compute.parallel f "gpu" Out
out = (compute.plrun k ((compute.A In)(.n 4, .I In(.a 0))) : Out)
(compute.read ((compute.Read _)(.from out.z, .at 0)) : Int)
"#;

#[test]
fn probe_the_phases() {
    let repeats = 5;

    let started = Instant::now();
    for _ in 0..repeats {
        let mut store = new_store(None).expect("the embedded compute package registers");
        let _ = lichen_language::preprocess::preprocess(PROGRAM, None, &mut store);
    }
    let preprocess = started.elapsed() / repeats;

    let started = Instant::now();
    for _ in 0..repeats {
        let _ = common::check(PROGRAM);
    }
    let check = started.elapsed() / repeats;

    let started = Instant::now();
    for _ in 0..repeats {
        let _ = common::run(PROGRAM);
    }
    let run = started.elapsed() / repeats;

    println!("preprocess (parse): {preprocess:?}");
    println!("check (parse+check+lower): {check:?}");
    println!("run (check+evaluate): {run:?}");
    println!(
        "so the evaluation alone is about {:?}",
        run.checked_sub(check).unwrap_or_default()
    );
}

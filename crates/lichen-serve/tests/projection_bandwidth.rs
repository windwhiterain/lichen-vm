//! What one projection costs, and whether its price grows with the rows or with each row's walk.

use std::time::Instant;

use lichen_compute::compute::graph::RunArgument;
use lichen_kernel_ir::ScalarClass;
use lichen_serve::{MATVEC_ROW_STEP, Session};

/// The input width every projection walks.
const DIM: usize = 2048;

/// The rows to time, from one workgroup's worth to enough to fill a device.
const ROWS: [usize; 5] = [1, 64, 256, 1024, 4096];

/// Whether a result is kept, which decides whether the run that produced it releases it.
///
/// # Invariant
/// A run releases every buffer it produced that the caller neither brought in nor is handed back, so a probe
/// that keeps its answers never takes that path and measures a step the model never runs.
const KEPT: [bool; 2] = [true, false];

/// The walks each run repeats, so a dispatch's own cost is a small part of the measurement.
const REPEATS: usize = 20;

/// A projection's time per row, and the bytes a second its reads amount to.
///
/// # Invariant
/// Every row reads `DIM` weights, so the bytes a row costs are the same for every row: a constant time per
/// row is a walk that does not depend on the rows, and a constant time in total would be a launch that is.
#[test]
fn a_projection_costs_what_its_rows_walk() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_projection_costs_what_its_rows_walk: no device");
        return;
    };
    let prepared = match session.prepare(MATVEC_ROW_STEP) {
        Ok(prepared) => prepared,
        Err(refusal) => {
            eprintln!("SKIPPED: {refusal}");
            return;
        }
    };
    // The model's own programs are the narrowed ones, so a Float probe may be measuring a path it never runs.
    match session.prepare(MATVEC_ROW_STEP) {
        Ok(prepared) => {
            println!("pass lines: {}", prepared.report().len());
            for line in prepared.report() {
                println!("pass: {line}");
            }
        }
        Err(refusal) => println!("the row-major projection does not prepare: {refusal}"),
    }
    let halved = match lichen_serve::narrowed(&session, MATVEC_ROW_STEP) {
        Ok(prepared) => {
            println!("narrowed pass lines: {}", prepared.report().len());
            for line in prepared.report() {
                println!("narrowed pass: {line}");
            }
            Some(prepared)
        }
        Err(refusal) => {
            eprintln!("the narrowed projection does not prepare: {refusal}");
            None
        }
    };
    for rows in ROWS {
        let weights: Vec<u8> = (0..rows * DIM)
            .flat_map(|index| ((index % 251) as f32 * 0.001).to_le_bytes())
            .collect();
        let values: Vec<u8> = (0..DIM)
            .flat_map(|index| ((index % 97) as f32 * 0.01).to_le_bytes())
            .collect();
        // A resident weight is what the model steps over, not a buffer handed in on every step.
        let placed = session
            .place(&weights, ScalarClass::Float)
            .unwrap_or_else(|refusal| panic!("the weights are placed: {refusal}"));
        let arguments = || {
            vec![
                RunArgument::Count(rows as i64),
                RunArgument::Count(DIM as i64),
                RunArgument::Count((rows / DIM).max(1) as i64),
                RunArgument::Buffer {
                    class: ScalarClass::Float,
                    data: values.clone(),
                },
                RunArgument::Resident(placed),
            ]
        };
        let mut timings = Vec::new();
        let mut answered = Vec::with_capacity(REPEATS);
        for kept in KEPT {
            let started = Instant::now();
            let mut last = None;
            for _ in 0..REPEATS {
                let stepped = prepared
                    .step(arguments())
                    .unwrap_or_else(|refusal| panic!("the projection runs: {refusal}"));
                let [lichen_compute::compute::graph::RunResult::Resident(resident)] =
                    &stepped.results[..]
                else {
                    panic!("a projection answers with one resident");
                };
                last = Some(*resident);
                if kept {
                    answered.push(*resident);
                }
            }
            timings.push(started.elapsed().as_secs_f64() / REPEATS as f64);
            if let (false, Some(resident)) = (kept, last) {
                let _ = session.release(resident);
            }
        }
        let dispatched = timings[0];
        let started = Instant::now();
        for resident in &answered {
            let _ = session.fetch(*resident).expect("it answers");
        }
        let read_back = started.elapsed().as_secs_f64() / REPEATS as f64;
        let _ = session.release(placed);
        let each = dispatched + read_back;
        let bytes = (rows * DIM * 4) as f64;
        println!(
            "rows {rows:5} kept {:.3} ms  released {:.3} ms  read back {:.3} ms  bytes/s {:.1} GB/s",
            timings[0] * 1000.0,
            timings[1] * 1000.0,
            read_back * 1000.0,
            bytes / each / 1e9
        );
        if let Some(halved) = &halved {
            let halves: Vec<u8> = (0..rows * DIM)
                .flat_map(|index| (((index % 251) as u16) << 4).to_le_bytes())
                .collect();
            let placed = session
                .place(&halves, ScalarClass::BFloat16)
                .unwrap_or_else(|refusal| panic!("the halves are placed: {refusal}"));
            let half_values: Vec<u8> = (0..DIM)
                .flat_map(|index| (((index % 97) as u16) << 4).to_le_bytes())
                .collect();
            let started = Instant::now();
            for _ in 0..REPEATS {
                let stepped = halved
                    .step(vec![
                        RunArgument::Count(rows as i64),
                        RunArgument::Count(DIM as i64),
                        RunArgument::Count((rows / DIM).max(1) as i64),
                        RunArgument::Buffer {
                            class: ScalarClass::BFloat16,
                            data: half_values.clone(),
                        },
                        RunArgument::Resident(placed),
                    ])
                    .unwrap_or_else(|refusal| panic!("the narrowed projection runs: {refusal}"));
                let _ = stepped;
            }
            println!(
                "rows {rows:5} narrowed {:.3} ms",
                started.elapsed().as_secs_f64() / REPEATS as f64 * 1000.0
            );
            let _ = session.release(placed);
        }
    }
}

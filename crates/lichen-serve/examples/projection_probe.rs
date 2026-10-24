//! One projection step against its row count: is the cost the bytes, the lanes, or the step?

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{MATVEC_BATCH_STEP, Session, narrowed};

fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: lichen_kernel_ir::ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    let dim = 2048usize;
    let per = 1024usize;
    let weight: Vec<f32> = (0..dim * per).map(|at| (at % 7) as f32 * 0.001).collect();
    let weight_bytes: Vec<u8> = weight
        .iter()
        .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
        .collect();
    let held = session
        .place(&weight_bytes, lichen_kernel_ir::ScalarClass::BFloat16)
        .expect("the weight places");
    let projection = narrowed(&session, MATVEC_BATCH_STEP).expect("the projection prepares");
    for rows in [1usize, 8, 64] {
        let x: Vec<f32> = (0..rows * dim).map(|at| (at % 13) as f32 * 0.001).collect();
        let mut total = 0.0;
        let rounds = 20;
        for _ in 0..rounds {
            let started = std::time::Instant::now();
            let stepped = projection
                .step(vec![
                    RunArgument::Count((rows * per) as i64),
                    RunArgument::Count(dim as i64),
                    RunArgument::Count(per as i64),
                    RunArgument::Buffer {
                        class: lichen_kernel_ir::ScalarClass::BFloat16,
                        data: x
                            .iter()
                            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
                            .collect(),
                    },
                    RunArgument::Resident(held),
                ])
                .expect("the projection runs");
            total += started.elapsed().as_secs_f64();
            for result in &stepped.results {
                if let RunResult::Resident(resident) = result {
                    session.release(*resident);
                }
            }
        }
        println!(
            "rows {rows}: {:.3} ms a step, {:.1} GB/s of weight",
            total * 1000.0 / rounds as f64,
            (dim * per * 2) as f64 / (total / rounds as f64) / 1e9
        );
    }
    session.release(held);
}

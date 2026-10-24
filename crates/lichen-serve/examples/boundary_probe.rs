//! The per-step floor and the attention's growth with the cache: two edges, timed.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{ATTEND_CACHE_STEP, ROPE_STEP, Session};

fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: lichen_kernel_ir::ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

fn run(session: &Session, prepared: &lichen_serve::Prepared<'_>, args: Vec<RunArgument>) -> f64 {
    let started = std::time::Instant::now();
    let stepped = prepared.step(args).expect("the step runs");
    let elapsed = started.elapsed().as_secs_f64();
    for result in &stepped.results {
        if let RunResult::Resident(resident) = result {
            session.release(*resident);
        }
    }
    elapsed
}

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    let rope = session.prepare(ROPE_STEP).expect("rope prepares");
    let values: Vec<f32> = (0..1024).map(|at| (at as f32) * 0.001).collect();
    let mut total = 0.0;
    for _ in 0..100 {
        total += run(
            &session,
            &rope,
            vec![
                RunArgument::Count(1024),
                RunArgument::Count(1),
                floats(&values),
            ],
        );
    }
    println!("rope: {:.3} ms a step", total * 10.0);

    let attention = session
        .prepare(ATTEND_CACHE_STEP)
        .expect("attention prepares");
    for p in [4usize, 64, 256] {
        let q: Vec<f32> = (0..2048).map(|at| (at as f32) * 0.001).collect();
        let k: Vec<f32> = (0..p * 1024).map(|at| (at as f32) * 0.001).collect();
        let v: Vec<f32> = (0..p * 1024).map(|at| (at as f32) * 0.002).collect();
        let mut total = 0.0;
        let rounds = 20;
        for _ in 0..rounds {
            total += run(
                &session,
                &attention,
                vec![
                    RunArgument::Count(16 * p as i64),
                    RunArgument::Count(p as i64),
                    floats(&q),
                    floats(&k),
                    floats(&v),
                ],
            );
        }
        println!(
            "attention p={p}: {:.3} ms a step ({:.1} MB a step of cache)",
            total * 1000.0 / rounds as f64,
            (p * 1024 * 2 * 4) as f64 / 1e6
        );
    }
}

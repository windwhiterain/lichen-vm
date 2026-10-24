//! A composed graph prepared at the narrowed width: the class pack that needs no plan or claim.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{PRE_STEP, Session, narrowed_composed};

fn bf16(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: lichen_kernel_ir::ScalarClass::BFloat16,
        data: values
            .iter()
            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
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
    let prepared = match narrowed_composed(&session, PRE_STEP, &[2, 6, 7], &[(1, "bfloat16")]) {
        Ok(prepared) => prepared,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let hidden: Vec<f32> = (0..2048).map(|at| (at % 11) as f32 * 0.001).collect();
    let gain: Vec<f32> = (0..2048).map(|at| (at % 7) as f32 * 0.001).collect();
    let k: Vec<f32> = (0..1024 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    let knorm: Vec<f32> = (0..128).map(|at| (at % 3) as f32 * 0.001).collect();
    let q: Vec<f32> = (0..2048 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    let qnorm: Vec<f32> = (0..128).map(|at| (at % 3) as f32 * 0.001).collect();
    let v: Vec<f32> = (0..1024 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    let stepped = prepared
        .step(vec![
            RunArgument::Count(0),
            bf16(&hidden),
            bf16(&gain),
            bf16(&k),
            bf16(&knorm),
            bf16(&q),
            bf16(&qnorm),
            bf16(&v),
        ])
        .expect("the composed step runs");
    let mut lengths = Vec::new();
    for result in &stepped.results {
        let RunResult::Resident(resident) = result else {
            panic!("no resident");
        };
        let answer = session.fetch(*resident).expect("the answer reads");
        lengths.push(match answer {
            lichen_kernel_ir::ScalarData::Float(values) => values.len(),
            other => panic!("unexpected {other:?}"),
        });
        session.release(*resident);
    }
    println!(
        "composed narrowed step: {} returns, lengths {lengths:?}",
        lengths.len()
    );
}

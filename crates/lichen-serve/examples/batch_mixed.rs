//! Two requests at different positions: their answers must be the single chain's.

use lichen_serve::{Batched, Qwen, Sampling, Session, Width};

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    let mut model = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    if let Err(refusal) = model.hold_in_memory() {
        eprintln!("{refusal}");
        std::process::exit(1);
    }
    let sampling = Sampling {
        temperature: 0.0,
        top_k: 0,
        top_p: 1.0,
        seed: 0,
    };
    let mut batch = Batched::new(&model);
    let first = batch
        .admit(&[9707], 4, sampling, 1)
        .expect("the first request is admitted");
    let second = batch
        .admit(&[9707, 25, 358], 4, sampling, 1)
        .expect("the second request is admitted");
    for _ in 0..8 {
        let _ = batch.step().expect("a step answers");
        if batch.done(first) && batch.done(second) {
            break;
        }
    }
    println!("first {:?}", batch.answer(first));
    println!("second {:?}", batch.answer(second));
}

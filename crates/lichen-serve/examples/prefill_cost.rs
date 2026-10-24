//! Time to the first answer for a prompt of N tokens, which is the prefill's ceiling.

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
    for length in [8usize, 32, 128] {
        let prompt: Vec<usize> = (0..length).map(|at| [9707, 25, 358][at % 3]).collect();
        let mut batch = Batched::new(&model);
        let (before_work, started) = (session.work(), std::time::Instant::now());
        let id = batch
            .admit(&prompt, 1, sampling, 1)
            .expect("the request is admitted");
        let mut steps = 0;
        loop {
            steps += 1;
            let answers = batch.step().expect("a step answers");
            if answers.iter().any(|(answered, _)| *answered == id) {
                break;
            }
            if steps > length + 4 {
                break;
            }
        }
        let after_work = session.work();
        println!(
            "prompt {length}: {:.3} s to the first answer, {steps} steps, {} submissions",
            started.elapsed().as_secs_f64(),
            after_work.0 - before_work.0
        );
    }
}

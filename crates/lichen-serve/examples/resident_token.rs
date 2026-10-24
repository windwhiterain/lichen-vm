//! A token with the weights resident on the device, where the per-step upload stops.

use lichen_serve::{Qwen, Session, Width};

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
    let layers: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(28);
    let started = std::time::Instant::now();
    if let Err(refusal) = model.reside(layers) {
        eprintln!("reside: {refusal}");
        std::process::exit(1);
    }
    if let Err(refusal) = model.place_head() {
        eprintln!("head: {refusal}");
        std::process::exit(1);
    }
    println!(
        "resided {layers} layers and the head in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    let (before_work, before_stage, before_kinds) =
        (session.work(), model.stage_nanos(), model.stage_costs());
    let started = std::time::Instant::now();
    let generated = match model.generate(9707, 4) {
        Ok(generated) => generated,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed().as_secs_f64();
    let (after_work, after_stage, after_kinds) =
        (session.work(), model.stage_nanos(), model.stage_costs());
    println!("tokens: {:?}", generated.tokens);
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("  token {}: {:.3} s", index + 1, elapsed.as_secs_f64());
    }
    println!(
        "submissions {}, fences {}, {:.3} s; stages {:.3} s",
        after_work.0 - before_work.0,
        after_work.1 - before_work.1,
        elapsed,
        (after_stage - before_stage) as f64 / 1e9
    );
    let names = [
        "norm",
        "projection",
        "rotation",
        "attention",
        "product",
        "head",
    ];
    for (at, name) in names.iter().enumerate() {
        println!(
            "  {name}: {:.3} s",
            (after_kinds[at] - before_kinds[at]) as f64 / 1e9
        );
    }
}

//! A warm token's clock, split into its stages and the work around them.

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
    let start: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(9707);
    let count: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    let (before_work, before_stage) = (session.work(), model.stage_nanos());
    let generated = match model.generate(start, count) {
        Ok(generated) => generated,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let (after_work, after_stage) = (session.work(), model.stage_nanos());
    let wall: f64 = generated
        .per_token
        .iter()
        .map(|one| one.as_secs_f64())
        .sum();
    let stage = (after_stage - before_stage) as f64 / 1e9;
    println!("tokens: {:?}", generated.tokens);
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("  token {}: {:.3} s", index + 1, elapsed.as_secs_f64());
    }
    println!(
        "submissions {}, fences {}",
        after_work.0 - before_work.0,
        after_work.1 - before_work.1
    );
    println!(
        "stages {:.3} s of {:.3} s; around {:.3} s ({:.0}%)",
        stage,
        wall,
        wall - stage,
        100.0 * (wall - stage) / wall
    );
}

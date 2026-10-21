//! Generate tokens from Qwen3-1.7B; the weights come from LICHEN_MODEL_DIR.

use lichen_serve::{Qwen, Sampling, Session, Width};

/// A refusal ends the run: a token nobody computed is worse than no token.
fn main() {
    let mut arguments = std::env::args().skip(1);
    let start: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(9707);
    let count: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let mut wide = false;
    let mut temperature = 0.0_f32;
    let mut top_k = 0_usize;
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--wide" => wide = true,
            "--temperature" => {
                temperature = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0.0);
            }
            "--top-k" => {
                top_k = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
            other => eprintln!("ignoring {other}"),
        }
    }
    let sampling = Sampling::sampled(temperature, top_k, 7);
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no compute backend: {refusal}");
            std::process::exit(1);
        }
    };
    let width = match wide {
        true => Width::Wide,
        false => Width::Narrowed,
    };
    let mut model = match Qwen::open_with(&session, width) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    // The halves are read once and kept: what a token pays after that is the work, not the file.
    if !wide && let Err(refusal) = model.hold_in_memory() {
        eprintln!("{refusal}");
        std::process::exit(1);
    }
    println!(
        "generating {count} token(s) after {start} at {width:?}, temperature {} top {top_k}",
        sampling.temperature
    );
    let generated =
        match model.generate_sampled(start, count, sampling, |token| println!("  {token}")) {
            Ok(generated) => generated,
            Err(refusal) => {
                eprintln!("{refusal}");
                std::process::exit(1);
            }
        };
    println!("tokens: {:?}", generated.tokens);
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("  token {} took {:.2} s", index + 1, elapsed.as_secs_f64());
    }
}

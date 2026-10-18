//! Generate tokens from Qwen3-1.7B; the weights come from LICHEN_MODEL_DIR.

use lichen_serve::{Qwen, Session, Width};

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
    let wide = arguments.any(|value| value == "--wide");
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
    println!("generating {count} token(s) after {start} at {width:?}");
    let generated = match model.generate_with(start, count, |token| println!("  {token}")) {
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

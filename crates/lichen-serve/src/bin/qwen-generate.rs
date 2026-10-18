//! Generate tokens from Qwen3-1.7B; the weights come from LICHEN_MODEL_DIR.

use lichen_serve::{Qwen, Session};

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
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no compute backend: {refusal}");
            std::process::exit(1);
        }
    };
    let model = match Qwen::open(&session) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    println!("generating {count} token(s) after {start}");
    let generated = match model.generate(start, count) {
        Ok(generated) => generated,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    println!("tokens: {:?}", generated.tokens);
    let total: f64 = generated
        .per_token
        .iter()
        .map(|elapsed| elapsed.as_secs_f64())
        .sum();
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("  token {} took {:.1} s", index + 1, elapsed.as_secs_f64());
    }
    println!(
        "{total:.1} s in all, {:.1} s per token",
        total / generated.per_token.len() as f64
    );
}

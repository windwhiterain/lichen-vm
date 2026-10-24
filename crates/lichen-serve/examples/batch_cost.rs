//! A batch's warm token: its submissions, its clock, and the tokens it answers.

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
    let rows: usize = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(8);
    let count: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);
    let starts = vec![9707; rows];
    let (before_work, started) = (session.work(), std::time::Instant::now());
    let tokens = match model.generate_batch(&starts, count) {
        Ok(tokens) => tokens,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let elapsed = started.elapsed();
    let after_work = session.work();
    println!("rows {rows}, {} tokens a row", count);
    for (row, tokens) in tokens.iter().enumerate() {
        println!("  row {row}: {tokens:?}");
    }
    println!(
        "submissions {}, fences {}, {:.3} s ({:.3} s a token)",
        after_work.0 - before_work.0,
        after_work.1 - before_work.1,
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() / count as f64
    );
}

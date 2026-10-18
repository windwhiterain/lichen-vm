//! What holding the halves in memory buys a token.

use lichen_serve::{Qwen, Session, Width};

/// The starting token, and how many tokens each run takes.
const START: usize = 9707;
const TOKENS: usize = 2;

/// Holding the halves costs less than reading them again.
///
/// # Invariant
/// The comparison is the **second** token of each run: the first pays for the files either way, and the
/// page cache hides most of the re-read, so only the warm pair says what a copy in this process adds on
/// top of what the operating system was already doing.
#[test]
fn holding_the_bytes_costs_less_than_reading_them() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED holding_the_bytes_costs_less_than_reading_them: no device");
        return;
    };
    let streamed = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED holding_the_bytes_costs_less_than_reading_them: {refusal}");
            return;
        }
    };
    let mut held = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the held model opens: {refusal}"));
    held.hold_in_memory()
        .unwrap_or_else(|refusal| panic!("the halves are held: {refusal}"));
    let reading = streamed
        .generate(START, TOKENS)
        .unwrap_or_else(|refusal| panic!("the streaming model generates: {refusal}"));
    let holding = held
        .generate(START, TOKENS)
        .unwrap_or_else(|refusal| panic!("the held model generates: {refusal}"));
    println!(
        "read at {:.2} s then {:.2} s, held at {:.2} s then {:.2} s",
        reading.per_token[0].as_secs_f64(),
        reading.per_token[1].as_secs_f64(),
        holding.per_token[0].as_secs_f64(),
        holding.per_token[1].as_secs_f64()
    );
    assert_eq!(
        reading.tokens, holding.tokens,
        "the same bytes are the same tokens"
    );
    assert!(
        holding.per_token[1] <= reading.per_token[1],
        "holding the bytes costs no more: {:.2} s against {:.2} s",
        holding.per_token[1].as_secs_f64(),
        reading.per_token[1].as_secs_f64()
    );
}

//! What holding the halves in memory buys a token.

use lichen_serve::{Qwen, Session, Width};

/// The starting token, and how many tokens each run takes.
const START: usize = 9707;
const TOKENS: usize = 2;

/// A model holding the halves answers the tokens a model reading them does.
///
/// # Invariant
/// The timings are printed rather than asserted: a shared machine's page cache and another session's load
/// move a token by seconds, so an assertion on the pair reads as a defect in the model when it is a defect
/// in the afternoon. The tokens are what this test keeps, and no cache can move them.
#[test]
fn holding_the_bytes_answers_the_same_tokens() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED holding_the_bytes_answers_the_same_tokens: no device");
        return;
    };
    let streamed = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED holding_the_bytes_answers_the_same_tokens: {refusal}");
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
}

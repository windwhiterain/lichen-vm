//! What holding the weights on the device buys a token.

use lichen_serve::{Qwen, Session, Width};

/// The starting token, and how many tokens each run takes.
const START: usize = 9707;
const TOKENS: usize = 2;

/// The three tokens the host's greedy walk names, which no residency may move.
const EXPECTED: [usize; 3] = [9707, 25, 358];

/// A model that placed its weights answers the tokens a model that streams them does, for less.
///
/// # Invariant
/// The residency count comes from `LICHEN_RESIDENT_LAYERS` so a card too small for all twenty-eight can be
/// measured at the count it holds: the tokens are asserted and the times are printed, because a timer on a
/// shared machine is an afternoon's weather rather than a property of the model.
#[test]
fn placed_weights_answer_the_same_tokens_for_less() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED placed_weights_answer_the_same_tokens_for_less: no device");
        return;
    };
    let layers: usize = std::env::var("LICHEN_RESIDENT_LAYERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(28);
    let streamed = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED placed_weights_answer_the_same_tokens_for_less: {refusal}");
            return;
        }
    };
    let mut placed = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the placed model opens: {refusal}"));
    placed
        .hold_in_memory()
        .unwrap_or_else(|refusal| panic!("the halves are held: {refusal}"));
    placed
        .reside(layers)
        .unwrap_or_else(|refusal| panic!("{layers} layers are placed: {refusal}"));
    let reading = streamed
        .generate(START, TOKENS)
        .unwrap_or_else(|refusal| panic!("the streamed model generates: {refusal}"));
    let holding = placed
        .generate(START, TOKENS)
        .unwrap_or_else(|refusal| panic!("the placed model generates: {refusal}"));
    println!(
        "{layers} placed: read at {:.2} s then {:.2} s, placed at {:.2} s then {:.2} s",
        reading.per_token[0].as_secs_f64(),
        reading.per_token[1].as_secs_f64(),
        holding.per_token[0].as_secs_f64(),
        holding.per_token[1].as_secs_f64()
    );
    assert_eq!(
        holding.tokens, EXPECTED,
        "the placed walk names the host's tokens"
    );
    assert_eq!(
        reading.tokens, holding.tokens,
        "a placed weight and a streamed one are the same weight"
    );
}

//! Qwen3-1.7B generating two tokens, through the module the binary runs.

mod common;

use lichen_serve::{Qwen, Session};

/// The starting token, and the two the host's greedy loop names after it.
const START: usize = 9707;
const EXPECTED: [usize; 5] = [9707, 25, 358, 2776, 4460];

/// Two tokens generated through the whole model, the second reading the cache the first filled.
///
/// # Invariant
/// The expected tokens are `generate_reference.py`'s own greedy walk over the same twelve tensors, and
/// the second is what proves the cache is read: a loop that attended only to the new position would name
/// something else. The forward itself is the crate's — `Qwen` — so this guards the module the binary runs
/// rather than a copy of it.
#[test]
fn two_tokens_are_generated_through_the_cache() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: no device");
        return;
    };
    let model = match Qwen::open(&session) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED two_tokens_are_generated_through_the_cache: {refusal}");
            return;
        }
    };
    let generated = model
        .generate(START, 4)
        .unwrap_or_else(|refusal| panic!("the model generates: {refusal}"));
    println!("tokens: {:?}", generated.tokens);
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("  token {} took {:.1} s", index + 1, elapsed.as_secs_f64());
    }
    assert_eq!(
        generated.tokens, EXPECTED,
        "the loop names the tokens the host names"
    );
}

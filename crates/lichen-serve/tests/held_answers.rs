//! What a model held in memory answers, which is the path the serving binary runs.

use lichen_serve::{Qwen, Session, Width};

/// The tokens the half-width model answers for the prompt `9707`.
const EXPECTED: [usize; 3] = [9707, 25, 358];

/// A held model answers what the streamed one answers.
///
/// # Invariant
/// The two paths store a layer's weights differently — the held one by column, the streamed one by row —
/// and each is read by the program that matches it, so the same prompt must produce the same tokens.
#[test]
fn a_held_model_answers_what_the_streamed_one_answers() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_held_model_answers_what_the_streamed_one_answers: no device");
        return;
    };
    let mut model = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED: {refusal}");
            return;
        }
    };
    if let Err(refusal) = model.hold_in_memory() {
        eprintln!("SKIPPED: the layers are not held: {refusal}");
        return;
    }
    let generated = model
        .generate(9707, 2)
        .unwrap_or_else(|refusal| panic!("the held model generates: {refusal}"));
    println!("held: {:?}", generated.tokens);
    assert_eq!(
        generated.tokens, EXPECTED,
        "a held model names the host's tokens"
    );
    // The serving binary places its layers without holding them, which is the other layout.
    if let Err(refusal) = model.reside(28) {
        eprintln!("SKIPPED: the layers do not fit: {refusal}");
        return;
    }
    let resident = model
        .generate(9707, 2)
        .unwrap_or_else(|refusal| panic!("the resident model generates: {refusal}"));
    println!("resident: {:?}", resident.tokens);
    assert_eq!(
        resident.tokens, EXPECTED,
        "a model whose layers are placed names the host's tokens"
    );
}

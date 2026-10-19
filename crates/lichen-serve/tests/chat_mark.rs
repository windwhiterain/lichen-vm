//! What a chat template's markers encode to, which decides whether the model sees a turn or its letters.

use lichen_serve::Tokenizer;

/// The token the checkpoint reserves for the end of a turn, and the one for its start.
const END_OF_TURN: usize = 151645;
const START_OF_TURN: usize = 151644;

/// A marker is one token wherever it appears, and nowhere more than once.
///
/// # Invariant
/// A chat turn is a token the model was trained to see. A scan that matches added tokens only at the start
/// of a text encodes a template's markers as the characters that spell them, and a prompt the model reads
/// as the literal word `<|im_end|>` is a different prompt.
#[test]
fn a_marker_is_one_token_wherever_it_appears() {
    let Ok(root) = std::env::var("LICHEN_MODEL_DIR") else {
        eprintln!("SKIPPED a_marker_is_one_token_wherever_it_appears: LICHEN_MODEL_DIR is not set");
        return;
    };
    let tokenizer = match Tokenizer::open(&std::path::Path::new(&root).join("tokenizer.json")) {
        Ok(tokenizer) => tokenizer,
        Err(refusal) => {
            eprintln!("SKIPPED: {refusal}");
            return;
        }
    };
    for text in ["<|im_end|>", "<|im_start|>user\nhello<|im_end|>\n"] {
        let ids = tokenizer
            .encode(text)
            .unwrap_or_else(|refusal| panic!("the text encodes: {refusal}"));
        println!("{text:?} -> {ids:?}");
        assert!(
            ids.contains(&START_OF_TURN) || ids.contains(&END_OF_TURN),
            "a marker is a token: {ids:?}"
        );
        assert!(
            !ids.iter().any(|id| *id == 27),
            "no marker was spelled out as characters: {ids:?}"
        );
    }
    let ids = tokenizer.encode("<|im_end|>").expect("it encodes");
    assert_eq!(ids, vec![END_OF_TURN], "the marker is one token");
}

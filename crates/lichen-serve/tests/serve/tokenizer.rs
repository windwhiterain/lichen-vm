//! The checkpoint's tokenizer, checked against ids worked out from its own vocabulary.

use lichen_serve::Tokenizer;

/// The tokenizer sits beside the weights, which these tests need.
fn tokenizer() -> Option<Tokenizer> {
    let root = match std::env::var("LICHEN_MODEL_DIR") {
        Ok(root) => std::path::PathBuf::from(root),
        Err(_) => return None,
    };
    match Tokenizer::open(&root.join("tokenizer.json")) {
        Ok(tokenizer) => Some(tokenizer),
        Err(refusal) => {
            eprintln!("SKIPPED: {refusal}");
            None
        }
    }
}

/// ASCII text tokenizes to the ids the checkpoint's own merges give it.
///
/// # Invariant
/// The two rows were worked out from `tokenizer.json`'s vocabulary and merge ranks, so an id here is the
/// checkpoint's answer and not this implementation's: `9707` is also the token every measurement in
/// `qwen-inference.md` starts from, which is "Hello".
#[test]
fn ascii_text_tokenizes_to_the_checkpoints_ids() {
    let Some(tokenizer) = tokenizer() else {
        return;
    };
    assert_eq!(
        tokenizer.encode("Hello world").expect("encodes"),
        vec![9707, 1879]
    );
    assert_eq!(
        tokenizer.encode("def f(x): return x + 1").expect("encodes"),
        vec![750, 282, 2075, 1648, 470, 856, 488, 220, 16]
    );
}

/// A special token is one id, and text around it is ordinary.
///
/// # Invariant
/// The added tokens are matched longest first and before the split, because `<|im_start|>` shares a prefix
/// with `<|im_end|>` and the split would cut both into ordinary pieces.
#[test]
fn a_special_token_is_one_id() {
    let Some(tokenizer) = tokenizer() else {
        return;
    };
    let ids = tokenizer.encode("<|im_start|>user").expect("encodes");
    assert_eq!(ids.first().copied(), Some(151644), "im_start is one id");
    assert_eq!(ids.len(), 2, "and the word after it is the second");
}

/// A script whose bytes are not printable tokenizes to the ids the checkpoint's merges give it.
///
/// # Invariant
/// The rows come from a second implementation reading the same vocabulary and merge ranks, so they are the
/// checkpoint's answer: a Chinese character's bytes are mostly continuation bytes, and the table that names
/// them is the one this had wrong while every ASCII row above still passed.
#[test]
fn a_non_ascii_script_tokenizes_to_the_checkpoints_ids() {
    let Some(tokenizer) = tokenizer() else {
        return;
    };
    assert_eq!(
        tokenizer.encode("你好，世界").expect("encodes"),
        vec![108386, 3837, 99489]
    );
    assert_eq!(
        tokenizer.encode("naïve café").expect("encodes"),
        vec![3376, 37572, 586, 51950]
    );
}

/// Text decodes back to itself, whatever script it is written in.
///
/// # Invariant
/// A round trip is the tokenizer's own consistency and needs no reference: bytes go to symbols and symbols
/// back to bytes, and a script the merges handle badly still has to survive the round trip unchanged.
#[test]
fn text_round_trips() {
    let Some(tokenizer) = tokenizer() else {
        return;
    };
    for text in [
        "你好，世界",
        "Hello world",
        "naïve café",
        "def f(x):\n    return x",
    ] {
        let ids = tokenizer.encode(text).expect("encodes");
        assert_eq!(tokenizer.decode(&ids), text, "{text} round trips");
    }
}

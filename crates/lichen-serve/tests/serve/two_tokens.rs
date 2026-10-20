//! The second token's price, which says whether the weight files stay in memory between tokens.

use lichen_serve::{Qwen, Session, Width};

/// The starting token.
const START: usize = 9707;

/// The tokens the narrowed model chooses, so a warm run is still checked against the host's walk.
const EXPECTED: [usize; 3] = [9707, 25, 358];

/// Two tokens at half width, the second reading the files the first already touched.
///
/// # Invariant
/// The pair of times is the measurement: a second token much cheaper than the first is the operating
/// system holding 3.2 GB of halves in the page cache, which is the same saving a copy in this process
/// would buy and costs nothing. Equal times would mean the files are being re-read and a resident copy
/// is the thing to build.
#[test]
fn the_second_token_says_whether_the_files_stay_in_memory() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_second_token_says_whether_the_files_stay_in_memory: no device");
        return;
    };
    let model = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED the_second_token_says_whether_the_files_stay_in_memory: {refusal}");
            return;
        }
    };
    let generated = model
        .generate(START, 2)
        .unwrap_or_else(|refusal| panic!("the narrowed model generates: {refusal}"));
    println!("tokens {:?}", generated.tokens);
    for (index, elapsed) in generated.per_token.iter().enumerate() {
        println!("token {}: {:.2} s", index + 1, elapsed.as_secs_f64());
    }
    assert_eq!(
        generated.tokens, EXPECTED,
        "the narrowed walk names the host's tokens"
    );
    assert!(
        generated.per_token[1] <= generated.per_token[0],
        "a second token is never slower than the first: {:?}",
        generated.per_token
    );
}

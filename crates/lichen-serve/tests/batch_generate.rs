//! A batch answers what each of its sequences answers alone.

use lichen_serve::{Qwen, Session, Width};

/// The two tokens the batch starts from, and how many each generates.
const STARTS: [usize; 1] = [9707];
const TOKENS: usize = 3;

/// A batch's rows are the rows of the sequences it holds.
///
/// # Invariant
/// The comparison is the same model answering the same starts one at a time: a row that carried its own
/// state forward, or borrowed another row's answer, names a token its sequence alone never names.
#[test]
fn a_batch_answers_what_each_sequence_answers_alone() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_batch_answers_what_each_sequence_answers_alone: no device");
        return;
    };
    let mut model = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the narrowed model opens: {refusal}"));
    model
        .hold_in_memory()
        .unwrap_or_else(|refusal| panic!("the halves are held: {refusal}"));
    let batched = model
        .generate_batch(&STARTS, TOKENS)
        .unwrap_or_else(|refusal| panic!("the batch generates: {refusal}"));
    for (index, start) in STARTS.iter().enumerate() {
        let alone = model
            .generate(*start, TOKENS)
            .unwrap_or_else(|refusal| panic!("{start} alone generates: {refusal}"));
        println!(
            "{start} alone {:?}, batched {:?}",
            alone.tokens, batched[index]
        );
        assert_eq!(
            batched[index], alone.tokens,
            "the batch's row {index} answers what {start} answers alone"
        );
    }
}

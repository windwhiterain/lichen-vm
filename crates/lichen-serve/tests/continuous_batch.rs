//! Requests of different lengths advanced together, each at its own position.

use lichen_serve::{Batched, Qwen, Sampling, Session, Width};

/// Two prompts of different lengths, and how many tokens each is asked for.
const PROMPTS: [&[usize]; 2] = [&[9707, 1879], &[9707]];
const TOKENS: usize = 3;

/// A batch answers what each of its requests answers alone, whatever its length.
///
/// # Invariant
/// The two prompts fill different numbers of positions, so the batch's rows stand at different positions in
/// the same step: a row that read another's position would answer a token its request alone never names,
/// which is what a single-position batch could not have told apart.
#[test]
fn a_batch_of_two_lengths_answers_what_each_answers_alone() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_batch_of_two_lengths_answers_what_each_answers_alone: no device");
        return;
    };
    let mut model = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the narrowed model opens: {refusal}"));
    model
        .hold_in_memory()
        .unwrap_or_else(|refusal| panic!("the halves are held: {refusal}"));
    let mut batched = Batched::new(&model);
    let ids: Vec<usize> = PROMPTS
        .iter()
        .map(|prompt| {
            batched
                .admit(prompt, TOKENS, Sampling::greedy())
                .unwrap_or_else(|refusal| panic!("a request is admitted: {refusal}"))
        })
        .collect();
    for _ in 0..TOKENS {
        batched
            .step()
            .unwrap_or_else(|refusal| panic!("the batch steps: {refusal}"));
    }
    for (index, prompt) in PROMPTS.iter().enumerate() {
        let alone = model
            .generate_from(prompt, TOKENS, Sampling::greedy(), |_| {})
            .unwrap_or_else(|refusal| panic!("request {index} alone generates: {refusal}"));
        let wanted = &alone.tokens[prompt.len()..];
        let answered = batched.answer(ids[index]).expect("the request answers");
        println!("{prompt:?} alone {wanted:?}, batched {answered:?}");
        assert_eq!(
            answered, wanted,
            "request {index} answers what it answers alone"
        );
    }
}

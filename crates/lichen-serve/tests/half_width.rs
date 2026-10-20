//! What a token costs when the weights are the checkpoint's halves.

use lichen_serve::{Qwen, Session, Width};

/// The starting token, which both widths read the same row of.
const START: usize = 9707;

/// Halving the weight halves the read, and the read is what a token pays.
///
/// # Invariant
/// Both widths answer a token and the narrowed one is the cheaper; the tokens they choose are printed
/// rather than asserted, because eight mantissa bits move a logit by about a percent and a greedy walk
/// through fifty thousand of them is entitled to change its mind.
#[test]
fn a_token_at_half_width_costs_less_than_at_full_width() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_token_at_half_width_costs_less_than_at_full_width: no device");
        return;
    };
    let wide = match Qwen::open(&session) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED a_token_at_half_width_costs_less_than_at_full_width: {refusal}");
            return;
        }
    };
    let narrow = Qwen::open_with(&session, Width::Narrowed)
        .unwrap_or_else(|refusal| panic!("the narrowed model opens: {refusal}"));
    let full = wide
        .generate(START, 1)
        .unwrap_or_else(|refusal| panic!("the full-width model generates: {refusal}"));
    let half = narrow
        .generate(START, 1)
        .unwrap_or_else(|refusal| panic!("the narrowed model generates: {refusal}"));
    println!(
        "f32: {:?} in {:.1} s, bf16: {:?} in {:.1} s",
        full.tokens,
        full.per_token[0].as_secs_f64(),
        half.tokens,
        half.per_token[0].as_secs_f64()
    );
    assert!(
        half.per_token[0] < full.per_token[0],
        "halving the weight costs less: f32 {:?} against bf16 {:?}",
        full.per_token[0],
        half.per_token[0]
    );
}

//! What a request's sampling does to a row of logits.

use lichen_serve::{Sampling, choose, randomness};

/// A greedy request takes the largest logit, whatever a draw would have given.
///
/// # Invariant
/// The draw always answers zero, which a weighted walk would put on the first candidate after sorting: a
/// sampler that consulted it for greedy would answer that one and not the largest.
#[test]
fn greedy_takes_the_largest_and_never_draws() {
    let logits = [0.1_f32, 2.5, -1.0, 0.4];
    let mut draw = || 0.0_f32;
    assert_eq!(
        choose(&logits, &Sampling::greedy(), &mut draw),
        1,
        "the largest logit is the answer"
    );
}

/// A temperature over one candidate is that candidate, and a seeded run repeats.
///
/// # Invariant
/// Two runs of the same seed answer the same tokens, so a request can be replayed: the draws are the seed's
/// and not the machine's.
#[test]
fn a_top_one_sampler_takes_the_largest_and_a_seed_repeats() {
    let logits = [0.1_f32, 2.5, -1.0, 0.4];
    let sampling = Sampling::sampled(0.7, 1, 42);
    let first: Vec<usize> = {
        let mut draw = randomness(sampling.seed);
        (0..8)
            .map(|_| choose(&logits, &sampling, &mut draw))
            .collect()
    };
    assert_eq!(first, vec![1; 8], "one candidate is the largest logit");
    let second: Vec<usize> = {
        let mut draw = randomness(sampling.seed);
        (0..8)
            .map(|_| choose(&logits, &sampling, &mut draw))
            .collect()
    };
    assert_eq!(first, second, "the same seed answers the same tokens");
}

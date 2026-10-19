//! Choosing the next token from a distribution, which is what a request's sampling asks for.

/// How the next token is chosen.
///
/// # Invariant
/// Greedy is a sampling with temperature zero rather than a branch of its own: a caller reading two code
/// paths for one answer cannot tell which one a request took, and the largest logit is the answer both give.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    /// The temperature: zero is greedy, and above zero divides the logits before they are weighted.
    pub temperature: f32,
    /// How many of the largest logits are kept, or zero for every one of them.
    pub top_k: usize,
    /// The smallest set of the largest candidates whose weight reaches this, or one for all of them.
    pub top_p: f32,
    /// The seed, so a run at a temperature can be asked again and answer the same token.
    pub seed: u64,
}

impl Sampling {
    /// The largest logit, which is the answer a temperature of zero gives.
    pub fn greedy() -> Self {
        Sampling {
            temperature: 0.0,
            top_k: 0,
            top_p: 1.0,
            seed: 0,
        }
    }

    /// A temperature, keeping the `top_k` largest logits.
    pub fn sampled(temperature: f32, top_k: usize, seed: u64) -> Self {
        Sampling {
            temperature,
            top_k,
            top_p: 1.0,
            seed,
        }
    }

    /// The same sampling, keeping the smallest set of candidates whose weight reaches `top_p`.
    ///
    /// # Invariant
    /// One keeps every candidate, so a request that states nothing samples what it also sampled before:
    /// the nucleus is a narrowing of the same walk and never a different one.
    pub fn with_top_p(self, top_p: f32) -> Self {
        Sampling {
            top_p: top_p.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Whether this draws or takes the largest, which is one question rather than a flag to read at each site.
    pub fn is_greedy(&self) -> bool {
        !(self.temperature > 0.0)
    }
}

/// The largest logit's index.
pub fn argmax(values: &[f32]) -> usize {
    let mut best = 0;
    for (index, value) in values.iter().enumerate() {
        if *value > values[best] {
            best = index;
        }
    }
    best
}

/// One token from `logits` under `sampling`, with `draw` advancing the caller's randomness.
///
/// # Invariant
/// Every draw consumes exactly one number from `draw`, whatever the temperature: a sampler that skipped one
/// would make a run's tokens depend on the order its requests happened to arrive in. A logit that is not a
/// number is left out, because a softmax over it would weight the whole row by nothing.
pub fn choose(logits: &[f32], sampling: &Sampling, draw: &mut impl FnMut() -> f32) -> usize {
    if sampling.is_greedy() {
        return argmax(logits);
    }
    let temperature = sampling.temperature as f64;
    let mut order: Vec<usize> = (0..logits.len())
        .filter(|index| logits[*index].is_finite())
        .collect();
    if order.is_empty() {
        return argmax(logits);
    }
    order.sort_by(|left, right| {
        logits[*right]
            .partial_cmp(&logits[*left])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if sampling.top_k > 0 {
        order.truncate(sampling.top_k.min(order.len()));
    }
    let largest = logits[order[0]] as f64;
    let mut weights: Vec<f64> = order
        .iter()
        .map(|index| ((logits[*index] as f64 - largest) / temperature).exp())
        .collect();
    // The nucleus keeps the smallest run holding this much weight, and never fewer than one.
    if sampling.top_p < 1.0 {
        let total: f64 = weights.iter().sum();
        let mut held = 0.0;
        let mut kept = 0;
        for weight in &weights {
            held += weight;
            kept += 1;
            if held / total >= f64::from(sampling.top_p) {
                break;
            }
        }
        order.truncate(kept.max(1));
        weights.truncate(kept.max(1));
    }
    let total: f64 = weights.iter().sum();
    let target = draw() as f64 * total;
    let mut running = 0.0;
    for (position, weight) in weights.iter().enumerate() {
        running += weight;
        if running >= target {
            return order[position];
        }
    }
    order[0]
}

/// A small deterministic generator, so a seeded run repeats.
///
/// # Invariant
/// Its state advances on every call, so a run's draws are independent of each other and depend only on the
/// seed and how many requests preceded them.
pub fn next(state: &mut u64) -> f32 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    let mixed = state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40;
    (mixed as f32) / ((1u32 << 24) as f32)
}

/// A small deterministic generator, so a seeded run repeats.
///
/// # Invariant
/// Its state advances on every call, so a run's draws are independent of each other and depend only on the
/// seed and how many requests preceded them.
pub fn randomness(seed: u64) -> impl FnMut() -> f32 {
    let mut state = seed ^ 0x9E37_79B9_7F4A_7C15;
    move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        ((state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

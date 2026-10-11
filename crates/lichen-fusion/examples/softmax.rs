//! Safe softmax, fused by a derived repair function.
//! Run with `cargo run -p lichen-fusion --example softmax`.
//!
//! # Invariant
//! Every number printed here is produced by evaluating the expressions the engine emitted,
//! never by a second implementation of the same algorithm: the reference is the two-pass
//! form a program would run unfused, and agreement between the two is the claim being made.

use lichen_fusion::{
    Abstraction, BinaryOp, Bindings, Expr, FusedReduction, FusionRefusal, InvertRefusal, Origin,
    Random, Reduction, Variable, derive, derive_with_repair, distributivity, distributivity_sides,
    evaluate, exp, max_relative_disagreement, normalize, relu, square, substitute_all,
};

const SEEDS: [u64; 3] = [1, 2, 3];
const LENGTHS: [usize; 3] = [1, 7, 1024];
const CROSS_CHECK_SEEDS: [u64; 4] = [11, 22, 33, 44];
const CROSS_CHECK_SAMPLES: usize = 256;
/// The agreement the f64 evaluation is held to.
///
/// # Invariant
/// The two forms differ only in the order their terms are summed, so anything above
/// rounding is a derivation that is not exact.
const EXACT_TOLERANCE: f64 = 1e-12;

/// The agreement the same body is held to once every value is rounded through `f32`.
///
/// # Invariant
/// The class a softmax kernel stores its rolling state in, so this is the tolerance the
/// fused body is judged at in the class it would run in.
const F32_TOLERANCE: f64 = 1e-4;

fn main() {
    safe_softmax();
    softmax_with_bias();
    rectified_square_element();
    refused_element_function();
    hand_specified_repair();
    hand_specified_repair_with_flipped_sign();
    refused_consumer_combine();
    println!("\nall assertions passed");
}

/// Case 1: safe softmax, with no custom logic anywhere.
///
/// # Invariant
/// The rolling update printed here is the one the engine derived, and the assertion that
/// it equals the two-pass form is over the emitted assignments rather than over anything
/// written beside them.
fn safe_softmax() {
    let element_function = exp(datum() - producer());
    let fused = derive(maximum_reduction(), element_function, sum_reduction()).expect(
        "a safe softmax's element function inverts, and the repair distributes over addition",
    );

    println!("case 1: safe softmax, op = max, g = exp(d - r), f = add\n--- derived\n{fused}---");
    println!("the element's contribution is the datum itself: no custom logic was in the way\n");

    assert_eq!(
        normalize(&fused.repair.body).expect("the repair function is canonical"),
        normalize(&(repair_argument() * exp(producer() - producer_next())))
            .expect("the closed form is canonical"),
        "the derived repair function is the online-softmax rescale"
    );
    distributivity(&fused.repair, BinaryOp::Add)
        .expect("the repair function distributes over addition");
    check_distributivity_numerically(&fused);

    let mut worst_exact: f64 = 0.0;
    let mut worst_rounded: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let values = pseudo_random_values(seed, length);
            let exact = run(&fused, length, |index| values[index], |_| 0.0, round_as_is);
            let rounded = run(&fused, length, |index| values[index], |_| 0.0, round_as_f32);
            let two_pass = reference(
                length,
                |index| values[index],
                |index| values[index],
                round_as_is,
            );
            let rounded_two_pass = reference(
                length,
                |index| values[index],
                |index| values[index],
                round_as_f32,
            );
            worst_exact = worst_exact.max(relative_gap(exact, two_pass));
            worst_rounded = worst_rounded.max(relative_gap(rounded, rounded_two_pass));
            assert!(
                relative_gap(exact, two_pass) <= EXACT_TOLERANCE,
                "seed {seed}, length {length}: fused {exact} against two-pass {two_pass}"
            );
            assert!(
                relative_gap(rounded, rounded_two_pass) <= F32_TOLERANCE,
                "seed {seed}, length {length}, rounded to f32: fused {rounded} against {rounded_two_pass}"
            );
        }
    }
    println!(
        "end to end over {} seeds and lengths {LENGTHS:?}: worst relative gap {worst_exact:.3e} \
         in f64, {worst_rounded:.3e} once rounded through f32",
        SEEDS.len()
    );
}

/// Case 2: the same softmax with a per-element bias inside the element function.
///
/// # Invariant
/// The bias is bound per element and read as a free variable, exactly as an attention bias
/// or slope would be: the abstraction is shown to have taken `d + bias` as one subexpression,
/// so the custom logic is carried through the derivation rather than around it.
fn softmax_with_bias() {
    let element_function = exp((datum() + bias()) - producer());
    let fused = derive(maximum_reduction(), element_function, sum_reduction())
        .expect("custom logic inside the element's contribution is not an obstacle");

    println!(
        "case 2: the same with a per-element bias, g = exp((d + bias) - r)\n--- derived\n{fused}---"
    );

    assert_eq!(
        normalize(&abstraction_of(&fused).source).expect("the parameter is canonical"),
        normalize(&(datum() + bias())).expect("the closed form is canonical"),
        "the outermost producer-free subexpression is the element's own contribution, bias included"
    );
    assert_eq!(
        normalize(&fused.repair.body).expect("the repair function is canonical"),
        normalize(&(repair_argument() * exp(producer() - producer_next())))
            .expect("the closed form is canonical"),
        "a bias inside the element does not change the repair function"
    );

    let mut worst: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let values = pseudo_random_values(seed, length);
            let biases = pseudo_random_values(seed.wrapping_add(1_000), length);
            let fused_value = run(
                &fused,
                length,
                |index| values[index],
                |index| biases[index],
                round_as_is,
            );
            let two_pass = reference(
                length,
                |index| values[index],
                |index| values[index] + biases[index],
                round_as_is,
            );
            worst = worst.max(relative_gap(fused_value, two_pass));
            assert!(
                relative_gap(fused_value, two_pass) <= EXACT_TOLERANCE,
                "seed {seed}, length {length}: fused {fused_value} against two-pass {two_pass}"
            );
        }
    }
    println!(
        "end to end against the two-pass form over the same seeds and lengths: worst relative gap {worst:.3e}\n"
    );
}

/// Case 3: the element function whose element-side logic is not injective.
///
/// # Invariant
/// This one derives, and the derivation is the point: `c` is the rectified square as a
/// whole, so nothing inside it is ever inverted — only the chain above it, an exponential
/// and a subtraction, is. The numbers agree with the two-pass form over the same seeds.
fn rectified_square_element() {
    let element_function = exp(square(relu(datum())) - producer());
    let fused = derive(maximum_reduction(), element_function, sum_reduction())
        .expect("the chain above the parameter inverts; nothing inside the parameter is touched");

    println!(
        "case 3: g = exp((relu(d))^2 - r), whose element-side logic is not one to one\n--- derived\n{fused}---"
    );

    assert_eq!(
        normalize(&abstraction_of(&fused).source).expect("the parameter is canonical"),
        normalize(&square(relu(datum()))).expect("the closed form is canonical"),
        "the outermost producer-free subexpression is taken whole, rectified square included"
    );
    assert_eq!(
        normalize(&fused.repair.body).expect("the repair function is canonical"),
        normalize(&(repair_argument() * exp(producer() - producer_next())))
            .expect("the closed form is canonical"),
        "a rectified square inside the parameter does not change the repair function"
    );

    let mut worst: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let values = pseudo_random_values(seed, length);
            let fused_value = run(&fused, length, |index| values[index], |_| 0.0, round_as_is);
            let two_pass = reference(
                length,
                |index| values[index],
                |index| rectified_square(values[index]),
                round_as_is,
            );
            worst = worst.max(relative_gap(fused_value, two_pass));
            assert!(
                relative_gap(fused_value, two_pass) <= EXACT_TOLERANCE,
                "seed {seed}, length {length}: fused {fused_value} against two-pass {two_pass}"
            );
        }
    }
    println!(
        "end to end against the two-pass form over the same seeds and lengths: worst relative gap {worst:.3e}\n"
    );
}

/// Case 4: an element function whose chain *above* the parameter is not invertible.
///
/// # Invariant
/// The refusal is the acceptance criterion: `relu` wraps the subtraction, so solving for
/// the datum would mean choosing a branch, and the engine names that operator instead.
fn refused_element_function() {
    let element_function = relu(datum() - producer());
    let refusal = derive(maximum_reduction(), element_function, sum_reduction())
        .err()
        .expect("a rectified subtraction refuses");
    assert!(
        matches!(
            refusal,
            FusionRefusal::NotInvertible {
                cause: InvertRefusal::AmbiguousOperator { .. },
                ..
            }
        ),
        "the refusal names the operator above the parameter, not the consumer"
    );
    println!(
        "case 4: g = relu(d - r), whose chain above the parameter is not one to one\n--- refused\n{}\n",
        refusal.message()
    );
}

/// Case 5: a hand-written repair function, which is what an optimization pack supplies.
///
/// # Invariant
/// No abstraction and no inversion ran, and the emitted body carries the expression that was
/// handed in; what the engine still checked is distributivity and the three slots. The
/// per-element identity is checked here instead, numerically, because it is a claim about a
/// hand-written expression rather than an equation over the inversion table.
fn hand_specified_repair() {
    let element_function = exp(datum() - producer());
    let hand_written = repair_argument() * exp(producer() - producer_next());
    let fused = derive_with_repair(
        maximum_reduction(),
        element_function.clone(),
        sum_reduction(),
        hand_written.clone(),
    )
    .expect("a hand-written repair that distributes over addition is emitted as written");

    println!("case 5: the same g, with h handed in rather than derived\n--- derived\n{fused}---");
    assert!(
        matches!(fused.origin, Origin::HandSpecified),
        "no abstraction and no inversion ran"
    );
    assert_eq!(
        normalize(&fused.repair.body).expect("the repair function is canonical"),
        normalize(&hand_written).expect("the closed form is canonical"),
        "the hand-written repair function is carried into the body as written"
    );
    check_distributivity_numerically(&fused);

    let disagreement = check_repair_identity_numerically(&fused, &element_function);
    assert!(
        disagreement <= EXACT_TOLERANCE,
        "the per-element identity holds to {EXACT_TOLERANCE:.0e}, but disagrees by {disagreement:.3e}"
    );
    println!(
        "per-element identity, checked numerically over {CROSS_CHECK_SAMPLES} points per seed: worst relative gap {disagreement:.3e}"
    );

    let mut worst: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let values = pseudo_random_values(seed, length);
            let fused_value = run(&fused, length, |index| values[index], |_| 0.0, round_as_is);
            let two_pass = reference(
                length,
                |index| values[index],
                |index| values[index],
                round_as_is,
            );
            worst = worst.max(relative_gap(fused_value, two_pass));
            assert!(
                relative_gap(fused_value, two_pass) <= EXACT_TOLERANCE,
                "seed {seed}, length {length}: fused {fused_value} against two-pass {two_pass}"
            );
        }
    }
    println!(
        "end to end against the two-pass form over the same seeds and lengths: worst relative gap {worst:.3e}\n"
    );
}

/// Case 6: a hand-written repair function that is wrong.
///
/// # Invariant
/// The engine cannot tell: the flipped rescale is `x` times a factor, and any such factor
/// distributes over a sum, so the symbolic obligations pass. The per-element identity is
/// what fails, and it fails by an amount no rounding could explain — which is the argument
/// for keeping that obligation on whoever hands the engine a repair function.
fn hand_specified_repair_with_flipped_sign() {
    let element_function = exp(datum() - producer());
    let hand_written = repair_argument() * exp(producer_next() - producer());
    let fused = derive_with_repair(
        maximum_reduction(),
        element_function.clone(),
        sum_reduction(),
        hand_written,
    )
    .expect("distributivity cannot tell a rescale from its own inverse");

    println!(
        "case 6: the same g, with the rescale's sign flipped, h = x * exp(r' - r)\n--- derived\n{fused}---"
    );
    check_distributivity_numerically(&fused);
    println!("distributivity passed, and it would have: every factor distributes over a sum");

    let disagreement = check_repair_identity_numerically(&fused, &element_function);
    assert!(
        disagreement > F32_TOLERANCE,
        "the per-element identity is the obligation that catches a flipped rescale, \
         but it disagrees by only {disagreement:.3e}"
    );
    println!(
        "per-element identity, checked numerically over {CROSS_CHECK_SAMPLES} points per seed: \
         worst relative gap {disagreement:.3e}, against {EXACT_TOLERANCE:.0e} for a repair that holds\n"
    );
}

/// Case 7: an element function that repairs, over a combine it does not repair.
///
/// # Invariant
/// The repair function is the same one safe softmax derived; what changes is the consumer's
/// combine, and the check on distributivity is what refuses it — a repair function that does
/// not distribute cannot repair a consumer reduction made of partial results.
fn refused_consumer_combine() {
    let element_function = exp(datum() - producer());
    let refusal = derive(maximum_reduction(), element_function, product_reduction())
        .err()
        .expect("a repair that rescales does not distribute over multiplication");
    assert!(
        matches!(refusal, FusionRefusal::NotDistributive { .. }),
        "the refusal names the repair function and the combine"
    );
    println!(
        "case 7: the same g, with a consumer whose combine is multiplication\n--- refused\n{}\n",
        refusal.message()
    );
}

/// The abstraction a derivation took, which a hand-specified repair function has none of.
fn abstraction_of(fused: &FusedReduction) -> &Abstraction {
    let Origin::Derived { abstraction, .. } = &fused.origin else {
        panic!("this case derives its repair function");
    };
    abstraction
}

/// The worst relative disagreement between `h(g(d, r), r, r')` and `g(d, r')`, over the
/// cross-check seeds.
///
/// # Invariant
/// Both sides are evaluated at the same points and read the same three slots, so the number
/// reported is the identity's own error and nothing else. This is the obligation the library
/// cannot discharge symbolically: the canonicalizer splits an exponential over a sum but
/// does not cancel the `exp(-r) * exp(r)` the composition leaves behind.
fn check_repair_identity_numerically(fused: &FusedReduction, element_function: &Expr) -> f64 {
    let producer_value = Expr::variable(Variable::PRODUCER);
    let next_producer = Expr::variable(Variable::PRODUCER_NEXT);
    let repaired = fused
        .repair
        .apply(element_function, &producer_value, &next_producer);
    let repaired_element = substitute_all(element_function, &[(Variable::PRODUCER, next_producer)]);
    let mut worst: f64 = 0.0;
    for seed in CROSS_CHECK_SEEDS {
        let gap = max_relative_disagreement(
            &repaired,
            &repaired_element,
            &mut Random::from_seed(seed),
            CROSS_CHECK_SAMPLES,
        )
        .expect("both sides of the repair identity read the same slots");
        worst = worst.max(gap);
    }
    worst
}

/// Checks the distributivity equation numerically, at points drawn from several seeds.
///
/// # Invariant
/// The symbolic check compares two canonical forms and says nothing about the values they
/// take; this one evaluates both at the same points, so a distributivity that holds as an
/// algebraic identity but reads a variable twice would show up here.
fn check_distributivity_numerically(fused: &FusedReduction) {
    let (combined, apart) = distributivity_sides(&fused.repair, fused.consumer.combine);
    for seed in CROSS_CHECK_SEEDS {
        let worst = max_relative_disagreement(
            &combined,
            &apart,
            &mut Random::from_seed(seed),
            CROSS_CHECK_SAMPLES,
        )
        .expect("both sides of the distributivity equation read the same variables");
        assert!(
            worst <= EXACT_TOLERANCE,
            "distributivity seed {seed}: the two forms differ by {worst}"
        );
    }
    println!(
        "distributivity: canonical forms equal, and {} points per seed over seeds {CROSS_CHECK_SEEDS:?} agree to {EXACT_TOLERANCE:.0e}",
        CROSS_CHECK_SAMPLES
    );
}

/// The producer reduction of a safe softmax.
fn maximum_reduction() -> Reduction {
    Reduction {
        combine: BinaryOp::Max,
        identity: f64::NEG_INFINITY,
    }
}

/// The consumer reduction of a safe softmax.
fn sum_reduction() -> Reduction {
    Reduction {
        combine: BinaryOp::Add,
        identity: 0.0,
    }
}

/// A consumer reduction whose combine is not one a rescale distributes over.
fn product_reduction() -> Reduction {
    Reduction {
        combine: BinaryOp::Mul,
        identity: 1.0,
    }
}

/// Runs the emitted loop body over `length` elements and returns the consumer's final state.
///
/// # Invariant
/// The only state the loop carries is what the two reductions declared as their identities,
/// and every value is read through [`evaluate`] on the emitted assignments: a change to the
/// derivation that keeps the printed form and changes the numbers cannot pass here.
fn run(
    fused: &FusedReduction,
    length: usize,
    datum_of: impl Fn(usize) -> f64,
    bias_of: impl Fn(usize) -> f64,
    round: fn(f64) -> f64,
) -> f64 {
    let [producer_step, consumer_step] = fused.steps.as_slice() else {
        panic!("a derived body is the producer's step and the consumer's");
    };
    let mut bindings = Bindings::new();
    bindings.insert(Variable::PRODUCER, round(fused.producer.identity));
    bindings.insert(Variable::CONSUMER_STATE, round(fused.consumer.identity));
    for index in 0..length {
        bindings.insert(Variable::DATUM, round(datum_of(index)));
        bindings.insert(Variable::BIAS, round(bias_of(index)));
        let next_producer = round(
            evaluate(&producer_step.value, &bindings)
                .expect("the producer's step reads carried state"),
        );
        bindings.insert(Variable::PRODUCER_NEXT, next_producer);
        let next_state = round(
            evaluate(&consumer_step.value, &bindings)
                .expect("the consumer's step reads carried state"),
        );
        bindings.insert(Variable::CONSUMER_STATE, next_state);
        bindings.insert(Variable::PRODUCER, next_producer);
    }
    bindings[&Variable::CONSUMER_STATE]
}

/// The two-pass reference an unfused program runs: the datum's maximum, then the sum of the
/// exponentials above it.
///
/// # Invariant
/// The maximum is taken over the datum alone and the bias is not in it, because that is what
/// the declared producer reduction reduces: a bias that the maximum must see is part of the
/// datum the logic hands in, which is what flash attention's row maximum does with a bias
/// folded into the scores.
fn reference(
    length: usize,
    datum_of: impl Fn(usize) -> f64,
    argument_of: impl Fn(usize) -> f64,
    round: fn(f64) -> f64,
) -> f64 {
    let maximum = (0..length)
        .map(|index| round(datum_of(index)))
        .fold(f64::NEG_INFINITY, f64::max);
    (0..length)
        .map(|index| (round(argument_of(index)) - maximum).exp())
        .sum()
}

fn pseudo_random_values(seed: u64, length: usize) -> Vec<f64> {
    let mut random = Random::from_seed(seed);
    (0..length)
        .map(|_| random.next_bounded(-4.0, 8.0))
        .collect()
}

fn relative_gap(value: f64, reference: f64) -> f64 {
    (value - reference).abs() / reference.abs().max(1.0)
}

/// The rectified square of a value, as the element function of case 3 computes it.
fn rectified_square(value: f64) -> f64 {
    let rectified = value.max(0.0);
    rectified * rectified
}

fn round_as_is(value: f64) -> f64 {
    value
}

/// Rounds through `f32`, the class a kernel stores its rolling state in.
fn round_as_f32(value: f64) -> f64 {
    value as f32 as f64
}

fn datum() -> Expr {
    Expr::variable(Variable::DATUM)
}

fn producer() -> Expr {
    Expr::variable(Variable::PRODUCER)
}

fn producer_next() -> Expr {
    Expr::variable(Variable::PRODUCER_NEXT)
}

fn bias() -> Expr {
    Expr::variable(Variable::BIAS)
}

fn repair_argument() -> Expr {
    Expr::variable(Variable::REPAIR_ARGUMENT)
}

//! Qwen3's rotation at the model's own head width.

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{ROPE_STEP, Session};

/// The head the rotation is applied to: Qwen3-1.7B's `head_dim`.
const HEAD: usize = 128;

/// The position the reference was computed at, which is what the angle multiplies.
const AT: i64 = 5;

/// The vector: `x[i] = sin(0.1 i) · 1.5`, the same pattern the reference used.
fn head() -> Vec<f32> {
    (0..HEAD).map(|i| (i as f32 * 0.1).sin() * 1.5).collect()
}

/// Both halves turn, and the pair that turns them is the partner's value.
///
/// # Invariant
/// The first half subtracts its partner and the second adds its own, which is the rotate-half form: a
/// kernel that added on both sides, or paired the wrong way, answers a different number in one half
/// and not the other. The reference is the host's f64 computation, so the `1e-5` covers the `exp` and
/// `ln` the device builds the angle from.
#[test]
fn the_rotation_turns_both_halves_the_hosts_way() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_rotation_turns_both_halves_the_hosts_way: no device");
        return;
    };
    let prepared = session
        .prepare(ROPE_STEP)
        .unwrap_or_else(|refusal| panic!("the rotation prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the turn and the copy"
    );
    let stepped = prepared
        .step(vec![
            RunArgument::Count(HEAD as i64),
            RunArgument::Count(AT),
            floats(&head()),
        ])
        .unwrap_or_else(|refusal| panic!("the rotation runs: {refusal}"));
    let [RunResult::Resident(turned)] = &stepped.results[..] else {
        panic!("the rotation answers with its vector, and this one answered otherwise");
    };
    let answered = read(&session, *turned);
    assert_eq!(answered.len(), HEAD, "the answer is one value per element");
    let first = [
        0.167642793_f32,
        0.155729650,
        -0.247228612,
        -0.687984575,
        -0.935748900,
        -0.952545457,
    ];
    let second = [
        0.049590903_f32,
        -0.319837063,
        -0.496049335,
        -0.303244849,
        0.122114522,
        0.602204551,
    ];
    for (index, wanted) in first.iter().enumerate() {
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    for (offset, wanted) in second.iter().enumerate() {
        let index = 64 + offset;
        assert!(
            (answered[index] - wanted).abs() < 1.0e-5,
            "element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total + 13.775613751).abs() < 1.0e-3,
        "the vector sums to {total} where the host sums to -13.775613751"
    );
    session.release(*turned);
    println!("the rotation at {AT} answered {} at element 0", answered[0]);
}

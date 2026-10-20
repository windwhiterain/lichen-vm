//! SwiGLU's product at the model's own feed-forward width.

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{SWIGLU_STEP, Session};

/// Qwen3-1.7B's `intermediate_size`.
const WIDTH: usize = 6144;

/// The gate's answer: `sin(0.001 i) · 2`, which is the reference's own pattern.
fn gate() -> Vec<f32> {
    (0..WIDTH).map(|i| (i as f32 * 0.001).sin() * 2.0).collect()
}

/// The up projection's answer: `cos(0.002 i) · 1.5`.
fn up() -> Vec<f32> {
    (0..WIDTH).map(|i| (i as f32 * 0.002).cos() * 1.5).collect()
}

/// The product is `silu(gate) · up`, which is neither the gate nor the up.
///
/// # Invariant
/// `silu(x) = x / (1 + exp(-x))`, so a kernel that multiplied the gate by the up without the sigmoid
/// answers a different number everywhere the gate is not zero — which is all but the first element.
/// The reference is the host's f64 computation of the same expression.
#[test]
fn the_product_is_silu_of_the_gate_times_the_up() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_product_is_silu_of_the_gate_times_the_up: no device");
        return;
    };
    let prepared = session
        .prepare(SWIGLU_STEP)
        .unwrap_or_else(|refusal| panic!("the product prepares: {refusal}"));
    assert_eq!(
        prepared.nodes(),
        2,
        "the held step is the product and the copy"
    );
    let stepped = prepared
        .step(vec![
            RunArgument::Count(WIDTH as i64),
            floats(&gate()),
            floats(&up()),
        ])
        .unwrap_or_else(|refusal| panic!("the product runs: {refusal}"));
    let [RunResult::Resident(product)] = &stepped.results[..] else {
        panic!("the product answers with its element, and this one answered otherwise");
    };
    let answered = read(&session, *product);
    assert_eq!(answered.len(), WIDTH, "one value per element");
    let expected = [0.000000000_f32, 0.001501497, 0.003005974, 0.004513412];
    for (index, wanted) in expected.iter().enumerate() {
        assert!(
            (answered[index] - wanted).abs() < 1.0e-6,
            "element {index} answered {} where the host answers {wanted}",
            answered[index]
        );
    }
    let total: f64 = answered.iter().map(|value| *value as f64).sum();
    assert!(
        (total + 1771.646491187).abs() < 1.0e-1,
        "the product sums to {total} where the host sums to -1771.646491187"
    );
    session.release(*product);
    println!(
        "the {WIDTH}-wide product answered {} at element 1",
        answered[1]
    );
}

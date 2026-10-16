//! `ln` spliced into a kernel body answers what its reduction computes at the band's
//! own edges, not only near one.
//!
//! # Invariant
//! The table runs the emitted kernel — compiled, assembled and run through the wasm
//! path — against a model written from the reduction's own algorithm and constants.
//! The model calls nothing in `lichen-kernel-ir`, so agreement is evidence about the
//! kernel body rather than about a function that mirrors it.

mod common;

use lichen_language::testing::float_array;

/// `1/sqrt(2)` as the class holds it: the reduction's closed lower edge.
const BAND_LOW: f32 = 0.707_106_78;
/// `sqrt(2)` as the class holds it: the reduction's open upper edge.
const BAND_HIGH: f32 = 1.414_213_5;
/// The fine phase's own guard: the `2^-4` step's product with the band, `16/sqrt(2)`.
///
/// # Invariant
/// **The fine guard is this, not the band's upper edge**: a `2^-4` step from inside the
/// band leaves it, so a guard on `BAND_HIGH` overshot below for 1,604,117,031 of the
/// positive class (`unary.rs`'s `LN_FINE_EDGE`).
const FINE_EDGE: f32 = BAND_LOW * 16.0;
/// `ln 2` in this class, the track's unit.
const LN_TWO: f32 = std::f32::consts::LN_2;
/// The `atanh` coefficients `1/(2i+1)`, ascending in `z^2`.
const COEFFICIENTS: [f32; 10] = [
    1.0,
    1.0 / 3.0,
    1.0 / 5.0,
    1.0 / 7.0,
    1.0 / 9.0,
    1.0 / 11.0,
    1.0 / 13.0,
    1.0 / 15.0,
    1.0 / 17.0,
    1.0 / 19.0,
];

/// The operand one unit of the last place below `x`, in the class's own order.
fn below(x: f32) -> f32 {
    f32::from_bits(x.to_bits() - 1)
}

/// The three conditional scalings, in the order the body runs them.
///
/// # Invariant
/// **Powers of two, and the track is added to rather than multiplied**, so `k` is the
/// exact integer exponent — it sums `[-32, 1]` per step and never leaves `[-160, 32]`,
/// which the class holds exactly.
fn reduce(mut value: f32) -> (f32, f32) {
    let mut track = 0.0f32;
    for _ in 0..5 {
        if value < BAND_LOW {
            value *= 4_294_967_296.0;
            track -= 32.0;
        }
    }
    for _ in 0..32 {
        if value >= FINE_EDGE {
            value *= 1.0 / 16.0;
            track += 4.0;
        }
    }
    for _ in 0..5 {
        if value >= BAND_HIGH {
            value *= 0.5;
            track += 1.0;
        }
    }
    (value, track)
}

/// Horner over `square`, in the order the polynomial states it.
fn horner(square: f32) -> f32 {
    let last = COEFFICIENTS.len() - 1;
    let mut value = COEFFICIENTS[last];
    for coefficient in COEFFICIENTS[..last].iter().rev() {
        value = value * square + coefficient;
    }
    value
}

/// The reduction and the `atanh` series, as the definition computes them.
fn model(x: f32) -> f32 {
    if !(x > 0.0) {
        return f32::NEG_INFINITY;
    }
    let (value, track) = reduce(x);
    let z = (value - 1.0) / (value + 1.0);
    z * 2.0 * horner(z * z) + track * LN_TWO
}

/// The operands the emitted kernel is run over.
///
/// # Invariant
/// Both band edges with the last `f32` either side of each, the fine phase's own
/// guard, `0.99999994` — the operand §3.3 measured against the rejected `[1, 2)`
/// band — `0.0001` and `2^40` to reach the coarse, fine and unit phases each, and
/// `0.0` and `-1.0` for the guard that answers `-inf` rather than a number.
fn operands() -> Vec<f32> {
    vec![
        1.0,
        0.99999994,
        BAND_LOW,
        below(BAND_LOW),
        BAND_HIGH,
        below(BAND_HIGH),
        FINE_EDGE,
        0.5,
        2.0,
        0.0001,
        1_099_511_627_776.0,
        0.0,
        -1.0,
    ]
}

/// One `compute.parallel` kernel applying `ln` elementwise, and its answers.
fn emitted(operands: &[f32]) -> Vec<f32> {
    // **A negative is written `0.0 - x`**: the language has no unary minus.
    let table: Vec<String> = operands
        .iter()
        .map(|x| {
            if x.is_sign_negative() {
                format!("(0.0 - {:?})", -*x)
            } else {
                format!("{x:?}")
            }
        })
        .collect();
    let source = format!(
        r#"
---
  compute = import "compute.lichen"
---
Args = struct{{.x (compute.Buf Float)}}
Out  = struct{{.v (compute.Buf Float)}}
Par  = compute.P (compute.KT Float)(.I Args, .O Out)
applied = (k : Par) => {{
  i = compute.range k.n
  x = compute.read ((compute.Read _)(.from k.in.x, .at i))
  compute.write ((compute.Write _)(.to k.out.v, .at i, .value (ln x ~ "element")))
}}
kernel = compute.parallel (applied ~ "claim:Elementwise(element)") "cpu"
args = (compute.buffer ([{}] : array{{Float, {}}}) : (compute.Buf Float))
out = (compute.plrun kernel ((compute.A Args)(.n {}, .I Args(.x args))) : Out)
compute.collect out.v : array{{Float, {}}}
"#,
        table.join(", "),
        operands.len(),
        operands.len(),
        operands.len(),
    );
    let (module, value, _root) = common::run(&source);
    let answers = float_array(&module, &value);
    assert_eq!(answers.len(), operands.len(), "every lane wrote its answer");
    answers
}

/// How far `got` is from `want`, in units of the last place of a `f32`.
fn ulps(got: f32, want: f32) -> i64 {
    (i64::from(got.to_bits()) - i64::from(want.to_bits())).abs()
}

/// The emitted body computes the reduction's own value, bit for bit, at every edge.
///
/// # Invariant
/// The comparison is exact, not a tolerance: the body runs the model's sequence, so
/// any gap at all is a different computation. `0.99999994` is here because that is
/// where the rejected band lost a factor of two, and the non-positive lanes are here
/// because the answer is a `Select` — the shape whose arms `floating-point.md` §8
/// records having been listed backwards.
#[test]
fn the_spliced_body_matches_the_reduction_at_every_band_edge() {
    let operands = operands();
    let answers = emitted(&operands);
    for (index, &x) in operands.iter().enumerate() {
        let want = model(x);
        assert_eq!(
            ulps(answers[index], want),
            0,
            "ln({x}) is emitted as {}, the reduction computes {want}",
            answers[index]
        );
    }
}

/// Every positive answer stays beside the logarithm, which is what the band buys.
///
/// # Invariant
/// **Two ulp is the criterion §3.3 measured, and the band is what meets it**: over
/// `[0.5, 2)` a `[1, 2)` reduction's worst relative error is 1.000 and this one's is
/// 2.271e-7. This compares the emitted answer against `f64`, not against a mirror,
/// because a mirror agreeing with `f64` is the failure `floating-point.md` §8 names.
#[test]
fn every_positive_answer_stays_within_two_ulp_of_the_logarithm() {
    let operands = operands();
    let answers = emitted(&operands);
    for (index, &x) in operands.iter().enumerate() {
        if !(x > 0.0) {
            continue;
        }
        let want = f64::from(x).ln() as f32;
        assert!(
            ulps(answers[index], want) <= 2,
            "ln({x}) is emitted as {}, the logarithm is {want}",
            answers[index]
        );
    }
}

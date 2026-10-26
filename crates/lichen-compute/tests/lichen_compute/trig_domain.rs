//! `sin` and `cos` spliced into a kernel body answer over the whole domain their
//! reduction names, not only below `π/4`.
//!
//! # Invariant
//! The table runs the emitted kernel — compiled, assembled and run through the wasm
//! path — against a model written from the reduction's own algorithm and constants.
//! The model calls nothing in `lichen-kernel-ir`, so agreement is evidence about the
//! kernel body rather than about a function that mirrors it.

use super::common;

use lichen_language::testing::float_array;

/// The two-term Cody–Waite split: `π/2` as a multiple of `2^-11`, and what that
/// leaves, rounded once from the real `π/2`.
const HALF_PI_HEAD: f32 = 3217.0 / 2048.0;
const HALF_PI_TAIL: f32 = (std::f64::consts::FRAC_PI_2 - 3217.0 / 2048.0) as f32;
/// `2/π`, the step that turns an angle into a quadrant count.
const INVERSE_TWO_PI: f32 = 0.636_619_75;
/// Where the reduction stops answering for the operand and answers for the clamp.
const ANGLE_LIMIT: f32 = 8192.0;
/// `(−1)^k /(2k+1)!` and `(−1)^k /(2k)!` as this class rounds them, ascending.
const SINE: [f32; 6] = [
    1.0,
    -0.166_666_67,
    0.008_333_333_8,
    -0.000_198_412_7,
    2.755_731_9e-6,
    -2.505_210_8e-8,
];
const COSINE: [f32; 6] = [
    1.0,
    -0.5,
    0.041_666_668,
    -0.001_388_888_9,
    2.480_158_7e-5,
    -2.755_731_9e-7,
];

/// Horner over `r²`, in the order the polynomials state it.
fn horner(coefficients: &[f32], square: f32) -> f32 {
    let last = coefficients.len() - 1;
    let mut value = coefficients[last];
    for coefficient in coefficients[..last].iter().rev() {
        value = value * square + coefficient;
    }
    value
}

/// The reduction, and the two polynomials, as the definitions compute them.
fn model(x: f32) -> (f32, f32) {
    let magnitude = if x < 0.0 { -x } else { x };
    let magnitude = if magnitude <= ANGLE_LIMIT {
        magnitude
    } else {
        ANGLE_LIMIT
    };
    // **Truncation, not rounding**: `f32 as i64` truncates toward zero.
    let quadrants = (magnitude * INVERSE_TWO_PI + 0.5) as i64 as f32;
    let whole = (quadrants * 0.25) as i64 as f32;
    let residue = quadrants - whole * 4.0;
    let r = (magnitude - quadrants * HALF_PI_HEAD) - quadrants * HALF_PI_TAIL;
    let square = r * r;
    let quadrant = residue as i64;
    let reduced_sine = r * horner(&SINE, square);
    let reduced_cosine = horner(&COSINE, square);
    let sine = match quadrant {
        0 => reduced_sine,
        1 => reduced_cosine,
        2 => -reduced_sine,
        _ => -reduced_cosine,
    };
    let cosine = match quadrant {
        0 => reduced_cosine,
        1 => -reduced_sine,
        2 => -reduced_cosine,
        _ => reduced_sine,
    };
    // `cos` is even, and the reduction ran over the magnitude; `sin` is odd.
    if x < 0.0 {
        (-sine, cosine)
    } else {
        (sine, cosine)
    }
}

/// The angles the emitted kernels are run over.
///
/// # Invariant
/// The first two straddle `π/4` to within one `f32` step, which is the edge the
/// reduction's quadrant count changes at and the edge a negative magnitude used to
/// overflow on. `2.4` and `2.5` are the first angles whose quadrant count is `2`, the
/// last count that used to answer. Every negative is one `f32` holds exactly, so the
/// table the kernels read is the table the model reduced.
const ANGLES: &[f32] = &[
    0.125, 0.5, 0.75, 0.7853981, 0.7853982, 1.0, 1.5, 2.0, 2.35, 2.4, 2.5, 3.0, 4.7, 4.712389,
    6.2831855, 10.0, 100.0, 8191.0, 8192.0, 9000.0, -0.25, -2.0, -5.0, -100.5,
];

/// One `compute.parallel` kernel applying `operator` elementwise, and its results.
fn emitted(operator: &str) -> Vec<f32> {
    // **A negative is written `0.0 - x`**: the language has no unary minus.
    let table: Vec<String> = ANGLES
        .iter()
        .map(|a| {
            if a.is_sign_negative() {
                format!("(0.0 - {:?})", -*a)
            } else {
                format!("{a:?}")
            }
        })
        .collect();
    let source = format!(
        r#"
---
  compute = import "compute.lichen"
---
Angles := _x => {{.x (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
Out    := _x => {{.v (compute.Buf (compute.BufParam{{.type Float, .length _}}))}}
Par    := _x => compute.P compute.KT{{.I Angles _, .O Out _}}
applied := (k <: Par) => {{
  i := compute.range k.n
  a := (compute.read (compute.Read{{.from k.in.x, .at i}}) : Float)
  compute.write (compute.Write{{.to k.out.v, .at i, .value ({operator} a ~ "element")}})
}}
kernel := compute.parallel (applied ~ "claim:Elementwise(element)") "cpu" (Out _)
angles := (compute.buffer ([@any : Float, {}][{}] : Array) : (compute.Buf (compute.BufParam{{.type Float, .length _}})))
out := (compute.plrun kernel (compute.A{{.n {}, .I Angles{{.x angles}}}}) : Struct)
compute.collect out.v : Array
"#,
        table.join(", "),
        ANGLES.len(),
        ANGLES.len(),
    );
    let (module, value, _root) = common::run(&source);
    let answers = float_array(&module, &value);
    assert_eq!(answers.len(), ANGLES.len(), "every lane wrote its answer");
    answers
}

/// How far `got` is from `want`, in units of the last place of a `f32`.
fn ulps(got: f32, want: f32) -> i64 {
    (i64::from(got.to_bits()) - i64::from(want.to_bits())).abs()
}

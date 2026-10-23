//! The one definition of each transcendental instruction.
//! See docs/notes/floating-point.md §6, §7.

use std::collections::HashMap;

use crate::{KernelBin, KernelBody, KernelInstr, ScalarClass, Terminator, ValueDef, ValueId};

/// The whole transcendental vocabulary, and the definition of each.
///
/// # Invariant
/// **One list, so the expansion and the instructions cannot disagree**: a transcendental
/// the IR names but this table omits would reach a backend unexpanded, which each emitter
/// refuses by name. See `docs/notes/floating-point.md` §6, §7.
pub const TRANSCENDENTALS: [(KernelInstr, fn() -> KernelBody); 6] = [
    (KernelInstr::Exp, exp_body),
    (KernelInstr::Sqrt, sqrt_body),
    (KernelInstr::Floor, floor_body),
    (KernelInstr::Ln, ln_body),
    (KernelInstr::Sin, sin_body),
    (KernelInstr::Cos, cos_body),
];

/// How many times the polynomial's argument is halved, and therefore how many times its
/// value is squared.
///
/// # Invariant
/// The two counts are one number: `exp(x) = P(x / 2^k)^(2^k)`, so a sequence that halved
/// and squared a different number of times would be a different function. Why seven is
/// where they trade off is `docs/notes/floating-point.md` §6.
const HALVINGS: u32 = 7;

/// The Taylor coefficients `1/n!` of the polynomial, ascending.
///
/// # Invariant
/// The argument is at most `128 / 2^HALVINGS`, and the coefficients are the `f32`
/// roundings of `1/n!`: constants of the language's definition rather than of any target,
/// so both backends read the same bits. The degree was measured, not guessed — the
/// relative error stops improving past it (`docs/notes/floating-point.md` §6).
const COEFFICIENTS: [f32; 11] = [
    1.0,
    1.0,
    0.5,
    0.166_666_67,
    0.041_666_668,
    0.008_333_333,
    0.001_388_889,
    0.000_198_412_7,
    0.000_024_801_588,
    0.000_002_755_732,
    0.000_000_275_573_2,
];

/// The smallest argument the definition answers for; below it `exp` is `0` in `f32`.
///
/// # Invariant
/// The Taylor sum diverges below this, and `exp(-128)` is `1.6e-56`, which the class cannot
/// hold: clamping here is exact rather than an approximation.
const FLOOR: f32 = -128.0;

/// The step a value below one is raised by before any finer one: `2^32`.
///
/// # Invariant
/// The coarse steps must raise every separated value above the fine steps' reach in one
/// step, and there must be enough of them that even `2^-149` clears the work interval:
/// `2^(32 * COARSE_STEPS)` is `2^160`, twice the smallest float's exponent.
const COARSE_STEP: f32 = 4_294_967_296.0;
/// How many coarse steps the reduction runs.
const COARSE_STEPS: u32 = 5;
/// The step a value above the work interval is lowered by: `2^-4`, which is a multiplication
/// rather than a division.
///
/// # Invariant
/// Four is small enough that the largest float descends into `[1, 16)` within
/// `FINE_STEPS` and large enough that the work is not one step per binade. **The step is a
/// power of four**, so the power of two the argument is scaled by is even and the track —
/// which is its square root — is an exact `f32` rather than a rounded one.
const FINE_STEP: f32 = 1.0 / 16.0;
/// The track's share of one coarse step: `2^-16`, the step's square root.
const COARSE_ROOT: f32 = 1.0 / 65_536.0;
/// The track's share of one fine step: `2^2`, the step's square root's reciprocal.
const FINE_ROOT: f32 = 4.0;
/// How many fine steps the reduction runs.
///
/// # Invariant
/// The largest finite float is `2^128`, so it is below `16` after 32 steps and well within
/// this count; the extra steps change nothing for a value already inside the interval, which
/// is the same `select` the earlier ones took.
const FINE_STEPS: u32 = 40;
/// Where Newton's iteration starts: the geometric middle of the work interval `[1, 16)`.
const NEWTON_SEED: f32 = 2.0;
/// How many Newton steps the definition runs.
///
/// # Invariant
/// The reduction leaves the argument in `[1, 16)`, so the seed is within a factor of two of
/// the answer and each step squares the error; six takes the first step's error of a half
/// below the class's own resolution, which is where the measured error stops improving
/// (`docs/notes/floating-point.md` §7).
const NEWTON_STEPS: u32 = 6;

/// The number the exponential's definition computes for `x`.
///
/// # Invariant
/// **The definition, not a reference implementation.** The emitters run [`exp_body`]'s
/// sequence rather than call this, so the two agree because the sequence is this one, and
/// the interpreter has an answer without a backend.
pub fn exp_value(x: f32) -> f32 {
    let clamped = if x < FLOOR { FLOOR } else { x };
    let halved = clamped * (1.0 / (1u32 << HALVINGS) as f32);
    let mut polynomial = COEFFICIENTS[COEFFICIENTS.len() - 1];
    for coefficient in COEFFICIENTS[..COEFFICIENTS.len() - 1].iter().rev() {
        polynomial = polynomial * halved + coefficient;
    }
    let mut powered = polynomial;
    for _ in 0..HALVINGS {
        powered *= powered;
    }
    powered
}

/// A `Float` constant pushed in `block`.
fn constant(body: &mut KernelBody, block: usize, value: f32) -> ValueId {
    body.add_const(block, ScalarClass::Float, f32::to_bits(value) as i64)
}

/// One binary `Float` operation over two values.
fn float_bin(
    body: &mut KernelBody,
    block: usize,
    op: KernelBin,
    lhs: ValueId,
    rhs: ValueId,
) -> ValueId {
    body.add_op(
        block,
        KernelInstr::Bin(ScalarClass::Float, op),
        vec![lhs, rhs],
        vec![ScalarClass::Float],
    )
}

/// The `0`/`1` the language's comparison yields, as the condition width a `Select` reads.
fn condition(
    body: &mut KernelBody,
    block: usize,
    op: KernelBin,
    lhs: ValueId,
    rhs: ValueId,
) -> ValueId {
    let compared = float_bin(body, block, op, lhs, rhs);
    body.add_op(
        block,
        KernelInstr::I32WrapI64,
        vec![compared],
        vec![ScalarClass::Int],
    )
}

/// One `rescale` step: a value outside `[1, 16)` is scaled in by a power of two, and the
/// track takes its square root.
///
/// # Invariant
/// `step` and `track_step` are powers of two, and **the track is multiplied rather than the
/// value divided**: `OpFDiv` is the one float operation this device does not round correctly
/// (`docs/notes/floating-point.md` §7), so no definition may need it. A zero stays where it
/// is, because zero is never below one and never at or above sixteen.
fn rescale(
    body: &mut KernelBody,
    block: usize,
    value: ValueId,
    track: ValueId,
    below: bool,
    step: f32,
    track_step: f32,
) -> (ValueId, ValueId) {
    let edge = constant(body, block, if below { 1.0 } else { 16.0 });
    let comparison = if below { KernelBin::Lt } else { KernelBin::Geq };
    let condition = condition(body, block, comparison, value, edge);
    let factor = constant(body, block, step);
    let scaled = float_bin(body, block, KernelBin::Mul, value, factor);
    let value = body.add_op(
        block,
        KernelInstr::Select,
        vec![scaled, value, condition],
        vec![ScalarClass::Float],
    );
    let factor = constant(body, block, track_step);
    let scaled = float_bin(body, block, KernelBin::Mul, track, factor);
    let track = body.add_op(
        block,
        KernelInstr::Select,
        vec![scaled, track, condition],
        vec![ScalarClass::Float],
    );
    (value, track)
}

/// The square root's definition: a range reduction over exact powers of two, then Newton's
/// iteration.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value — the shape
/// [`KernelBody::expand_transcendentals`] inlines. The reduction leaves the argument in
/// `[1, 16)`, so the seed is within a factor of two and Newton's error squares from there,
/// and the track is the square root of what scaled it, so answer = estimate × track.
/// `docs/notes/floating-point.md` §7 measures it.
pub fn sqrt_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // A non-positive operand answers `0`: the iteration would divide by a seed the
    // reduction pinned to zero.
    let zero = constant(&mut body, entry, 0.0);
    let positive = condition(&mut body, entry, KernelBin::Gt, x, zero);
    let mut reduced = body.add_op(
        entry,
        KernelInstr::Select,
        vec![x, zero, positive],
        vec![ScalarClass::Float],
    );
    let mut track = constant(&mut body, entry, 1.0);

    // Coarse steps first, overshooting inwards: the fine steps then descend a range rather
    // than oscillate in one.
    for _ in 0..COARSE_STEPS {
        (reduced, track) = rescale(
            &mut body,
            entry,
            reduced,
            track,
            true,
            COARSE_STEP,
            COARSE_ROOT,
        );
    }
    for _ in 0..FINE_STEPS {
        (reduced, track) = rescale(
            &mut body, entry, reduced, track, false, FINE_STEP, FINE_ROOT,
        );
    }

    let mut root = constant(&mut body, entry, NEWTON_SEED);
    for _ in 0..NEWTON_STEPS {
        let quotient = float_bin(&mut body, entry, KernelBin::Div, reduced, root);
        let sum = float_bin(&mut body, entry, KernelBin::Add, root, quotient);
        let half = constant(&mut body, entry, 0.5);
        root = float_bin(&mut body, entry, KernelBin::Mul, sum, half);
    }
    let root = float_bin(&mut body, entry, KernelBin::Mul, root, track);
    let root = body.add_op(
        entry,
        KernelInstr::Select,
        vec![root, zero, positive],
        vec![ScalarClass::Float],
    );

    body.set_terminator(entry, Terminator::Return { values: vec![root] });
    body
}

/// The number the square root's definition computes for `x`.
///
/// # Invariant
/// **The definition, not a reference implementation**: the emitters run [`sqrt_body`]'s
/// sequence rather than call this, so the two agree because the sequence is this one, and
/// the interpreter has an answer without a backend.
pub fn sqrt_value(x: f32) -> f32 {
    let mut reduced = if x > 0.0 { x } else { 0.0 };
    let mut track = 1.0f32;
    for _ in 0..COARSE_STEPS {
        (reduced, track) = rescale_value(reduced, track, true, COARSE_STEP, COARSE_ROOT);
    }
    for _ in 0..FINE_STEPS {
        (reduced, track) = rescale_value(reduced, track, false, FINE_STEP, FINE_ROOT);
    }
    let mut root = NEWTON_SEED;
    for _ in 0..NEWTON_STEPS {
        root = 0.5 * (root + reduced / root);
    }
    let root = root * track;
    if x > 0.0 { root } else { 0.0 }
}

/// One [`rescale`] step evaluated in Rust, in the same order and the same arithmetic.
fn rescale_value(value: f32, track: f32, below: bool, step: f32, track_step: f32) -> (f32, f32) {
    let condition = if below { value < 1.0 } else { value >= 16.0 };
    if condition {
        (value * step, track * track_step)
    } else {
        (value, track)
    }
}

/// The angle the sine and cosine definitions reduce to, and the quadrant they reduced by.
///
/// # Invariant
/// **`r` is `|x| − q·(π/2)` in this class and `q` is `⌊|x|·2/π + 1/2⌋`,** so `|r| ≤ π/4`
/// and `m` is `q` modulo four in `[0, 4)`. The two subtractions are **exact** —
/// [`HALF_PI_HEAD`]'s own — which is why the residual does not carry the operand's `ulp`.
fn reduce_angle(body: &mut KernelBody, block: usize, x: ValueId) -> (ValueId, ValueId) {
    // The magnitude, so one reduction serves both.
    let zero = constant(body, block, 0.0);
    let negative = condition(body, block, KernelBin::Lt, x, zero);
    let negated = float_bin(body, block, KernelBin::Sub, zero, x);
    let magnitude = body.add_op(
        block,
        KernelInstr::Select,
        vec![negated, x, negative],
        vec![ScalarClass::Float],
    );
    // Past the domain edge the operand no longer determines the answer.
    let limit = constant(body, block, ANGLE_LIMIT);
    let inside = condition(body, block, KernelBin::Leq, magnitude, limit);
    let magnitude = body.add_op(
        block,
        KernelInstr::Select,
        vec![magnitude, limit, inside],
        vec![ScalarClass::Float],
    );

    let inverse = constant(body, block, INVERSE_TWO_PI);
    let scale = float_bin(body, block, KernelBin::Mul, magnitude, inverse);
    let half = constant(body, block, 0.5);
    let shifted = float_bin(body, block, KernelBin::Add, scale, half);
    let quadrants = truncate(body, block, shifted);

    // `⌊q/4⌋` is `trunc(q/4)`: `q` is not negative, so the round trip already holds it.
    let quarter = constant(body, block, ONE_QUARTER);
    let scaled = float_bin(body, block, KernelBin::Mul, quadrants, quarter);
    let whole = truncate(body, block, scaled);
    let four = constant(body, block, 4.0);
    let four = float_bin(body, block, KernelBin::Mul, whole, four);
    let residue = float_bin(body, block, KernelBin::Sub, quadrants, four);

    let head_value = constant(body, block, HALF_PI_HEAD);
    let head = float_bin(body, block, KernelBin::Mul, quadrants, head_value);
    let tail_value = constant(body, block, HALF_PI_TAIL);
    let tail = float_bin(body, block, KernelBin::Mul, quadrants, tail_value);
    let r = float_bin(body, block, KernelBin::Sub, magnitude, head);
    let r = float_bin(body, block, KernelBin::Sub, r, tail);
    (r, residue)
}

/// The largest argument the angle reduction answers for, `2^13`.
///
/// # Invariant
/// **`q` never passes `5215`**, and `5215 · HALF_PI_HEAD` is `8191.7`, below `2^13` and so
/// a multiple of `2^-11` the class still holds. That is where the exactness of the first
/// subtraction ends, so the domain ends with it.
const ANGLE_LIMIT: f32 = 8192.0;

/// `2/π`, the step that turns an angle into a quadrant count.
///
/// # Invariant
/// The rounded quotient chooses the quadrant, so an argument within about
/// `|x|·2^-24` of a quadrant edge may be reduced one quadrant over.
const INVERSE_TWO_PI: f32 = 0.636_619_75;

/// `π/2` as a multiple of `2^-11`, the head the exact subtraction takes.
///
/// # Invariant
/// **Eleven fraction bits, so `q · HALF_PI_HEAD` is a multiple of `2^-11`**, and every
/// such value below `2^13` is one `f32` holds. That is what makes the multiplication,
/// and the subtraction after it, exact rather than `ulp`-losing.
const HALF_PI_HEAD: f32 = 3217.0 / 2048.0;

/// What the head leaves of `π/2`.
///
/// # Invariant
/// **Rounded once from the real `π/2`, not from this class's own `π/2`**, so the expr
/// sums to `π/2` rather than to the `1.2 × 10^-8` above it that `f32`'s `π/2` states.
const HALF_PI_TAIL: f32 = (std::f64::consts::FRAC_PI_2 - HALF_PI_HEAD as f64) as f32;

/// One quarter, the step that takes a quadrant count to a whole quadrant count.
const ONE_QUARTER: f32 = 0.25;

/// `sin`'s odd coefficients `(−1)^k /(2k+1)!`, ascending, in this class.
///
/// # Invariant
/// The argument is at most `π/4` and these are the `f32` roundings of the signed
/// coefficients, so the first omitted term is `(π/4)^13/13!` — `6.9 × 10^-12`, a
/// hundredth of the class's resolution at one.
const SINE_COEFFICIENTS: [f32; 6] = [
    1.0,
    -0.166_666_67,
    0.008_333_333_8,
    -0.000_198_412_7,
    2.755_731_9e-6,
    -2.505_210_8e-8,
];

/// `cos`'s even coefficients `(−1)^k /(2k)!`, ascending, in this class.
///
/// # Invariant
/// The argument is at most `π/4` and these are the `f32` roundings of the signed
/// coefficients, so the first omitted term is `(π/4)^12/12!` — `1.2 × 10^-10`, and the
/// answer is near one there, so it is a thousandth of the resolution.
const COSINE_COEFFICIENTS: [f32; 6] = [
    1.0,
    -0.5,
    0.041_666_668,
    -0.001_388_888_9,
    2.480_158_7e-5,
    -2.755_731_9e-7,
];

/// A value truncated toward zero and back, over `[0, 2^24)`.
///
/// # Invariant
/// The round trip is the conversion [`floor_body`] performs after its own guard, and
/// a non-negative operand's floor **is** that truncation — so this is the two
/// instructions the guard would keep, not the whole definition.
fn truncate(body: &mut KernelBody, block: usize, value: ValueId) -> ValueId {
    let truncated = body.add_op(
        block,
        KernelInstr::Conv {
            from: ScalarClass::Float,
            to: ScalarClass::Int,
        },
        vec![value],
        vec![ScalarClass::Int],
    );
    body.add_op(
        block,
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        vec![truncated],
        vec![ScalarClass::Float],
    )
}

/// One of the two reduced-angle polynomials at `r²`, Horner over that square.
///
/// # Invariant
/// The coefficients are the class's own roundings and the reduced argument is at most
/// `π/4`, so the answer is the sum both targets' sequences name — no reference `sin`
/// or `cos` stands behind it.
fn polynomial(
    body: &mut KernelBody,
    block: usize,
    square: ValueId,
    coefficients: &[f32],
) -> ValueId {
    let last = coefficients.len() - 1;
    let mut value = constant(body, block, coefficients[last]);
    for coefficient in coefficients[..last].iter().rev() {
        let product = float_bin(body, block, KernelBin::Mul, value, square);
        let addend = constant(body, block, *coefficient);
        value = float_bin(body, block, KernelBin::Add, product, addend);
    }
    value
}

/// The four quadrants' answers, chosen by `m`, each already signed.
///
/// # Invariant
/// **`m` is `[0, 4)`**, so the three tests partition it: `0`, `1`, `2`, and everything
/// left, which is `3`. A fourth test would be unreachable rather than wrong.
fn quadrant(
    body: &mut KernelBody,
    block: usize,
    m: ValueId,
    first: ValueId,
    second: ValueId,
    third: ValueId,
    fourth: ValueId,
) -> ValueId {
    let one = constant(body, block, 1.0);
    let two = constant(body, block, 2.0);
    let three = constant(body, block, 3.0);
    let is_first = condition(body, block, KernelBin::Eq, m, one);
    let is_second = condition(body, block, KernelBin::Eq, m, two);
    let is_third = condition(body, block, KernelBin::Eq, m, three);
    let chosen = body.add_op(
        block,
        KernelInstr::Select,
        vec![second, first, is_first],
        vec![ScalarClass::Float],
    );
    let chosen = body.add_op(
        block,
        KernelInstr::Select,
        vec![third, chosen, is_second],
        vec![ScalarClass::Float],
    );
    body.add_op(
        block,
        KernelInstr::Select,
        vec![fourth, chosen, is_third],
        vec![ScalarClass::Float],
    )
}

/// The sine's definition: a two-term Cody–Waite reduction, then a Horner polynomial.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value — the shape
/// [`KernelBody::expand_transcendentals`] inlines, and named in
/// [`crate::TRANSCENDENTALS`] like every other transcendental. **Every instruction here
/// is one the two backends already emit**, so the expansion is what a target sees.
pub fn sin_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);
    let (r, m) = reduce_angle(&mut body, entry, x);

    let zero = constant(&mut body, entry, 0.0);
    let square = float_bin(&mut body, entry, KernelBin::Mul, r, r);
    let s = polynomial(&mut body, entry, square, &SINE_COEFFICIENTS);
    let c = polynomial(&mut body, entry, square, &COSINE_COEFFICIENTS);
    let s = float_bin(&mut body, entry, KernelBin::Mul, r, s);
    let negative_s = float_bin(&mut body, entry, KernelBin::Sub, zero, s);
    let negative_c = float_bin(&mut body, entry, KernelBin::Sub, zero, c);
    let answer = quadrant(&mut body, entry, m, s, c, negative_s, negative_c);

    // `sin` is odd, and the reduction ran over the magnitude.
    let negative = condition(&mut body, entry, KernelBin::Lt, x, zero);
    let negated = float_bin(&mut body, entry, KernelBin::Sub, zero, answer);
    let answer = body.add_op(
        entry,
        KernelInstr::Select,
        vec![negated, answer, negative],
        vec![ScalarClass::Float],
    );

    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![answer],
        },
    );
    body
}

/// The cosine's definition: the sine's own reduction, without its sign.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value, as [`sin_body`]'s.
pub fn cos_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);
    let (r, m) = reduce_angle(&mut body, entry, x);

    let zero = constant(&mut body, entry, 0.0);
    let square = float_bin(&mut body, entry, KernelBin::Mul, r, r);
    let s = polynomial(&mut body, entry, square, &SINE_COEFFICIENTS);
    let c = polynomial(&mut body, entry, square, &COSINE_COEFFICIENTS);
    let s = float_bin(&mut body, entry, KernelBin::Mul, r, s);
    let negative_s = float_bin(&mut body, entry, KernelBin::Sub, zero, s);
    let negative_c = float_bin(&mut body, entry, KernelBin::Sub, zero, c);
    // Cosine is even, so the magnitude the reduction ran over is the whole answer's.
    let answer = quadrant(&mut body, entry, m, c, negative_s, negative_c, s);

    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![answer],
        },
    );
    body
}

/// The sine and the cosine of `x`, in the order the two definitions compute them.
///
/// # Invariant
/// **The definitions, not reference implementations**: the emitters run [`sin_body`]'s
/// and [`cos_body`]'s sequences rather than call this, so the two agree because the
/// sequences are these ones, and the interpreter has an answer without a backend.
pub fn sin_cos_value(x: f32) -> (f32, f32) {
    let (r, m) = reduce_angle_value(x);
    let square = r * r;
    let s = r * horner(&SINE_COEFFICIENTS, square);
    let c = horner(&COSINE_COEFFICIENTS, square);
    let quadrants = m as i64;
    let sine = match quadrants {
        0 => s,
        1 => c,
        2 => -s,
        _ => -c,
    };
    let cosine = match quadrants {
        0 => c,
        1 => -s,
        2 => -c,
        _ => s,
    };
    if x < 0.0 {
        (-sine, cosine)
    } else {
        (sine, cosine)
    }
}

/// The number the sine's definition computes for `x`.
///
/// # Invariant
/// [`Self::sin_cos_value`]'s first half, as that function's own.
pub fn sin_value(x: f32) -> f32 {
    sin_cos_value(x).0
}

/// The number the cosine's definition computes for `x`.
///
/// # Invariant
/// [`Self::sin_cos_value`]'s second half, as that function's own.
pub fn cos_value(x: f32) -> f32 {
    sin_cos_value(x).1
}

/// [`reduce_angle`] evaluated in Rust, in the same order and the same arithmetic.
///
/// # Invariant
/// The truncation is toward zero over a non-negative value, which is what the two
/// conversions [`truncate`] performs give, and every operand is below `ANGLE_LIMIT`.
fn reduce_angle_value(x: f32) -> (f32, f32) {
    let magnitude = if x < 0.0 { -x } else { x };
    let magnitude = if magnitude <= ANGLE_LIMIT {
        magnitude
    } else {
        ANGLE_LIMIT
    };
    let quadrants = ((magnitude * INVERSE_TWO_PI + 0.5) as i64) as f32;
    let whole = ((quadrants * ONE_QUARTER) as i64) as f32;
    let m = quadrants - whole * 4.0;
    let head = quadrants * HALF_PI_HEAD;
    let tail = quadrants * HALF_PI_TAIL;
    let r = (magnitude - head) - tail;
    (r, m)
}

/// One [`polynomial`] evaluated in Rust, in the same order and the same arithmetic.
fn horner(coefficients: &[f32], square: f32) -> f32 {
    let last = coefficients.len() - 1;
    let mut value = coefficients[last];
    for coefficient in coefficients[..last].iter().rev() {
        value = value * square + coefficient;
    }
    value
}

/// The operand of the sine a body value holds as an inlined definition, or `None`.
///
/// # Invariant
/// **The reader's half of [`sin_body`]**, matched on the same terms as
/// [`inlined_sqrt_operand`]: the whole of that body's instructions and constants, and
/// `None` for anything that is not an instance of it.
pub fn inlined_sin_operand(body: &KernelBody, value: ValueId) -> Option<ValueId> {
    inlined_operand(body, value, &sin_body())
}

/// The operand of the cosine a body value holds as an inlined definition, or `None`.
///
/// # Invariant
/// **The reader's half of [`cos_body`]**, as [`inlined_sin_operand`]'s.
pub fn inlined_cos_operand(body: &KernelBody, value: ValueId) -> Option<ValueId> {
    inlined_operand(body, value, &cos_body())
}

/// The floor's definition: truncation toward zero, corrected for a negative operand.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value — the shape
/// [`KernelBody::expand_transcendentals`] inlines. Every operation is exact
/// (`docs/notes/floating-point.md` §7), and the conversion is total: the guard pins the
/// operand below the integer range, so it never runs on a value it would refuse.
pub fn floor_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // The magnitude, not the operand: `select` evaluates both arms, so a guarded
    // conversion would still meet a negative.
    let zero = constant(&mut body, entry, 0.0);
    let negative = condition(&mut body, entry, KernelBin::Lt, x, zero);
    let negated = float_bin(&mut body, entry, KernelBin::Sub, zero, x);
    let magnitude = body.add_op(
        entry,
        KernelInstr::Select,
        vec![negated, x, negative],
        vec![ScalarClass::Float],
    );
    // Above `2^24` an operand is an integer already and an infinity has no integer to be.
    let resolution = constant(&mut body, entry, 16_777_216.0);
    let small = condition(&mut body, entry, KernelBin::Lt, magnitude, resolution);
    let convertible = body.add_op(
        entry,
        KernelInstr::Select,
        vec![magnitude, zero, small],
        vec![ScalarClass::Float],
    );
    let truncated = body.add_op(
        entry,
        KernelInstr::Conv {
            from: ScalarClass::Float,
            to: ScalarClass::Int,
        },
        vec![convertible],
        vec![ScalarClass::Int],
    );
    let integral = body.add_op(
        entry,
        KernelInstr::Conv {
            from: ScalarClass::Int,
            to: ScalarClass::Float,
        },
        vec![truncated],
        vec![ScalarClass::Float],
    );
    // **`floor(x) = -(|integral| + fraction)`** for a negative operand, `trunc(|x|)` otherwise.
    let kept = float_bin(&mut body, entry, KernelBin::Sub, magnitude, integral);
    let zero = constant(&mut body, entry, 0.0);
    let fraction = condition(&mut body, entry, KernelBin::Gt, kept, zero);
    let one = constant(&mut body, entry, 1.0);
    let widened = float_bin(&mut body, entry, KernelBin::Add, integral, one);
    let lowered = body.add_op(
        entry,
        KernelInstr::Select,
        vec![widened, integral, fraction],
        vec![ScalarClass::Float],
    );
    let negated = float_bin(&mut body, entry, KernelBin::Sub, zero, lowered);
    let answer = body.add_op(
        entry,
        KernelInstr::Select,
        vec![negated, integral, negative],
        vec![ScalarClass::Float],
    );

    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![answer],
        },
    );
    body
}

/// The number the floor's definition computes for `x`.
///
/// # Invariant
/// **The definition, not a reference implementation**: the emitters run [`floor_body`]'s
/// sequence rather than call this, so the two agree because the sequence is this one, and
/// the interpreter has an answer without a backend. The steps are the body's own, in the
/// same order, on the magnitude the conversion can hold.
pub fn floor_value(x: f32) -> f32 {
    let negative = x < 0.0;
    let magnitude = if negative { -x } else { x };
    // Above `2^24` an operand is an integer already and an infinity has no integer to be.
    if !(magnitude < 16_777_216.0) {
        return x;
    }
    let integral = (magnitude as i64) as f32;
    // A non-negative operand's floor is `trunc(|x|)`; a negative one takes one more off
    // when `|x|` had a fraction.
    let fraction = magnitude - integral > 0.0;
    if negative {
        let widened = if fraction { integral + 1.0 } else { integral };
        -widened
    } else {
        integral
    }
}

/// The band the `ln` reduction leaves its argument in: `[1/sqrt(2), sqrt(2))`.
///
/// # Invariant
/// **Round to nearest: the band is what makes the answer usable.** `[1, 2)` lands
/// arguments next to `2`, so the answer is `ln(v) - ln 2` from two terms near `0.693` —
/// worst **relative** error `1.000`, measured. This one bounds `|ln(v)|` by `0.347`, so
/// `k != 0` implies an answer at least that and the addition cannot cancel.
/// §3.3 of `transcendentals-as-operators.md` measures both.
const LN_BAND_LOW: f32 = 0.707_106_78;
const LN_BAND_HIGH: f32 = 1.414_213_5;
/// The edge below which one `2^-4` step would leave the band: `16/sqrt(2)`.
///
/// # Invariant
/// **The fine step's guard is `value >= this`, not `value >= LN_BAND_HIGH`.** A `2^-4`
/// step from inside the band leaves it, and a guard on the band's upper edge overshot
/// below for **1,604,117,031** of the positive class. This is the step size's own
/// product with the band, so the step can never leave it.
const LN_FINE_EDGE: f32 = LN_BAND_LOW * 16.0;

/// The coarse step, the fine step and the unit step, each an exact power of two.
///
/// # Invariant
/// **Powers of two, so the reduction is exact and the track stays exact.** `k` is a
/// `Float` summing these counts and never leaves `[-160, 32]`, which the class holds
/// exactly, so `k` is the true exponent rather than a rounded one.
const LN_COARSE_STEP: f32 = 4_294_967_296.0;
const LN_FINE_STEP: f32 = 1.0 / 16.0;
const LN_UNIT_STEP: f32 = 0.5;

/// How many steps of each kind the reduction runs.
///
/// # Invariant
/// **The smallest counts that reduce every positive class value into the band**,
/// measured exhaustively over all `2,130,706,431` positive `f32`: **0** fell outside and
/// **0** went non-finite. Forty-two conditional scalings, the same order as
/// [`COARSE_STEPS`] and [`FINE_STEPS`] between them.
const LN_COARSE_STEPS: u32 = 5;
const LN_FINE_STEPS: u32 = 32;
const LN_UNIT_STEPS: u32 = 5;

/// `ln 2` in this class, the track's unit.
const LN_TWO: f32 = std::f32::consts::LN_2;

/// `ln`'s `atanh` coefficients `1/(2i+1)`, ascending in `z^2`.
///
/// # Invariant
/// The reduced argument is within `1/sqrt(2)` of one, so `|z| <= 0.1716` and ten terms
/// truncate at a measured `2.9e-10` relative — below a hundredth of the class's own
/// resolution, so this degree is not the limiting term.
const LN_COEFFICIENTS: [f32; 10] = [
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

/// One `ln` reduction step, and the exponent it moved the value by.
///
/// # Invariant
/// **The track is added to, not multiplied**: `k` is the exponent, so it sums exact
/// integers where a scaled track would carry a rounding per step. The value multiplies
/// rather than divides, for the reason [`rescale`] gives.
fn ln_scale(
    body: &mut KernelBody,
    block: usize,
    value: ValueId,
    track: ValueId,
    below: bool,
    edge: f32,
    step: f32,
    track_step: f32,
) -> (ValueId, ValueId) {
    let edge = constant(body, block, edge);
    let comparison = if below { KernelBin::Lt } else { KernelBin::Geq };
    let condition = condition(body, block, comparison, value, edge);
    let factor = constant(body, block, step);
    let scaled = float_bin(body, block, KernelBin::Mul, value, factor);
    let value = body.add_op(
        block,
        KernelInstr::Select,
        vec![scaled, value, condition],
        vec![ScalarClass::Float],
    );
    let count = constant(body, block, track_step);
    let counted = float_bin(body, block, KernelBin::Add, track, count);
    let track = body.add_op(
        block,
        KernelInstr::Select,
        vec![counted, track, condition],
        vec![ScalarClass::Float],
    );
    (value, track)
}

/// `numerator / denominator` as a Markstein step: a reciprocal, a fused seed, the residual, and
/// one correction.
///
/// # Invariant
/// The device does not round `OpFDiv` correctly (floating-point.md §7), so the plain quotient is
/// only the seed. `ln_value` runs the same four operations in the same order, and that is what
/// makes the two backends answer the same bits.
fn compensated_quotient(
    body: &mut KernelBody,
    entry: usize,
    numerator: ValueId,
    denominator: ValueId,
) -> ValueId {
    let one = constant(body, entry, 1.0);
    let zero = constant(body, entry, 0.0);
    let reciprocal = float_bin(body, entry, KernelBin::Div, one, denominator);
    let quotient = body.add_op(
        entry,
        KernelInstr::Fma,
        vec![numerator, reciprocal, zero],
        vec![ScalarClass::Float],
    );
    let negated = body.add_op(
        entry,
        KernelInstr::Negate,
        vec![denominator],
        vec![ScalarClass::Float],
    );
    let residual = body.add_op(
        entry,
        KernelInstr::Fma,
        vec![negated, quotient, numerator],
        vec![ScalarClass::Float],
    );
    body.add_op(
        entry,
        KernelInstr::Fma,
        vec![residual, reciprocal, quotient],
        vec![ScalarClass::Float],
    )
}

/// The logarithm: a reduction into the band, then the `atanh` series and `k ln 2`.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value — the shape
/// [`KernelBody::expand_transcendentals`] inlines. The `(v-1)/(v+1)` division is a compensated
/// quotient over [`KernelInstr::Fma`] that [`ln_value`] also runs, so device and definition agree
/// bit for bit; wasm refuses `Fma` by name, and this body with it.
pub fn ln_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // A non-positive operand answers `-inf`; `select` evaluates both arms, so pin it.
    let zero = constant(&mut body, entry, 0.0);
    let positive = condition(&mut body, entry, KernelBin::Gt, x, zero);
    let floor = constant(&mut body, entry, LN_BAND_LOW);
    let mut value = body.add_op(
        entry,
        KernelInstr::Select,
        vec![x, floor, positive],
        vec![ScalarClass::Float],
    );
    let mut track = constant(&mut body, entry, 0.0);

    for _ in 0..LN_COARSE_STEPS {
        (value, track) = ln_scale(
            &mut body,
            entry,
            value,
            track,
            true,
            LN_BAND_LOW,
            LN_COARSE_STEP,
            -32.0,
        );
    }
    for _ in 0..LN_FINE_STEPS {
        (value, track) = ln_scale(
            &mut body,
            entry,
            value,
            track,
            false,
            LN_FINE_EDGE,
            LN_FINE_STEP,
            4.0,
        );
    }
    for _ in 0..LN_UNIT_STEPS {
        (value, track) = ln_scale(
            &mut body,
            entry,
            value,
            track,
            false,
            LN_BAND_HIGH,
            LN_UNIT_STEP,
            1.0,
        );
    }

    let one = constant(&mut body, entry, 1.0);
    let shifted = float_bin(&mut body, entry, KernelBin::Sub, value, one);
    let raised = float_bin(&mut body, entry, KernelBin::Add, value, one);
    let z = compensated_quotient(&mut body, entry, shifted, raised);
    let square = float_bin(&mut body, entry, KernelBin::Mul, z, z);
    let last = LN_COEFFICIENTS.len() - 1;
    let mut series = constant(&mut body, entry, LN_COEFFICIENTS[last]);
    for coefficient in LN_COEFFICIENTS[..last].iter().rev() {
        let product = float_bin(&mut body, entry, KernelBin::Mul, series, square);
        let addend = constant(&mut body, entry, *coefficient);
        series = float_bin(&mut body, entry, KernelBin::Add, product, addend);
    }
    let two = constant(&mut body, entry, 2.0);
    let doubled = float_bin(&mut body, entry, KernelBin::Mul, z, two);
    let reduced = float_bin(&mut body, entry, KernelBin::Mul, doubled, series);

    let unit = constant(&mut body, entry, LN_TWO);
    let exponent = float_bin(&mut body, entry, KernelBin::Mul, track, unit);
    let answer = float_bin(&mut body, entry, KernelBin::Add, reduced, exponent);

    let infinity = constant(&mut body, entry, f32::NEG_INFINITY);
    let answer = body.add_op(
        entry,
        KernelInstr::Select,
        vec![answer, infinity, positive],
        vec![ScalarClass::Float],
    );

    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![answer],
        },
    );
    body
}

/// The number the logarithm's definition computes for `x`.
///
/// # Invariant
/// **The definition, not a reference implementation**: the emitters run [`ln_body`]'s
/// sequence rather than call this, so the two agree because the sequence is this one,
/// and the interpreter has an answer without a backend. The steps are the body's own,
/// in the same order, on the value the band guard admits.
pub fn ln_value(x: f32) -> f32 {
    if !(x > 0.0) {
        return f32::NEG_INFINITY;
    }
    let (value, track) = ln_reduce_value(x);
    // The body's compensated quotient, in the same order: reciprocal, seed, residual, correction.
    let numerator = value - 1.0;
    let denominator = value + 1.0;
    let reciprocal = 1.0 / denominator;
    let quotient = numerator.mul_add(reciprocal, 0.0);
    let residual = (-denominator).mul_add(quotient, numerator);
    let z = residual.mul_add(reciprocal, quotient);
    let square = z * z;
    let last = LN_COEFFICIENTS.len() - 1;
    let mut series = LN_COEFFICIENTS[last];
    for coefficient in LN_COEFFICIENTS[..last].iter().rev() {
        series = series * square + coefficient;
    }
    2.0 * z * series + track * LN_TWO
}

/// [`ln_body`]'s reduction evaluated in Rust, in the same order and the same arithmetic.
fn ln_reduce_value(x: f32) -> (f32, f32) {
    let mut value = x;
    let mut track = 0.0f32;
    for _ in 0..LN_COARSE_STEPS {
        if value < LN_BAND_LOW {
            value *= LN_COARSE_STEP;
            track -= 32.0;
        }
    }
    for _ in 0..LN_FINE_STEPS {
        if value >= LN_FINE_EDGE {
            value *= LN_FINE_STEP;
            track += 4.0;
        }
    }
    for _ in 0..LN_UNIT_STEPS {
        if value >= LN_BAND_HIGH {
            value *= LN_UNIT_STEP;
            track += 1.0;
        }
    }
    (value, track)
}

/// The exponential's definition: a one-parameter, one-result body over the primitives
/// both backends already emit.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value — the shape
/// [`KernelBody::expand_transcendentals`] inlines. Every operation here is one wasm and
/// SPIR-V already emit identically, which is what makes the two backends' answers the same
/// bits rather than merely the same function.
pub fn exp_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // Clamped, not guarded: `Select` evaluates both arms, and `exp(x) = 0` below the floor
    // is all the class holds.
    let floor = constant(&mut body, entry, FLOOR);
    let below = body.add_op(
        entry,
        KernelInstr::Bin(ScalarClass::Float, KernelBin::Lt),
        vec![x, floor],
        vec![ScalarClass::Float],
    );
    let condition = body.add_op(
        entry,
        KernelInstr::I32WrapI64,
        vec![below],
        vec![ScalarClass::Int],
    );
    let clamped = body.add_op(
        entry,
        KernelInstr::Select,
        vec![floor, x, condition],
        vec![ScalarClass::Float],
    );

    // Horner over the halved argument, then the squarings that undo the halving.
    let divisor = constant(&mut body, entry, 1.0 / (1u32 << HALVINGS) as f32);
    let halved = float_bin(&mut body, entry, KernelBin::Mul, clamped, divisor);
    let last = COEFFICIENTS.len() - 1;
    let mut value = constant(&mut body, entry, COEFFICIENTS[last]);
    for coefficient in COEFFICIENTS[..last].iter().rev() {
        let product = float_bin(&mut body, entry, KernelBin::Mul, value, halved);
        let addend = constant(&mut body, entry, *coefficient);
        value = float_bin(&mut body, entry, KernelBin::Add, product, addend);
    }
    for _ in 0..HALVINGS {
        value = float_bin(&mut body, entry, KernelBin::Mul, value, value);
    }

    body.set_terminator(
        entry,
        Terminator::Return {
            values: vec![value],
        },
    );
    body
}

/// The operand of the exponential a body value holds as an inlined definition, or `None`.
///
/// # Invariant
/// **The reader's half of [`exp_body`]**, matched against that body's own instructions and
/// constants: a subtree that is not an instance of the definition answers `None`, so a change
/// to the definition moves this with it rather than leaving a second copy behind.
pub fn inlined_exp_operand(body: &KernelBody, value: ValueId) -> Option<ValueId> {
    inlined_operand(body, value, &exp_body())
}

/// The operand of the square root a body value holds as an inlined definition, or `None`.
///
/// # Invariant
/// **The reader's half of [`sqrt_body`]**, matched on the same terms as
/// [`inlined_exp_operand`]: the whole of that body's instructions and constants, and `None` for
/// anything that is not an instance of it.
pub fn inlined_sqrt_operand(body: &KernelBody, value: ValueId) -> Option<ValueId> {
    inlined_operand(body, value, &sqrt_body())
}

/// The operand of the logarithm a body value holds as an inlined definition, or `None`.
///
/// # Invariant
/// **The reader's half of [`ln_body`]**, matched on the same terms as
/// [`inlined_sqrt_operand`]: the whole of that body's instructions and constants, and `None`
/// for anything that is not an instance of it.
pub fn inlined_ln_operand(body: &KernelBody, value: ValueId) -> Option<ValueId> {
    inlined_operand(body, value, &ln_body())
}

/// The operand `value` holds as an inlined copy of `definition`, or `None`.
///
/// # Invariant
/// **The whole of `definition`, instruction for instruction**, rebuilt here rather than kept
/// beside it, so no match is against a stale copy. From `value` the walk follows it backwards;
/// every step must be in the body at the mapped value with the same opcode and arity, a value
/// the body holds twice is refused, and every occurrence of the parameter lands on one value.
fn inlined_operand(body: &KernelBody, value: ValueId, definition: &KernelBody) -> Option<ValueId> {
    let source = &definition.blocks[definition.entry];
    let result = match &source.terminator {
        Terminator::Return { values } => *values.first()?,
        _ => return None,
    };
    let produced = match definition.values.get(result.0 as usize)? {
        ValueDef::Instr { op, .. } => *op,
        _ => return None,
    };
    // The definition's own last instruction, so a definition answering another way moves this.
    if !computes(body, value, produced) {
        return None;
    }
    let parameter = *source.params.first()?;
    let mut mapped: HashMap<ValueId, ValueId> = HashMap::new();
    mapped.insert(result, value);
    let mut operand: Option<ValueId> = None;
    // Reverse order, so a step is read from a body value whose readers are already mapped.
    for step in source.instrs.iter().rev() {
        let here = *mapped.get(step)?;
        let ValueDef::Instr { op, args, .. } = definition.values.get(step.0 as usize)? else {
            return None;
        };
        let ValueDef::Instr {
            op: found,
            args: found_args,
            ..
        } = body.values.get(here.0 as usize)?
        else {
            return None;
        };
        if found != op || found_args.len() != args.len() {
            return None;
        }
        for (argument, found_argument) in args.iter().zip(found_args) {
            if *argument == parameter {
                match operand {
                    Some(seen) if seen != *found_argument => return None,
                    _ => operand = Some(*found_argument),
                }
                continue;
            }
            match mapped.insert(*argument, *found_argument) {
                Some(seen) if seen != *found_argument => return None,
                _ => {}
            }
        }
    }
    operand
}

/// Whether `value` is a body value the instruction `op` produces.
fn computes(body: &KernelBody, value: ValueId, op: KernelInstr) -> bool {
    matches!(
        body.values.get(value.0 as usize),
        Some(ValueDef::Instr { op: found, .. }) if *found == op
    )
}

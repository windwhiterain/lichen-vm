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
pub const TRANSCENDENTALS: [(KernelInstr, fn() -> KernelBody); 3] = [
    (KernelInstr::Exp, exp_body),
    (KernelInstr::Sqrt, sqrt_body),
    (KernelInstr::Floor, floor_body),
];

/// How many times the polynomial's argument is halved before it, and therefore how many
/// times its value is squared after.
///
/// # Invariant
/// The two counts are one number: `exp(x) = P(x / 2^k)^(2^k)`, so a sequence that halved
/// and squared a different number of times would be a different function. Seven is where
/// the halving and the polynomial trade against each other: each squaring doubles the
/// polynomial's relative error, so a smaller count needs a higher degree over a wider
/// argument and loses more to the series' cancellation.
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

/// One step of the reduction: a value outside the work interval `[1, 16)` is scaled into it
/// by a power of two, and the track — the square root of what scaled it — takes the matching
/// power.
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
/// `[1, 16)`, so the seed is within a factor of two of the answer and the iteration's error
/// squares from there; the track is the square root of what scaled the argument, so the
/// answer is the estimate times the track and needs no separate unscaling.
/// `docs/notes/floating-point.md` §7 has what the definition measures.
pub fn sqrt_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // A non-positive operand answers `0` whole rather than the iteration's own answer, which
    // for such an operand would divide by a seed the reduction pinned to zero.
    let zero = constant(&mut body, entry, 0.0);
    let positive = condition(&mut body, entry, KernelBin::Gt, x, zero);
    let mut reduced = body.add_op(
        entry,
        KernelInstr::Select,
        vec![x, zero, positive],
        vec![ScalarClass::Float],
    );
    let mut track = constant(&mut body, entry, 1.0);

    // The coarse steps run first and overshoot inwards: each raises a value below one by
    // `2^32`, which leaves the fine steps a range to descend through rather than one to
    // oscillate in.
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

/// The floor's definition: the truncation toward zero, corrected when there was a fraction
/// to drop and the operand was negative.
///
/// # Invariant
/// Straight line, one block, one parameter and one returned value. **Every operation is
/// exact**: `float2int` truncates, `int2float` answers the same integer back for every value
/// below `2^24` — above which the operand is an integer already and the comparison leaves it
/// alone — and the one subtraction is of two adjacent integers. SPIR-V has no core `OpFloor`,
/// so a native emission would mean this definition's work on one target and an extension
/// import on the other; the definition is what both targets and the interpreter read.
/// The conversion is total in the definition's shape: the guard pins the operand below the
/// integer range, so the crossing never runs on a value it would refuse.
/// `docs/notes/floating-point.md` §7.
pub fn floor_body() -> KernelBody {
    let mut body = KernelBody::new();
    let entry = body.add_block();
    let x = body.add_param(entry);

    // **The conversion runs on the magnitude**, because `select` evaluates both arms and
    // `float2int` refuses a negative operand (its destination is the language's unsigned
    // `Int`), so a definition that converted `x` under a guard would refuse every negative —
    // which the wasm backend's trapping conversion does. Negating first is the one shape that
    // is total: the sign is put back by the last `select`, and the truncation's correction is
    // the `ceil` the other side of zero needs.
    let zero = constant(&mut body, entry, 0.0);
    let negative = condition(&mut body, entry, KernelBin::Lt, x, zero);
    let negated = float_bin(&mut body, entry, KernelBin::Sub, zero, x);
    let magnitude = body.add_op(
        entry,
        KernelInstr::Select,
        vec![negated, x, negative],
        vec![ScalarClass::Float],
    );
    // Above `2^24` the operand is an integer already, and an infinity has no integer to be:
    // both are answered as they are, which is also what keeps the conversion off them.
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
    // `magnitude - integral` is the fractional part, and only a negative operand needs it
    // taken off: **`floor(x) = -(|integral| + fraction)`** for a negative `x`, and
    // `trunc(|x|)` — the conversion's own answer — for a non-negative one.
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
    // Above `2^24` the operand is an integer already, and an infinity has no integer to be:
    // both are answered as they are, which is also what keeps the conversion off them.
    if !(magnitude < 16_777_216.0) {
        return x;
    }
    let integral = (magnitude as i64) as f32;
    // `trunc(|x|)` **is** the floor for a non-negative operand; a negative one takes one more
    // off, which is the `ceil` that side needs: `floor(x) = -(|integral| + 1)` when `|x|` had
    // a fraction and `-|integral|` when it did not.
    let fraction = magnitude - integral > 0.0;
    if negative {
        let widened = if fraction { integral + 1.0 } else { integral };
        -widened
    } else {
        integral
    }
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

    // The argument is clamped rather than guarded: `Select` evaluates both arms, and
    // `exp(x) = 0` below the floor is what the class can hold.
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
    let definition = exp_body();
    let source = &definition.blocks[definition.entry];
    let result = match &source.terminator {
        Terminator::Return { values } => *values.first()?,
        _ => return None,
    };
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

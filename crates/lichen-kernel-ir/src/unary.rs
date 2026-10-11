//! The one definition of the transcendental instruction.
//! See docs/notes/floating-point.md §6.

use crate::{KernelBin, KernelBody, KernelInstr, ScalarClass, Terminator, ValueId};

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

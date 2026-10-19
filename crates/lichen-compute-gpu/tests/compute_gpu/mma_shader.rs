//! The cooperative-matrix product: `C = A · B + C` over one subgroup.
//!
//! # Invariant
//! The module's shape is the device's own advertised combination, so the device test is
//! skipped where no device advertises `M16 N16 K32` `s8`→`s32` at subgroup scope rather
//! than run against a guess, and the validator's absence is a skip the same way. The
//! reference is exact integer arithmetic, so the agreement is bit for bit and not a
//! tolerance.

use lichen_compute_gpu::cooperative_matrix::{self, Bindings, K, M, N};

/// The host's reference: `C = A · B + C`, row-major signed bytes and 32-bit integers.
///
/// # Invariant
/// The arithmetic is two's-complement wrapping, which is what the device's integer
/// accumulator does: an order-free exact sum would differ on overflow, and a host that
/// computed one would disagree with the hardware in the last bits.
fn reference(a: &[i8], b: &[i8], c: &mut [i32]) {
    for row in 0..M as usize {
        for column in 0..N as usize {
            let mut sum = c[row * N as usize + column];
            for step in 0..K as usize {
                sum = sum.wrapping_add(
                    i32::from(a[row * K as usize + step])
                        .wrapping_mul(i32::from(b[step * N as usize + column])),
                );
            }
            c[row * N as usize + column] = sum;
        }
    }
}

#[test]
fn the_context_advertises_the_shape_it_can_run() {
    let Some(context) = super::common::context("the_context_advertises_the_shape_it_can_run")
    else {
        return;
    };
    match context.cooperative_matrix() {
        Some(shape) => assert_eq!((shape.m, shape.n, shape.k), (M, N, K)),
        None => eprintln!(
            "SKIPPED the_context_advertises_the_shape_it_can_run: this device advertises \
             no s8 cooperative matrix"
        ),
    }
}

#[test]
fn the_cooperative_matrix_module_validates() {
    let words = cooperative_matrix::module(Bindings {
        set: 0,
        a: 0,
        b: 1,
        c: 2,
    });
    assert!(
        super::spirv_validation::validate("the cooperative matrix module", &words),
        "the cooperative matrix module did not validate"
    );
}

#[test]
fn the_cooperative_matrix_product_matches_the_host() {
    let Some(context) = super::common::context("the_cooperative_matrix_product_matches_the_host")
    else {
        return;
    };
    if context.cooperative_matrix().is_none() {
        eprintln!(
            "SKIPPED the_cooperative_matrix_product_matches_the_host: this device advertises \
             no s8 cooperative matrix"
        );
        return;
    }
    let a: Vec<i8> = (0..(M * K) as usize)
        .map(|index| (index % 5) as i8 - 2)
        .collect();
    let b: Vec<i8> = (0..(K * N) as usize)
        .map(|index| (index % 7) as i8 - 3)
        .collect();
    let c: Vec<i32> = (0..(M * N) as usize).map(|index| index as i32).collect();
    let mut expected = c.clone();
    reference(&a, &b, &mut expected);
    let result = context
        .run_cooperative_matrix(&a, &b, &c)
        .expect("the device advertises the shape, so the product runs");
    assert_eq!(
        result, expected,
        "the device's product disagrees with the host's reference"
    );
}

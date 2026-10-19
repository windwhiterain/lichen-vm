//! The cooperative matrix product answers the same words on both backends.
//!
//! # Invariant
//! The operands are non-negative and small, so the product fits the array reader's words;
//! the device test skips where no device advertises the shape. The host product is the
//! oracle: the arithmetic is exact, so the agreement is bit for bit and not a tolerance.

use super::common;

/// The shape's edges, as the module states them.
const ROWS: usize = 16;
const STEPS: usize = 32;
const COLUMNS: usize = 16;

/// The three operands: `A` (16x32), `B` (32x16) and `C` (16x16), small and non-negative.
fn operands() -> (Vec<i64>, Vec<i64>, Vec<i64>) {
    let a: Vec<i64> = (0..ROWS * STEPS).map(|index| (index % 4) as i64).collect();
    let b: Vec<i64> = (0..STEPS * COLUMNS)
        .map(|index| (index % 3) as i64)
        .collect();
    let c: Vec<i64> = (0..ROWS * COLUMNS)
        .map(|index| (index % 5) as i64)
        .collect();
    (a, b, c)
}

/// The host's product, the oracle both backends are compared with.
fn product(a: &[i64], b: &[i64], c: &[i64]) -> Vec<i64> {
    let mut product = vec![0i64; ROWS * COLUMNS];
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let mut sum = c[row * COLUMNS + column];
            for step in 0..STEPS {
                sum += a[row * STEPS + step] * b[step * COLUMNS + column];
            }
            product[row * COLUMNS + column] = i64::from(sum as i32);
        }
    }
    product
}

/// The program: `compute.mma` over three buffers, collected back to an array.
fn source(backend: &str, a: &[i64], b: &[i64], c: &[i64]) -> String {
    let array = |values: &[i64]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut source =
        String::from("---\n  order = \"30\"\n  compute = import \"compute.lichen\"\n---\n");
    source.push_str(&format!(
        "a = (compute.buffer ([{}] : array{{Int, {}}}))\n",
        array(a),
        a.len()
    ));
    source.push_str(&format!(
        "b = (compute.buffer ([{}] : array{{Int, {}}}))\n",
        array(b),
        b.len()
    ));
    source.push_str(&format!(
        "c = (compute.buffer ([{}] : array{{Int, {}}}))\n",
        array(c),
        c.len()
    ));
    source.push_str(&format!("product = (compute.mma \"{backend}\" a b c)\n"));
    source.push_str(&format!(
        "compute.collect product : array{{Int, {}}}\n",
        ROWS * COLUMNS
    ));
    source
}

/// The observed words, as the signed integers they are.
fn words(
    module: &lichen_lowlevel::Module<lichen_compute::language::LangProgram>,
    value: &lichen_compute::language::LangValue,
) -> Vec<i64> {
    common::usize_array(module, value)
        .into_iter()
        .map(|word| word as i64)
        .collect()
}

#[test]
fn the_cpu_product_matches_the_host() {
    let (a, b, c) = operands();
    let expected = product(&a, &b, &c);
    let (module, value, _root) = common::run(&source("cpu", &a, &b, &c));
    assert_eq!(words(&module, &value), expected);
}

#[test]
fn the_gpu_product_matches_the_host() {
    let (a, b, c) = operands();
    let expected = product(&a, &b, &c);
    let Some((_context, _guard)) = common::device("the_gpu_product_matches_the_host") else {
        return;
    };
    let (module, value, _root) = common::run(&source("gpu", &a, &b, &c));
    assert_eq!(words(&module, &value), expected);
}

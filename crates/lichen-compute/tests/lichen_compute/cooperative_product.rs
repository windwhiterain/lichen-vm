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
    source.push_str(&format!(
        "product = (compute.mma (compute.Mma Int)(.s \"{backend}\", .a a, .b b, .c c))\n"
    ));
    source.push_str(&format!(
        "compute.collect product : array{{Int, {}}}\n",
        ROWS * COLUMNS
    ));
    source
}

/// The program with the product's value under `requirement`.
///
/// # Invariant
/// The requirement is spelled on the product's own annotation, so it is checked
/// against the certificate `compute.mma` states — not against the buffers.
fn source_with_requirement(requirement: &str, a: &[i64], b: &[i64], c: &[i64]) -> String {
    let plain = "(compute.mma (compute.Mma Int)(.s \"cpu\", .a a, .b b, .c c))";
    let program = source("cpu", a, b, c);
    let annotated = format!("({plain} # {requirement})");
    assert!(
        program.contains(plain),
        "the product's expression is the one the requirement wraps"
    );
    program.replace(plain, &annotated)
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

/// A stub computes nothing, so this leg proves the shape of what it answered.
#[test]
fn the_gpu_product_matches_the_host() {
    let (a, b, c) = operands();
    let (_stub, _guard) = common::stub();
    let (module, value, _root) = common::run(&source("gpu", &a, &b, &c));
    let produced = words(&module, &value);
    assert_eq!(
        produced.len(),
        (lichen_kernel_ir::COOPERATIVE_MATRIX_M * lichen_kernel_ir::COOPERATIVE_MATRIX_N) as usize,
        "the device's answer is the whole `M × N` product the trait states"
    );
}

#[test]
fn the_product_is_warp_uniform_not_wider() {
    let (a, b, c) = operands();
    // The product is a per-warp segment: a `# 64` requirement is wider than its
    // certificate, so it is refused.
    let report = common::check(&source_with_requirement("64", &a, &b, &c));
    assert!(!report.ok(), "the product is not 64-lane uniform");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected 64, found 32")),
        "{messages:?}"
    );
}

#[test]
fn a_lane_varying_operand_is_refused() {
    let (a, b, c) = operands();
    // The tile descriptors are warp-uniform (`# warp`); a lane-varying claim is refused.
    let program = source("cpu", &a, &b, &c).replace(".a a", ".a (a # 1)");
    let report = common::check(&program);
    assert!(!report.ok(), "a lane-varying operand must not check");
    let messages: Vec<String> = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expected 32, found 1")),
        "{messages:?}"
    );
}

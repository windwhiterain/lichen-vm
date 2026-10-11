//! The kernel math operators, measured.
//! Run with `cargo run -p lichen-language --example math_operators`.
//!
//! # Invariant
//! Every kernel column comes from the language's own `compute.parallel` over
//! `compute.buffer` inputs, so the two backends run one source and any disagreement is the
//! emitters'. The accuracy column sweeps the `lichen-kernel-ir` definition against the
//! platform's correctly rounded `f32` answer, never a second implementation.
//! `docs/notes/floating-point.md` §7 measures both.

use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The seed of the pseudo-random sweep, so a run is reproducible.
const SWEEP_SEED: u32 = 0x9e37_79b9;
/// How many mantissas are swept per exponent.
const SWEEP_PER_EXPONENT: u32 = 2048;
/// How many elements the kernel columns are run over.
const KERNEL_COUNT: usize = 64;

/// One operator's measured behaviour.
struct Measurement {
    name: &'static str,
    /// How many samples the sweep covered.
    samples: u64,
    /// The worst relative error against the correctly rounded `f32` answer.
    worst: f64,
    /// Where that worst case was.
    worst_at: f32,
    /// How many samples answered the correctly rounded `f32` answer exactly.
    bit_exact: u64,
    /// What the definition answers for `0`, `+inf`, `NaN` and a negative operand.
    edges: Vec<(String, f32)>,
}

fn main() {
    println!("--- the definitions, swept over every exponent of the class");
    println!(
        "{:>6} {:>10} {:>12} {:>13} {:>9}",
        "op", "samples", "worst rel", "at", "bit-exact"
    );
    let measurements = [
        measure("sqrt", lichen_kernel_ir::sqrt_value, f32::sqrt),
        measure("floor", lichen_kernel_ir::floor_value, f32::floor),
    ];
    for measurement in &measurements {
        println!(
            "{:>6} {:>10} {:>12.3e} {:>13.6e} {:>8.2}%",
            measurement.name,
            measurement.samples,
            measurement.worst,
            measurement.worst_at,
            100.0 * measurement.bit_exact as f64 / measurement.samples as f64
        );
    }
    for measurement in &measurements {
        println!("\n{} at the class's edges:", measurement.name);
        for (case, answer) in &measurement.edges {
            println!("  {case:<14} {answer:e}");
        }
    }

    println!("\n--- the same operators as kernels, on both backends");
    // The device column is only claimed where a device is installed; a machine without
    // one says so instead of failing.
    let device = lichen_compute_gpu::install_default();
    let has_device = device.is_ok();
    if let Err(reason) = &device {
        println!("no device is installed, so there is no \"gpu\" column: {reason}");
    }
    let data: Vec<f32> = (0..KERNEL_COUNT)
        .map(|step| (step as f32) * 0.25 + 1.0)
        .collect();
    println!(
        "{:>6} {:>10} {:>12} {:>12} {:>12}",
        "op", "cpu", "definition", "cpu==def", "gpu"
    );
    for (name, operator) in [("sqrt", "sqrt"), ("floor", "floor")] {
        let definitions: Vec<f32> = data
            .iter()
            .map(|&x| match name {
                "sqrt" => lichen_kernel_ir::sqrt_value(x),
                _ => lichen_kernel_ir::floor_value(x),
            })
            .collect();
        let cpu = match kernel_column(operator, "cpu", &data) {
            Ok(values) => values,
            Err(refusal) => {
                println!("{name:>6} the cpu kernel was refused: {refusal}");
                continue;
            }
        };
        let cpu_agrees = cpu
            .iter()
            .zip(&definitions)
            .all(|(kernel, definition)| kernel.to_bits() == definition.to_bits());
        // The device column counts the elements whose bits differ, so a reader knows
        // which one.
        let backend_column = if has_device {
            match kernel_column(operator, "gpu", &data) {
                Ok(values) => {
                    let differing = values
                        .iter()
                        .zip(&cpu)
                        .filter(|(device, host)| device.to_bits() != host.to_bits())
                        .count();
                    if differing == 0 {
                        "identical".to_string()
                    } else {
                        format!("{differing} of {KERNEL_COUNT} differ")
                    }
                }
                Err(refusal) => format!("refused: {refusal}"),
            }
        } else {
            "no device".to_string()
        };
        println!(
            "{name:>6} {:>10.6} {:>12.6} {:>12} {:>12}",
            cpu[KERNEL_COUNT / 2],
            definitions[KERNEL_COUNT / 2],
            cpu_agrees,
            backend_column
        );
    }
}

/// Sweep one operator over every exponent of the class and every sampled mantissa.
///
/// # Invariant
/// The reference is the platform's `f32` answer, which is correctly rounded for both
/// operations, and the domain spans the subnormals (`2^-149`) through `f32::MAX`, so the
/// tail is measured rather than assumed.
fn measure(
    name: &'static str,
    definition: fn(f32) -> f32,
    reference: fn(f32) -> f32,
) -> Measurement {
    let mut worst = 0.0f64;
    let mut worst_at = 0.0f32;
    let mut bit_exact = 0u64;
    let mut samples = 0u64;
    let mut residue = SWEEP_SEED;
    for exponent in -149..=127i32 {
        let base = (2.0f32).powi(exponent);
        for _ in 0..SWEEP_PER_EXPONENT {
            residue = residue.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let mantissa = 1.0 + (residue >> 9) as f32 / (1u32 << 23) as f32;
            let x = base * mantissa;
            if !x.is_finite() || x == 0.0 {
                continue;
            }
            let answer = definition(x);
            let wanted = reference(x);
            samples += 1;
            if answer.to_bits() == wanted.to_bits() {
                bit_exact += 1;
            }
            let relative = ((answer as f64 - wanted as f64) / wanted as f64).abs();
            if relative > worst {
                worst = relative;
                worst_at = x;
            }
        }
    }
    let wanted = |x: f32| reference(x);
    let edges = [
        ("0", 0.0f32),
        ("+inf", f32::INFINITY),
        ("NaN", f32::NAN),
        ("-1", -1.0f32),
    ]
    .into_iter()
    .map(|(case, x)| (case.to_string(), definition(x)))
    .chain(std::iter::once(("reference(1)".to_string(), wanted(1.0))))
    .collect();
    Measurement {
        name,
        samples,
        worst,
        worst_at,
        bit_exact,
        edges,
    }
}

/// One operator's kernel column: the same source, compiled for one backend name.
///
/// # Invariant
/// The program is the language's own `compute.parallel` over a `compute.buffer`, and the
/// operator is written where the language writes it — `sqrt d` — so the column measures the
/// whole pipeline rather than a hand-built fragment.
fn kernel_column(operator: &str, backend: &str, data: &[f32]) -> Result<Vec<f32>, String> {
    let elements = data
        .iter()
        .map(|value| spell(*value))
        .collect::<Vec<_>>()
        .join(", ");
    let source = math_program(operator, backend, &elements, data.len());
    let (module, root) = compile(&source)?;
    let Some(value) = module.node_value(AnyNodeId::Dynamic(root)) else {
        return Err("the kernel column's root is undecided".to_string());
    };
    let Some(LowValue::Array(items)) = value.as_enum() else {
        return Err(format!("the kernel column's root is {value:?}"));
    };
    // SAFETY: the array is the root value the module just evaluated, whose block is alive.
    let items = unsafe { items.items() };
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        let Some(held) = module.node_value(item.node) else {
            return Err("a collected element is undecided".to_string());
        };
        match held.as_enum() {
            Some(LowValue::Float(x)) => values.push(x),
            other => return Err(format!("a collected element is {other:?}")),
        }
    }
    Ok(values)
}

/// The kernel program for one operator and one backend name.
///
/// # Invariant
/// The element expression is the operator applied to the buffer's element, so this file
/// states no arithmetic of its own; the extent is the literal's length, so the read and the
/// launch cannot disagree about how much data there is.
fn math_program(operator: &str, backend: &str, elements: &str, length: usize) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct<.d (compute.Buf Float)>
Out  = struct<.z (compute.Buf Float)>
Par  = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {{
  i = compute.range k.n
  d = compute.read ((compute.Read _)(.from k.in.d, .at i))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value {operator} d))
}}
data = (compute.buffer ([{elements}] : array<Float, {length}>) : (compute.Buf _))
k = compute.parallel f "{backend}"
out = (compute.plrun k ((compute.A In)(.n {length}, .I In(.d data))) : Out)
compute.collect out.z
"#
    )
}

/// One `f32` as an expression the source lexer reads.
///
/// # Invariant
/// The lexer's float literal is `[0-9]+.[0-9]+` and there is no unary minus, so a whole value
/// keeps its `.0` and a negative one is `0.0 - x`. The data is a multiple of `0.25` and
/// bounded, so its shortest spelling has no exponent and is the value itself rather than a
/// rounding of it.
fn spell(value: f32) -> String {
    let magnitude = value.abs();
    let mut digits = format!("{magnitude}");
    if !digits.contains('.') {
        digits.push_str(".0");
    }
    if value < 0.0 {
        format!("(0.0 - {digits})")
    } else {
        digits
    }
}

/// Compile and run the source, keeping the module and its root value.
fn compile(source: &str) -> Result<(Module<LangProgram>, NodeId), String> {
    let mut store = PackageStore::<LangProgram>::new();
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    if !diags.is_empty() {
        return Err(diags
            .into_iter()
            .map(|diag| diag.message)
            .collect::<Vec<_>>()
            .join("; "));
    }
    let line_starts = lichen_language::lex::line_starts(source);
    let report = lichen_language::compile_with_imports_at::<LangProgram>(
        preprocessed.code,
        &preprocessed.imports,
        Some(store.registry()),
        preprocessed.code_base,
        &line_starts,
        lichen_highlevel::no_native_ops(),
    );
    if !report.ok() {
        return Err(report
            .diagnostics
            .into_iter()
            .map(|diag| diag.message)
            .collect::<Vec<_>>()
            .join("; "));
    }
    let Some(build) = report.build else {
        return Err("the build produced no module".to_string());
    };
    let mut module = build.module;
    let root = build.root_val;
    module
        .evaluate_node_deep(root, None)
        .ok_or_else(|| "the program's root value is undecided".to_string())?;
    let refused: Vec<String> = module
        .extension_diagnostics
        .iter()
        .map(|entry| format!("{}: {}", entry.category, entry.message))
        .collect();
    if !refused.is_empty() {
        return Err(refused.join("; "));
    }
    Ok((module, root))
}

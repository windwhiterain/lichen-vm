//! The fusion chain, run numerically.
//! Run with `cargo run -p lichen-language --example fusion_numbers`.
//!
//! # Invariant
//! Every column derives from one element function and one pair of declared reductions, and
//! every host number is [`lichen_fusion::evaluate`] over a tree the engine or this file
//! built, never a second implementation: the columns are the two-pass form, the emitted
//! rolling update, and the same chain as unfused `compute.parallel` kernels.
//! `docs/notes/graph-fusion.md` §6.

use lichen_fusion::{
    BinaryOp, Bindings, Expr, FusedReduction, Random, Reduction, Variable, derive, evaluate,
    max_relative_disagreement, substitute_all,
};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, LowValue, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The seeds and lengths every case is run over, so no result is one point.
const SEEDS: [u64; 3] = [1, 2, 3];
const LENGTHS: [usize; 4] = [1, 4, 9, 64];

/// The agreement the two host forms are held to.
const TOLERANCE: f64 = 1e-12;

/// The agreement the kernel column is held to.
///
/// # Invariant
/// A kernel computes in `f32` (`ScalarClass::Float`) and the host forms in `f64`, so the
/// kernel column can agree with them only to the class's own width: the sum accumulates one
/// rounding per element and every consumer term shifts by that one error, which leaves the
/// `max`'s argmax alone.
const KERNEL_TOLERANCE: f64 = 1e-4;

fn main() {
    println!("\n--- the chain");
    println!(
        "producer: sum over the data, identity 0;  consumer: max over g(d, r) = d - r, identity \
         -inf"
    );

    let element = Expr::variable(Variable::DATUM) - Expr::variable(Variable::PRODUCER);
    println!("element function g(d, r) = {element}");

    println!("\n--- the fusion the engine derives");
    let fused = match derive(
        Reduction {
            combine: BinaryOp::Add,
            identity: 0.0,
        },
        element.clone(),
        Reduction {
            combine: BinaryOp::Max,
            identity: f64::NEG_INFINITY,
        },
    ) {
        Ok(fused) => {
            println!("{fused}---");
            fused
        }
        Err(refusal) => {
            println!("refused: {}", refusal.message());
            return;
        }
    };
    let identity = repair_identity(&fused, &element);
    println!(
        "per-element identity h(g(d, r), r, r') = g(d, r'), checked numerically over {} points \
         per seed: worst relative gap {identity:.3e}",
        SEEDS.len() * 256
    );

    println!("\n--- the three forms over the same data");
    println!(
        "{:>5} {:>6}  {:>14} {:>14} {:>14} {:>9} {:>9}",
        "seed", "length", "two pass", "fused", "kernels", "host gap", "kernel gap"
    );
    let mut worst: f64 = 0.0;
    let mut worst_kernel: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let data = pseudo_random(seed, length);
            let two_pass = two_pass(&fused, &element, &data);
            let fused_value = rolling(&fused, &data);
            let gap = relative_gap(two_pass, fused_value);
            worst = worst.max(gap);
            match kernel_column(&data) {
                Ok(kernels) => {
                    let kernel_gap = relative_gap(two_pass, kernels);
                    worst_kernel = worst_kernel.max(kernel_gap);
                    println!(
                        "{seed:>5} {length:>6}  {two_pass:>14.6} {fused_value:>14.6} \
                         {kernels:>14.6} {gap:>9.2e} {kernel_gap:>9.2e}"
                    );
                }
                Err(refusal) => {
                    println!(
                        "{seed:>5} {length:>6}  {two_pass:>14.6} {fused_value:>14.6} \
                         {:>14} {gap:>9.2e} {refusal}",
                        "refused"
                    );
                }
            }
        }
    }
    println!(
        "\nworst host gap over {} cases: {worst:.3e}, against {TOLERANCE:.0e}",
        SEEDS.len() * LENGTHS.len()
    );
    if worst > TOLERANCE {
        println!("the two host forms disagree above the tolerance");
    }
    println!(
        "worst kernel gap over {} cases: {worst_kernel:.3e}, against {KERNEL_TOLERANCE:.0e}",
        SEEDS.len() * LENGTHS.len()
    );
    if worst_kernel > KERNEL_TOLERANCE {
        println!("the kernels disagree with the host forms above the class's width");
    }

    println!("\n--- and the reduction the producer's own marks declare");
    report_marks();
}

/// The relative gap between one number and the reference, with the reference guarded at one.
fn relative_gap(reference: f64, value: f64) -> f64 {
    (reference - value).abs() / reference.abs().max(1.0)
}

/// The two-pass form: the producer's whole value first, then the element function at it.
///
/// # Invariant
/// The producer's value is the combine fold over every datum, and the consumer then reduces
/// the element function evaluated at that one value twice over: once for the producer, once
/// for the element — so this is the form that stores what the fusion removes, and it reads
/// the element function rather than restating it.
fn two_pass(fused: &FusedReduction, element: &Expr, data: &[f64]) -> f64 {
    let Some(total) = data
        .iter()
        .copied()
        .reduce(|left, right| combine(fused.producer.combine, left, right))
    else {
        return fused.consumer.identity;
    };
    let mut bindings = Bindings::new();
    bindings.insert(Variable::PRODUCER, total);
    data.iter()
        .filter_map(|datum| {
            bindings.insert(Variable::DATUM, *datum);
            evaluate(element, &bindings)
        })
        .fold(fused.consumer.identity, |state, term| {
            combine(fused.consumer.combine, state, term)
        })
}

/// The loop body the engine emitted, evaluated over the data.
///
/// # Invariant
/// The state starts at the two declared identities and every value is read through
/// [`evaluate`] on an emitted assignment, so a change to the derivation that keeps the
/// printed form and changes the numbers cannot pass here.
fn rolling(fused: &FusedReduction, data: &[f64]) -> f64 {
    let [producer_step, consumer_step] = fused.steps.as_slice() else {
        return f64::NAN;
    };
    let mut bindings = Bindings::new();
    bindings.insert(Variable::PRODUCER, fused.producer.identity);
    bindings.insert(Variable::CONSUMER_STATE, fused.consumer.identity);
    for datum in data {
        bindings.insert(Variable::DATUM, *datum);
        let Some(next) = evaluate(&producer_step.value, &bindings) else {
            return f64::NAN;
        };
        bindings.insert(Variable::PRODUCER_NEXT, next);
        let Some(state) = evaluate(&consumer_step.value, &bindings) else {
            return f64::NAN;
        };
        bindings.insert(Variable::CONSUMER_STATE, state);
        bindings.insert(Variable::PRODUCER, next);
    }
    bindings[&Variable::CONSUMER_STATE]
}

/// The same two-pass algorithm as two parallel kernels, run unfused on the cpu backend.
///
/// # Invariant
/// The producer is one `@loop` fold over the whole buffer and the consumer another over
/// `max (d - r)`, with `r` read back from the producer's output buffer: the algorithm the
/// two-pass column states, in the kernel IR, and no third implementation of it.
fn kernel_column(data: &[f64]) -> Result<f64, String> {
    let (module, root) = compile(&kernel_source(data))
        .map_err(|messages| format!("the program was refused: {}", messages.join("; ")))?;
    match module
        .node_value(AnyNodeId::Dynamic(root))
        .and_then(|value| AsEnum::<LowValue>::as_enum(&value))
    {
        Some(LowValue::Float(value)) => Ok(f64::from(value)),
        other => Err(format!("the kernel column's root is {other:?}")),
    }
}

/// The source of the two-kernel program over `data`.
///
/// # Invariant
/// The data is spelled into the source because a buffer's ingress is an array literal
/// (`compute.buffer`); the extent `.n` is that literal's length, so the producer's fold and
/// the launch cannot disagree about how much data there is.
fn kernel_source(data: &[f64]) -> String {
    let length = data.len();
    let elements = data
        .iter()
        .map(|value| spell(*value as f32))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct<.d (compute.Buf Float)>
OutR = struct<.r (compute.Buf Float)>
Par  = compute.P (compute.KT _)(.I In, .O OutR)
producer = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0)))
}}
prod = compute.parallel producer "cpu"
In2  = struct<.d (compute.Buf Float), .p (compute.Buf Float)>
OutZ = struct<.z (compute.Buf Float)>
Par2 = compute.P (compute.KT _)(.I In2, .O OutZ)
consumer = (k : Par2) => {{
  i = compute.range k.n
  r = compute.read ((compute.Read _)(.from k.in.p, .at 0))
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, max (compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) - r) s(1))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, (0.0 - 1000000.0))))
}}
cons = compute.parallel consumer "cpu"
data  = (compute.buffer ([{elements}] : array<Float, {length}>) : (compute.Buf _))
total = (compute.plrun prod ((compute.A In)(.n {length}, .I In(.d data))) : OutR)
worst = (compute.plrun cons ((compute.A In2)(.n {length}, .I In2(.d data, .p total.r))) : OutZ)
compute.read ((compute.Read _)(.from worst.z, .at 0))
"#
    )
}

/// One `f32` as an expression the source lexer reads.
///
/// # Invariant
/// The lexer's float literal is `[0-9]+\.[0-9]+` and there is no unary minus, so a whole value
/// keeps its `.0` and a negative one is `0.0 - x`. The data is a multiple of `1/16`, so its
/// shortest spelling has no exponent and is the value itself rather than a rounding of it.
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

/// One combine over two values, in the operator the reduction declares.
fn combine(operator: BinaryOp, left: f64, right: f64) -> f64 {
    match operator {
        BinaryOp::Add => left + right,
        BinaryOp::Sub => left - right,
        BinaryOp::Mul => left * right,
        BinaryOp::Div => left / right,
        BinaryOp::Max => left.max(right),
        BinaryOp::Min => left.min(right),
        _ => f64::NAN,
    }
}

/// A fixed pseudo-random data array, the same at every run.
///
/// # Invariant
/// Every value is the `f32` the host holds and the shortest decimal string that rounds
/// back to it, so the source spells the number the host computes over. Bounded magnitudes
/// keep that spelling out of the exponent form the lexer has no literal for, and the values
/// are deliberately **not** exact in `f32`, so the kernel column compares two arithmetic
/// classes rather than two spellings.
fn pseudo_random(seed: u64, length: usize) -> Vec<f64> {
    let mut random = Random::from_seed(seed);
    (0..length)
        .map(|_| {
            let step = (random.next_bounded(-4.0, 8.0) * 16.0).round();
            f64::from(((step + 0.1) / 16.0) as f32)
        })
        .collect()
}

/// The worst relative disagreement between the repair applied to an element and the element
/// at the producer's next value.
fn repair_identity(fused: &FusedReduction, element: &Expr) -> f64 {
    let repaired = fused.repair.apply(
        element,
        &Expr::Variable(Variable::PRODUCER),
        &Expr::Variable(Variable::PRODUCER_NEXT),
    );
    let next_element = substitute_all(
        element,
        &[(Variable::PRODUCER, Expr::Variable(Variable::PRODUCER_NEXT))],
    );
    SEEDS
        .iter()
        .map(|seed| {
            max_relative_disagreement(&repaired, &next_element, &mut Random::from_seed(*seed), 256)
                .unwrap_or(f64::INFINITY)
        })
        .fold(0.0, f64::max)
}

/// Compile and run the source, keeping the module and its root value.
fn compile(source: &str) -> Result<(Module<LangProgram>, NodeId), Vec<String>> {
    let mut store = PackageStore::<LangProgram>::new();
    let (preprocessed, diags) = lichen_language::preprocess::preprocess(source, None, &mut store);
    if !diags.is_empty() {
        return Err(diags.into_iter().map(|diag| diag.message).collect());
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
            .collect());
    }
    let Some(build) = report.build else {
        return Err(vec!["the build produced no module".to_string()]);
    };
    let mut module = build.module;
    let root = build.root_val;
    module
        .evaluate_node_deep(root, None)
        .ok_or_else(|| vec!["the program's root value is undecided".to_string()])?;
    let refused: Vec<String> = module
        .extension_diagnostics
        .iter()
        .map(|entry| format!("{}: {}", entry.category, entry.message))
        .collect();
    if !refused.is_empty() {
        return Err(refused);
    }
    Ok((module, root))
}

/// What a producer kernel's marks declare, and what stops it short.
///
/// # Invariant
/// The claim and the marks are registry reads and the refusal is the registry's own: the role
/// this reports is missing because the emitter never lowered the expression its mark rides,
/// which is graph-fusion.md §3's open measurement and not a fact about this file.
fn report_marks() {
    let (module, root) = match compile(producer_program()) {
        Ok(run) => run,
        Err(messages) => {
            println!("the producer was refused: {}", messages.join("; "));
            return;
        }
    };
    let Some(AnyNodeId::Dynamic(node)) = Some(AnyNodeId::Dynamic(root)) else {
        return;
    };
    let Ok(id) = kernel_id(&module, node) else {
        println!("the producer's root is not a kernel");
        return;
    };
    let claim = claim_of(id);
    let Some(claim) = claim else {
        println!("the producer claims no interface");
        return;
    };
    println!(
        "the producer claims {} with roles {:?}",
        claim.interface, claim.roles
    );
    let Ok(fragment) = fragment_of(id) else {
        println!("the producer's fragment is not registered");
        return;
    };
    for mark in &fragment.marks {
        println!(
            "  published: ~ {:?} on value {:?}",
            mark.role().unwrap_or("<unnamed>"),
            mark.value
        );
    }
    for role in &claim.roles {
        if let Err(refusal) = marked_value(&fragment, &claim.interface, role) {
            println!("  {} — {WHY_THE_IDENTITY_IS_MISSING}", refusal.message());
        }
    }
}

/// Why a `compute.write`'s own seed is not lowered.
///
/// # Invariant
/// The `@loop` nest's carried state is the entering call's arguments, which the conversion
/// reads as the values the block receives; the expression a caller hands in is therefore never
/// walked, and a mark on it is never published. See graph-fusion.md §3's open measurement.
const WHY_THE_IDENTITY_IS_MISSING: &str = "the identity's expression is the `@loop` nest's entering \
     state, which the conversion reads as the value the block receives rather than lowering, so \
     the mark on it never reaches the fragment";

/// A producer kernel with both roles marked, compiled so its marks can be read.
fn producer_program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct<.d (compute.Buf _)>
Out = struct<.r (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (6, 0.0 ~ "identity")))
}
compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu"
"#
}

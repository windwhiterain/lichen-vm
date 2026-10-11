//! The fusion chain, run numerically: the emitted rolling update against the two-pass form.
//!
//! # Invariant
//! Both columns are built from one element function and one pair of declared reductions, and
//! every number is [`lichen_fusion::evaluate`] over an expression the engine or this file
//! built — never a second implementation of the algorithm. The two-pass column applies the
//! element function at the producer's **final** value, which is the form that stores the
//! intermediate the fusion removes; the fused column is the emitted rolling update. Agreement
//! between them is the claim, and a refusal prints as the refusal it is.
//! Run with `cargo run -p lichen-language --example fusion_numbers`.

use lichen_fusion::{
    BinaryOp, Bindings, Expr, FusedReduction, Random, Reduction, Variable, derive, evaluate,
    max_relative_disagreement, substitute_all,
};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, Module, NodeId};

/// The seeds and lengths every case is run over, so no result is one point.
const SEEDS: [u64; 3] = [1, 2, 3];
const LENGTHS: [usize; 4] = [1, 4, 9, 64];

/// The agreement the two forms are held to.
const TOLERANCE: f64 = 1e-12;

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

    println!("\n--- the two forms over the same data");
    println!(
        "{:>5} {:>6}  {:>14} {:>14} {:>10}",
        "seed", "length", "two pass", "fused", "gap"
    );
    let mut worst: f64 = 0.0;
    for seed in SEEDS {
        for length in LENGTHS {
            let data = pseudo_random(seed, length);
            let two_pass = two_pass(&fused, &element, &data);
            let fused_value = rolling(&fused, &data);
            let gap = (two_pass - fused_value).abs() / two_pass.abs().max(1.0);
            worst = worst.max(gap);
            println!("{seed:>5} {length:>6}  {two_pass:>14.6} {fused_value:>14.6} {gap:>10.3e}");
        }
    }
    println!(
        "\nworst gap over {} cases: {worst:.3e}, against {TOLERANCE:.0e}",
        SEEDS.len() * LENGTHS.len()
    );
    if worst > TOLERANCE {
        println!("the two forms disagree above the tolerance");
    }

    println!("\n--- and the reduction the producer's own marks declare");
    report_marks();
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
/// Every value is rounded through `f32`, the class a kernel computes in, so the two forms are
/// compared on data both of them can hold exactly.
fn pseudo_random(seed: u64, length: usize) -> Vec<f64> {
    let mut random = Random::from_seed(seed);
    (0..length)
        .map(|_| f64::from(random.next_bounded(-4.0, 8.0) as f32))
        .collect()
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

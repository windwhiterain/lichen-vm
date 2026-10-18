//! What a reduction's own marks publish and bridge, for a literal count and a runtime one.
//!
//! # Invariant
//! Every printed fact is a registry read: the claim and the marks the compile stored, the
//! expression the marked value bridges to, and the reduction those two projections declare.
//! See docs/notes/graph-fusion.md §3.
//! Run with `cargo run -p lichen-language --example reduction_algebra`.

use lichen_compute::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_compute::language::{LangProgram, new_store};
use lichen_fusion::{Expr, normalize};
use lichen_fusion_bridge::{ReadRoles, projection};
use lichen_kernel_ir::{KernelClaim, KernelFragment, ValueId};
use lichen_lowlevel::{Module, NodeId};

/// The role a reduction's associative operator is declared under.
const COMBINE: &str = "combine";

/// The role a reduction's unit is declared under.
const IDENTITY: &str = "identity";

fn main() {
    for (name, source) in cases() {
        println!("\n================ {name}");
        match compile(&source) {
            Ok((module, root)) => report(&module, root),
            Err(messages) => {
                println!("  the program was refused:");
                for message in messages {
                    println!("    {message}");
                }
            }
        }
    }
}

/// The five programs: one algebraic intent, over two classes, at two extents.
///
/// # Invariant
/// A literal case and its runtime case differ in the entering count alone, so a difference in
/// what publishes is the count's and nothing else's. The fifth case changes the literal seed,
/// so a published constant's origin is the seed or is not.
fn cases() -> [(&'static str, String); 5] {
    [
        (
            "a literal-count fold over Float",
            source("Float", "6", "0.0"),
        ),
        (
            "a runtime-length fold over Float",
            source("Float", "k.n", "0.0"),
        ),
        ("a literal-count fold over Int", source("Int", "6", "0")),
        ("a runtime-length fold over Int", source("Int", "k.n", "0")),
        (
            "a literal-count fold over Float, seeded 3.5",
            source("Float", "6", "3.5"),
        ),
    ]
}

/// One reduction kernel over `class`, folding `bound` elements from `seed`.
fn source(class: &str, bound: &str, seed: &str) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.b (compute.Buf {class})}}
Out = struct{{.z (compute.Buf {class})}}
Par = compute.P (compute.KT {class})(.I In, .O Out)
reduction = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Int)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold ({bound}, {seed} ~ "identity")))
}}
compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "cpu" Out
"#
    )
}

/// The claim, the published marks, each role's projection, and the reduction they declare.
fn report(module: &Module<LangProgram>, root: NodeId) {
    let Ok(id) = kernel_id(module, root) else {
        println!("  the root value is not a kernel");
        return;
    };
    let Some(claim) = claim_of(id) else {
        println!("  the kernel claims no interface");
        return;
    };
    let Ok(fragment) = fragment_of(id) else {
        println!("  the kernel's fragment is not registered");
        return;
    };
    println!("  claim: {} with roles {:?}", claim.interface, claim.roles);
    println!("  published marks:");
    for mark in &fragment.marks {
        println!(
            "    ~ {:?} on {:?}",
            mark.text().unwrap_or("<a claim payload>"),
            mark.value
        );
    }
    println!("  projections:");
    let combine = bridged(&fragment, &claim, COMBINE);
    let identity = bridged(&fragment, &claim, IDENTITY);
    println!("  the reduction the marks declare:");
    let operator = match &combine {
        Err(reason) => {
            println!("    combine: {reason}");
            None
        }
        Ok(Expr::Binary(operator, _, _)) => Some(*operator),
        Ok(other) => {
            println!("    combine: `{other}` is not one operator");
            None
        }
    };
    let unit = match &identity {
        Err(reason) => {
            println!("    identity: {reason}");
            None
        }
        Ok(expr) => match normalize(expr).and_then(|canonical| canonical.as_constant()) {
            Some(value) => Some(value),
            None => {
                println!("    identity: `{expr}` is not a literal");
                None
            }
        },
    };
    match (operator, unit) {
        (Some(combine), Some(identity)) => println!(
            "    Reduction {{ combine: {}, identity: {identity} }} — derivable",
            combine.name()
        ),
        (None, None) => println!("    not derivable: neither half"),
        (None, Some(identity)) => {
            println!("    not derivable: combine missing, identity {identity}")
        }
        (Some(combine), None) => println!(
            "    not derivable: combine `{}`, identity missing",
            combine.name()
        ),
    }
}

/// One role's marked subexpression, bridged, printed as it bridges and canonicalized.
fn bridged(fragment: &KernelFragment, claim: &KernelClaim, role: &str) -> Result<Expr, String> {
    let marked: ValueId = marked_value(fragment, &claim.interface, role)
        .map_err(|refusal| format!("not marked — {}", refusal.message()))?;
    match projection(fragment, marked, ReadRoles::DATUM_ONLY) {
        Err(refusal) => {
            println!(
                "    {role}: value {marked:?} does not bridge — {}",
                refusal.message()
            );
            Err("the projection did not bridge".to_string())
        }
        Ok(expression) => {
            println!("    {role}: value {marked:?} bridges to `{expression}`");
            if let Some(definition) = fragment.body.values.get(marked.0 as usize) {
                println!("      instruction: {definition:?}");
            }
            match normalize(&expression) {
                Some(canonical) => println!("      canonical `{canonical}`"),
                None => println!("      canonical: none within the normalizer's budget"),
            }
            Ok(expression)
        }
    }
}

/// Compile and run the source, keeping the module and its root value.
fn compile(source: &str) -> Result<(Module<LangProgram>, NodeId), Vec<String>> {
    let mut store = new_store(None).expect("the embedded compute package registers");
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

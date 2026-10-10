//! The interface a lichen kernel declares, read back host-side and bridged.
//! Run with `cargo run -p lichen-language --example reduction_interface`.
//!
//! # Invariant
//! Every step of the chain is printed with its outcome, and a step the language or the
//! engine refuses is printed as the refusal that named it: this example measures where the
//! chain stops today (docs/notes/graph-fusion.md §3).

use lichen_fusion::{BinaryOp, Expr, Reduction};
use lichen_fusion_bridge::{ReadRoles, projection};
use lichen_kernel_ir::KernelFragment;
use lichen_language::fusion_claim::{Claim, claim_fragment, marked_value, read_claim};
use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
use lichen_lowlevel::Module;

/// The input position of the consumer whose read is the producer's value.
const PRODUCER_INPUT: usize = 1;

/// One kernel as a host reads it: the claim it carries, and the fragment its marks sit in.
struct Kernel {
    claim: Claim,
    fragment: Result<KernelFragment, String>,
}

fn main() {
    let producer = kernel(producer_program(), "producer");
    let consumer = kernel(consumer_program(), "consumer");
    let element = kernel(element_program(), "element");
    let (producer, consumer, element) = (producer, consumer, element);

    println!("\n--- projections");
    let combine = role(&producer, "combine", ReadRoles::DATUM_ONLY);
    let identity = role(&producer, "identity", ReadRoles::DATUM_ONLY);
    let consumer_combine = role(&consumer, "combine", ReadRoles::consumer(PRODUCER_INPUT));
    let consumer_identity = role(&consumer, "identity", ReadRoles::consumer(PRODUCER_INPUT));
    let element_function = role(&element, "element", ReadRoles::parameters(0, 1));

    println!("\n--- the interfaces the claims name");
    for (what, kernel) in [
        ("producer", &producer),
        ("consumer", &consumer),
        ("element", &element),
    ] {
        println!(
            "{what}: {} with roles {:?}",
            kernel.claim.interface, kernel.claim.roles
        );
    }

    println!("\n--- the reduction the producer's marks declare");
    match reduction(&combine, &identity) {
        Ok(reduction) => println!(
            "combine {} identity {}",
            reduction.combine.name(),
            reduction.identity
        ),
        Err(reason) => println!("not declared: {reason}"),
    }

    println!("\n--- what the chain reaches today");
    let reached = [
        combine.is_ok(),
        identity.is_ok(),
        consumer_combine.is_ok(),
        consumer_identity.is_ok(),
        element_function.is_ok(),
    ]
    .into_iter()
    .filter(|reached| *reached)
    .count();
    println!("{reached} of 5 projections bridge; {} claim(s) read", 3);
    assert_eq!(
        (
            producer.claim.interface.as_str(),
            consumer.claim.interface.as_str()
        ),
        ("Reduction", "Reduction"),
        "both kernels claim the interface they implement"
    );
}

/// Compile one of the three programs and read its kernel's claim and fragment.
fn kernel(source: &str, what: &str) -> Kernel {
    let (mut module, root) = match compile(source) {
        Ok(run) => run,
        Err(messages) => {
            println!("\n{what}: the program was refused");
            for message in messages {
                println!("  {message}");
            }
            return Kernel {
                claim: Claim {
                    interface: "unread".to_string(),
                    roles: Vec::new(),
                },
                fragment: Err("the program did not compile".to_string()),
            };
        }
    };
    let claim = match read_claim(&module, root) {
        Ok(claim) => claim,
        Err(refusal) => {
            println!("\n{what}: {}", refusal.message());
            return Kernel {
                claim: Claim {
                    interface: "unread".to_string(),
                    roles: Vec::new(),
                },
                fragment: Err(refusal.message()),
            };
        }
    };
    println!(
        "\n{what}: claims {} with roles {:?}",
        claim.interface, claim.roles
    );
    let fragment = claim_fragment(&mut module, root).map_err(|refusal| refusal.message());
    match &fragment {
        Ok(fragment) => {
            if fragment.marks.is_empty() {
                println!("  the fragment published no marks");
            }
            for mark in &fragment.marks {
                println!("  mark on {:?}: {}", mark.value, mark.role().unwrap_or("?"));
            }
        }
        Err(reason) => println!("  no fragment: {reason}"),
    }
    Kernel { claim, fragment }
}

/// One role's marked subexpression, bridged.
fn role(kernel: &Kernel, role: &str, reads: ReadRoles) -> Result<Expr, String> {
    let fragment = kernel.fragment.as_ref().map_err(|reason| reason.clone())?;
    let marked = marked_value(fragment, &kernel.claim.interface, role)
        .map_err(|refusal| refusal.message())?;
    let expr = projection(fragment, marked, reads).map_err(|refusal| refusal.message())?;
    println!("{role} = {expr}");
    Ok(expr)
}

/// The reduction two bridged projections declare.
fn reduction(
    combine: &Result<Expr, String>,
    identity: &Result<Expr, String>,
) -> Result<Reduction, String> {
    let combine = combine.as_ref().map_err(|reason| reason.clone())?;
    let identity = identity.as_ref().map_err(|reason| reason.clone())?;
    let Some(operator) = combine_operator(combine) else {
        return Err(format!("the combine is `{combine}`, not one operator"));
    };
    let Some(identity) = identity.as_constant() else {
        return Err(format!("the identity is `{identity}`, not a literal"));
    };
    Ok(Reduction {
        combine: operator,
        identity,
    })
}

/// The operator a combine projection denotes, when it is exactly one two-operand operator.
fn combine_operator(expr: &Expr) -> Option<BinaryOp> {
    match expr {
        Expr::Binary(operator, _, _) => Some(*operator),
        _ => None,
    }
}

/// The producer program: one kernel body, its claim, and the two marks.
///
/// # Invariant
/// The claim rides the body's value and the projections ride the body's expressions:
/// `? "combine"` marks the reduction's step and `? "identity"` the value it starts from, so
/// nothing the claim says is restated anywhere.
fn producer_program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
Claim = struct<.name string, .roles _>
In = struct<.d (compute.Buf _)>
Out = struct<.r (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) ? "combine")
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (6, 0.0 ? "identity")))
}
producer_raw ? Claim(.name "Reduction", .roles ("combine", "identity"))
"#
}

/// The consumer program; the element function is its own kernel, see [`element_program`].
fn consumer_program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
Claim = struct<.name string, .roles _>
In2 = struct<.d (compute.Buf _), .p (compute.Buf _)>
Out2 = struct<.z (compute.Buf _)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.p, .at 0))) ? "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (6, 0.0 ? "identity")))
}
consumer_raw ? Claim(.name "Reduction", .roles ("combine", "identity"))
"#
}

/// The element kernel: the per-element function of the datum and the producer's value.
///
/// # Invariant
/// Its two parameter leaves are the slots a projection is written over — leaf 0 the datum,
/// leaf 1 the producer's value — so the element mark names one function of both, and a body
/// carrying three marks is avoided because it does not parse today.
fn element_program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
Claim = struct<.name string, .roles _>
element_raw = (p : <Float, Float>) => (p(0) * p(1)) ? "element"
element_raw ? Claim(.name "Reduction", .roles ("element"))
"#
}

/// Compile and run the source, keeping the module and its root value.
fn compile(source: &str) -> Result<(Module<LangProgram>, lichen_lowlevel::NodeId), Vec<String>> {
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

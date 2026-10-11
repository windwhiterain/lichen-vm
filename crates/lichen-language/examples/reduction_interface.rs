//! A kernel's interface claim, read from the registry and bridged.
//!
//! # Invariant
//! Every step is printed with its outcome, and a refused step as its refusal.
//! Run with `cargo run -p lichen-language --example reduction_interface`.

use lichen_fusion::{BinaryOp, Expr, Reduction};
use lichen_fusion_bridge::{ReadRoles, projection};
use lichen_kernel_ir::{KernelClaim, KernelFragment, ValueId};
use lichen_language::fusion_claim::{claim_of, fragment_of, kernel_id, marked_value};
use lichen_language::package::PackageStore;
use lichen_language::program::LangProgram;
use lichen_lowlevel::{AnyNodeId, Module, NodeId};

/// The input position of the consumer whose read is the producer's value.
const PRODUCER_INPUT: usize = 1;

/// One kernel as a host reads it: its claim and its compiled fragment.
struct Kernel {
    name: &'static str,
    claim: Option<KernelClaim>,
    fragment: KernelFragment,
}

/// One role's bridged projection, or the refusal that named it.
type Projection = Result<Expr, String>;

fn main() {
    let (module, root) = match compile(program()) {
        Ok(run) => run,
        Err(messages) => {
            println!("the program was refused:");
            for message in messages {
                println!("  {message}");
            }
            return;
        }
    };

    let kernels = match kernels(&module, root) {
        Ok(kernels) => kernels,
        Err(message) => {
            println!("the program's root is not its three kernels: {message}");
            return;
        }
    };

    println!("\n--- the claims the kernel registry holds");
    for kernel in &kernels {
        match &kernel.claim {
            Some(claim) => {
                println!(
                    "{}: {} with roles {:?}",
                    kernel.name, claim.interface, claim.roles
                )
            }
            None => println!("{}: no claim", kernel.name),
        }
    }

    println!("\n--- the marks each dispatched kernel's fragment published");
    for kernel in &kernels {
        for mark in &kernel.fragment.marks {
            println!(
                "{}: ~ {:?} on value {:?}",
                kernel.name,
                mark.role().unwrap_or("<unnamed>"),
                mark.value
            );
        }
    }

    println!("\n--- projections");
    let combine = projection_of(&kernels[0], "combine", ReadRoles::DATUM_ONLY);
    let identity = projection_of(&kernels[0], "identity", ReadRoles::DATUM_ONLY);
    let consumer_combine =
        projection_of(&kernels[1], "combine", ReadRoles::consumer(PRODUCER_INPUT));
    let consumer_identity =
        projection_of(&kernels[1], "identity", ReadRoles::consumer(PRODUCER_INPUT));
    let element = projection_of(&kernels[2], "element", ReadRoles::parameters(0, 1));

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
    let bridged = [
        &combine,
        &identity,
        &consumer_combine,
        &consumer_identity,
        &element,
    ]
    .into_iter()
    .filter(|projection| projection.is_ok())
    .count();
    let claims = kernels
        .iter()
        .filter(|kernel| kernel.claim.is_some())
        .count();
    println!("{bridged} of 5 projections bridge; {claims} of 3 claims read");

    println!("\n--- a claim its body cannot satisfy is refused while the program compiles");
    for message in compile_refused(incomplete_claim_program()) {
        println!("  {message}");
    }
}

/// The program's three kernels, each with the claim and fragment the registry holds.
///
/// # Invariant
/// Both facts are registry reads: the claim from the compile that filled the entry, the
/// marks from the fragment beside it. A kernel *value* carries neither, and the module is
/// read only for the id the value names.
fn kernels(module: &Module<LangProgram>, root: NodeId) -> Result<Vec<Kernel>, String> {
    // SAFETY: `root` is a live node of `module`; nothing here drops a block.
    let items = unsafe { module.array_items(root) }.ok_or("the root is not a tuple")?;
    let mut kernels = Vec::with_capacity(items.len());
    for (name, item) in ["producer", "consumer", "element"].iter().zip(items.iter()) {
        let AnyNodeId::Dynamic(node) = item.node else {
            return Err("a kernel read from a frozen artifact is not compiled here".to_string());
        };
        let id = kernel_id(module, node).map_err(|refusal| refusal.message())?;
        kernels.push(Kernel {
            name,
            claim: claim_of(id),
            fragment: fragment_of(id).map_err(|refusal| refusal.message())?,
        });
    }
    Ok(kernels)
}

/// One role's marked subexpression, bridged.
fn projection_of(kernel: &Kernel, role: &str, reads: ReadRoles) -> Projection {
    let interface = kernel
        .claim
        .as_ref()
        .map(|claim| claim.interface.clone())
        .unwrap_or_default();
    let marked: ValueId =
        marked_value(&kernel.fragment, &interface, role).map_err(|refusal| refusal.message())?;
    projection(&kernel.fragment, marked, reads).map_err(|refusal| refusal.message())
}

/// The reduction two bridged projections declare.
fn reduction(combine: &Projection, identity: &Projection) -> Result<Reduction, String> {
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

/// The diagnostics a program that cannot satisfy its claim reports.
fn compile_refused(source: &str) -> Vec<String> {
    match compile(source) {
        Ok(_) => vec!["the program compiled, and it should not have".to_string()],
        Err(messages) => messages,
    }
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

/// The program: a producer, a consumer and an element kernel in one file.
///
/// # Invariant
/// The claim rides the kernel-producing expression and each projection is a mark inside
/// that kernel's own body, so nothing a claim says is restated anywhere. The producer's
/// recurrence carries **three** marks on one expression — the case the attribute channel
/// could not parse.
fn program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In = struct<.d (compute.Buf Float)>
Out = struct<.r (compute.Buf Float)>
Par = compute.P (compute.KT _)(.I In, .O Out)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
In2 = struct<.d (compute.Buf Float), .p (compute.Buf Float)>
Out2 = struct<.z (compute.Buf Float)>
Par2 = compute.P (compute.KT _)(.I In2, .O Out2)
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) * compute.read ((compute.Read _)(.from k.in.p, .at 0))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
element_raw = (p : <Float, Float>) => (p(0) * p(1)) ~ "element"
(compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu",
 compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity)") "cpu",
 compute.jit (element_raw ~ "claim:Reduction(element)"))
"#
}

/// A claim whose body does not mark every role it requires.
///
/// # Invariant
/// The refusal is the assembly's coverage check, reported while the program compiles: the
/// body names `combine` and the claim asks for `combine` and `identity`, so nothing
/// downstream ever has to discover the gap.
fn incomplete_claim_program() -> &'static str {
    r#"
---
  compute = import "compute.lichen"
---
In = struct<.d (compute.Buf _)>
Out = struct<.r (compute.Buf _)>
Par = compute.P (compute.KT _)(.I In, .O Out)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1))) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (6, 0.0)))
}
compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu"
"#
}

//! The region schema a kernel chain derives, and the fusion the engine makes of it.
//!
//! # Invariant
//! See docs/notes/graph-fusion.md §4 and §6.
//! Run with `cargo run -p lichen-language --example fusion_schema`.

use lichen_compute::fusion_claim::{
    Instance, Port, PortKind, RegionSchema, SchemaRefusal, claim_of, derive_regions, fragment_of,
    kernel_id,
};
use lichen_compute::language::{LangProgram, new_store};
use lichen_compute::{ComputeValue, compute::graph};
use lichen_fusion::derive;
use lichen_graph_ir::Graph;
use lichen_kernel_ir::fragment_digest;
use lichen_lowlevel::{AnyNodeId, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The consumer's read of the per-element datum, at input position 0.
const DATUM: &str = "(compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)";

fn main() {
    let softmax = chain(
        &format!("(max ({DATUM}) s(1))"),
        "(0.0 - 1000000.0)",
        &format!("(s(1) + (exp (({DATUM}) - r) ~ \"element\"))"),
        "0.0",
    );
    let reference = chain(
        &format!("(s(1) + {DATUM})"),
        "0.0",
        &format!("(max ((({DATUM}) - r) ~ \"element\") s(1))"),
        "(0.0 - 1000000.0)",
    );
    let changed = chain(
        &format!("(s(1) + {DATUM})"),
        "0.0",
        &format!("(max ((({DATUM}) + r) ~ \"element\") s(1))"),
        "(0.0 - 1000000.0)",
    );

    let softmax = report(
        "the softmax-shaped chain: a max, then a sum of exp(d - r)",
        &softmax,
    );
    let reference = report("the reference sum/max chain", &reference);
    let respelled = report(
        "the same reference logic, re-spelled: renamed binders, the independent statements \
         swapped, the commutative operators' operands the other way round",
        RESPELT,
    );
    let changed = report(
        "the reference chain with one changed algebra: the element is d + r",
        &changed,
    );

    println!("\n================ the contract's two load-bearing properties");
    let (Some(reference), Some(respelled), Some(changed)) = (&reference, &respelled, &changed)
    else {
        println!("a chain did not derive, so the comparison has nothing to compare");
        return;
    };
    let reference_schema = schema_of(reference);
    let respelled_schema = schema_of(respelled);
    let changed_schema = schema_of(changed);
    println!("reference      {}", reference_schema.hash);
    println!("re-spelled     {}", respelled_schema.hash);
    println!("changed `g`    {}", changed_schema.hash);
    println!(
        "producer fragment digests: reference {:#018x}, re-spelled {:#018x}",
        fragment_digest(&reference.instances[0].fragment),
        fragment_digest(&respelled.instances[0].fragment),
    );
    println!(
        "consumer fragment digests: reference {:#018x}, re-spelled {:#018x}",
        fragment_digest(&reference.instances[1].fragment),
        fragment_digest(&respelled.instances[1].fragment),
    );
    println!(
        "a re-spelling that changes both fragments: {}",
        verdict(reference_schema.hash == respelled_schema.hash)
    );
    println!(
        "a changed algebra: {}",
        verdict(reference_schema.hash != changed_schema.hash)
    );

    println!("\n================ what the engine derives from a schema's own projections");
    let mut handoffs = vec![("the reference instance", reference_schema)];
    if let Some(softmax) = &softmax {
        handoffs.push(("the softmax instance", schema_of(softmax)));
    }
    for (title, schema) in handoffs {
        println!("\n--- {title}");
        let producer = schema.nodes[0].reduction();
        let consumer = schema.nodes[1].reduction();
        let Some(element) = schema
            .between
            .first()
            .map(|between| between.element.clone())
        else {
            println!("the schema has no between code to hand the engine");
            continue;
        };
        let (Ok(producer), Ok(consumer)) = (producer, consumer) else {
            println!("a declared reduction did not read out of the schema");
            continue;
        };
        println!(
            "producer: combine `{}`, identity {}",
            producer.combine.name(),
            producer.identity
        );
        println!("element:  {element}");
        println!(
            "consumer: combine `{}`, identity {}",
            consumer.combine.name(),
            consumer.identity
        );
        match derive(producer, element, consumer) {
            Ok(fused) => print!("--- derived\n{fused}"),
            Err(refusal) => println!("--- refused: {}", refusal.message()),
        }
    }
    println!(
        "the numeric obligation this hands over is `fusion_numbers`'s: the emitted rolling \
         update against the two-pass form over real data"
    );
}

/// One program as the derivation reads it: the registry instances, and the recorded graph.
struct Derived {
    instances: Vec<Instance>,
    graph: Graph,
    regions: Vec<Result<RegionSchema, SchemaRefusal>>,
}

/// Compile, record and derive one chain, printing every step.
fn report(title: &str, source: &str) -> Option<Derived> {
    println!("\n================ {title}");
    match compile(source) {
        Ok(derived) => {
            for instance in &derived.instances {
                println!(
                    "kernel {} claims {} with roles {:?}, fragment digest {:#018x}",
                    instance.kernel,
                    instance.claim.interface,
                    instance.claim.roles,
                    fragment_digest(&instance.fragment),
                );
            }
            println!(
                "the recorded graph holds {} node(s), {} input(s), {} value(s)",
                derived.graph.len(),
                derived.graph.input_count(),
                derived.graph.value_count(),
            );
            for (index, region) in derived.regions.iter().enumerate() {
                match region {
                    Ok(schema) => print_schema(index, schema),
                    Err(refusal) => println!("region {index}: refused — {}", refusal.message()),
                }
            }
            Some(derived)
        }
        Err(messages) => {
            println!("the program was refused:");
            for message in messages {
                println!("  {message}");
            }
            None
        }
    }
}

/// The one schema a program derived, for the sections that compare two of them.
fn schema_of(derived: &Derived) -> &RegionSchema {
    let schema = derived
        .regions
        .iter()
        .find_map(|region| region.as_ref().ok())
        .expect("a report that reached this point derived a region");
    schema
}

/// One region: its instances and their projections, the code between them, and its ports.
fn print_schema(index: usize, schema: &RegionSchema) {
    println!("region {index}: {} instance(s)", schema.nodes.len());
    for node in &schema.nodes {
        println!(
            "  instance {}: kernel {}, interface {}",
            node.position, node.kernel, node.interface
        );
        for projection in &node.projections {
            println!("    {:>8} = {}", projection.role, projection.expression);
        }
    }
    for between in &schema.between {
        println!(
            "  between {} -> {} at input {}: {}",
            between.producer, between.consumer, between.input, between.element
        );
    }
    for port in &schema.ports {
        println!("  port {}", describe(port));
    }
    println!("  hash {}", schema.hash);
    println!("  canonical form:");
    for line in schema.canonical_form().lines() {
        println!("    {line}");
    }
}

/// One boundary port, as a report reads it.
fn describe(port: &Port) -> String {
    let class = port
        .class
        .map_or(String::new(), |class| format!(" class {class:?}"));
    match port.kind {
        PortKind::Extent => match (port.count, port.value) {
            (Some(count), _) => format!("extent of instance {}, constant {count}", port.node),
            (None, Some(value)) => format!("extent of instance {}, graph value {value}", port.node),
            (None, None) => format!("extent of instance {}", port.node),
        },
        PortKind::Input => format!(
            "input of instance {} position {}{class}",
            port.node, port.position
        ),
        PortKind::Output => format!(
            "output of instance {} position {}{class}",
            port.node, port.position
        ),
    }
}

/// What a property's comparison came out as.
fn verdict(holds: bool) -> &'static str {
    if holds { "holds" } else { "DOES NOT HOLD" }
}

/// The chain's source: one producer reduction, one consumer reduction, and the recording.
///
/// # Invariant
/// The consumer reads the producer's output buffer at input position 1, so the recorded edge
/// and the `element` projection's `Variable::PRODUCER` read name the same input.
fn chain(
    producer_combine: &str,
    producer_identity: &str,
    consumer_combine: &str,
    consumer_identity: &str,
) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   = struct{{.d (compute.Buf Float)}}
OutR = struct{{.r (compute.Buf Float)}}
Par  = compute.P (compute.KT _)(.I In, .O OutR)
producer_raw = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ({producer_combine}) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, {producer_identity} ~ "identity")))
}}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu" OutR
In2  = struct{{.d (compute.Buf Float), .p (compute.Buf Float)}}
OutZ = struct{{.z (compute.Buf Float)}}
Par2 = compute.P (compute.KT _)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {{
  i = compute.range k.n
  r = (compute.read ((compute.Read _)(.from k.in.p, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ({consumer_combine}) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, {consumer_identity} ~ "identity")))
}}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutZ
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r)))
}}
built = compute.graph step
(producer, consumer, built)
"#
    )
}

/// The reference chain re-spelled: the same algebra under different names and orders.
const RESPELT: &str = r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutR = struct{.r (compute.Buf Float)}
Par  = compute.P (compute.KT _)(.I In, .O OutR)
producer_raw = (lane : Par) => {
  at = compute.range lane.n
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, ((compute.read ((compute.Read _)(.from lane.in.d, .at s(0) - 1)) : Float) + s(1)) ~ "combine")
  compute.write ((compute.Write _)(.to lane.out.r, .at at, .value acc (lane.n, 0.0 ~ "identity")))
}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu" OutR
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT _)(.I In2, .O OutZ)
consumer_raw = (lane : Par2) => {
  running = (compute.read ((compute.Read _)(.from lane.in.p, .at 0)) : Float)
  at = compute.range lane.n
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, (max s(1) (((compute.read ((compute.Read _)(.from lane.in.d, .at s(0) - 1)) : Float) - running) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to lane.out.z, .at at, .value acc (lane.n, (0.0 - 1000000.0) ~ "identity")))
}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "cpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r)))
}
built = compute.graph step
(producer, consumer, built)
"#;

/// Compile and record a chain, keeping the registry instances and the graph it recorded.
fn compile(source: &str) -> Result<Derived, Vec<String>> {
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
    let instances = instances(&module, root)?;
    let graph = recorded(&module, root)?;
    let regions = derive_regions(&graph, &instances);
    Ok(Derived {
        instances,
        graph,
        regions,
    })
}

/// The two claimed kernels the program's root tuple holds.
fn instances(module: &Module<LangProgram>, root: NodeId) -> Result<Vec<Instance>, Vec<String>> {
    // SAFETY: `root` is a live node of `module`, and nothing here drops a block.
    let items = unsafe { module.array_items(root) }
        .ok_or_else(|| vec!["the program's root is not a tuple".to_string()])?;
    let mut instances = Vec::with_capacity(2);
    for (name, item) in ["producer", "consumer"].iter().zip(items.iter()) {
        let AnyNodeId::Dynamic(node) = item.node else {
            return Err(vec![format!("{name} comes from a frozen artifact")]);
        };
        let id = kernel_id(module, node).map_err(|refusal| vec![refusal.message()])?;
        let claim = claim_of(id).ok_or_else(|| vec![format!("kernel {id} claims no interface")])?;
        let fragment = fragment_of(id).map_err(|refusal| vec![refusal.message()])?;
        instances.push(Instance {
            kernel: id,
            claim,
            fragment,
        });
    }
    Ok(instances)
}

/// The graph the program's third root element recorded.
fn recorded(module: &Module<LangProgram>, root: NodeId) -> Result<Graph, Vec<String>> {
    // SAFETY: `root` is a live node of `module`, and nothing here drops a block.
    let items = unsafe { module.array_items(root) }
        .ok_or_else(|| vec!["the program's root is not a tuple".to_string()])?;
    let Some(item) = items.get(2) else {
        return Err(vec!["the program's root holds no graph".to_string()]);
    };
    let AnyNodeId::Dynamic(node) = item.node else {
        return Err(vec!["the graph comes from a frozen artifact".to_string()]);
    };
    let value = module
        .node_value(AnyNodeId::Dynamic(node))
        .or_else(|| module.node_value(AnyNodeId::Dynamic(node)))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        return Err(vec![
            "the program's third root element is not a graph".to_string(),
        ]);
    };
    graph::lookup(id).map_err(|reason| vec![reason])
}

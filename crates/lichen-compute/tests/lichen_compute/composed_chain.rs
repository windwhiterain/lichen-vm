//! The example's two recorded fragments, composed and run on a device.
//! See `docs/notes/graph-fusion.md` §4.

use lichen_compute::compute::{FusedKernel, read_fused_kernel, write_fused_kernel};
use lichen_compute::fusion_claim::{Instance, claim_of, fragment_of, kernel_id};
use lichen_compute::language::LangProgram;
use lichen_compute::language::new_store;
use lichen_compute::{ComputeValue, compute::graph};
use lichen_graph_ir::{Count, Graph, KernelNode, Node, Policy, Runner, Value};
use lichen_kernel_ir::{
    BufferSlot, KernelBody, KernelFragment, KernelInstr, Plan, ScalarClass, ScalarData, ValueDef,
    compose,
};
use lichen_lowlevel::{AnyNodeId, Module, NodeId};
use lichen_utils::extend::AsEnum;

/// The lengths the differential runs at: a small one, one over a whole workgroup, and one longer.
const LENGTHS: [usize; 3] = [8, 100, 1024];

/// The consumer's input position the producer's output feeds.
const ABSORBED: u32 = 1;

#[test]
fn the_composed_chain_records_the_merged_body() {
    let (instances, recorded) = compile(SOURCE).expect("the chain compiles");
    let producer = instances[0].fragment.clone();
    let consumer = instances[1].fragment.clone();

    // The recorded pair, untouched: the producer's marks go by contract, not by caller surgery.
    let merged = compose(&producer, &consumer, ABSORBED).expect("the recorded chain composes");
    merged
        .body
        .validate()
        .expect("the merged body is well formed");
    merged
        .validate()
        .expect("the merged fragment is well formed");
    assert_eq!(
        (write_count(&merged.body), write_count(&consumer.body)),
        (1, 1),
        "the merged body holds exactly the consumer's writes, not the producer's store"
    );
    assert_eq!(
        merged
            .marks
            .iter()
            .map(|mark| mark.text().unwrap_or("<named>"))
            .collect::<Vec<_>>(),
        consumer
            .marks
            .iter()
            .map(|mark| mark.text().unwrap_or("<named>"))
            .collect::<Vec<_>>(),
        "the surviving marks are the consumer's"
    );
    println!(
        "merged: {} value(s), {} block(s), {} write(s), {} input(s) {:?}, {} output(s) {:?}, {} \
         mark(s) {:?}",
        merged.body.values.len(),
        merged.body.blocks.len(),
        write_count(&merged.body),
        merged.inputs,
        merged.input_classes,
        merged.outputs,
        merged.output_classes,
        merged.marks.len(),
        merged
            .marks
            .iter()
            .map(|mark| mark.text().unwrap_or("<named>"))
            .collect::<Vec<_>>(),
    );

    // The artifact reader re-derives a block parameter's index: an orphan would read back refused.
    let artifact = write_fused_kernel(&FusedKernel {
        fragment: merged.clone(),
        claim: None,
        plan: Plan::default(),
    });
    let read = read_fused_kernel(&artifact).expect("the merged fragment round-trips");
    assert_eq!(
        read.fragment.body, merged.body,
        "the artifact holds the merged body element for element"
    );
}

/// How many instructions of `body` are buffer writes.
fn write_count(body: &KernelBody) -> usize {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter())
        .filter(|&&value| {
            matches!(
                body.values[value.0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::BufferWriteCall(_),
                    ..
                }
            )
        })
        .count()
}

/// The answer `d[i] = i + 1` folds to.
///
/// # Invariant
/// The fold covers the whole extent in every lane, so every position is `n` less the sum
/// `1 + … + n`, and both are exactly representable `f32` integers.
fn closed_form(extent: usize) -> f32 {
    let sum = (extent * (extent + 1) / 2) as f32;
    extent as f32 - sum
}

/// The two recorded fragments as two nodes: the reference the merge is differenced against.
fn two_pass_graph(producer: &KernelFragment, consumer: &KernelFragment, extent: usize) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let first = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: producer.clone(),
                inputs: vec![0],
                count: Count::Constant(extent),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    let second = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: consumer.clone(),
                inputs: vec![0, first],
                count: Count::Constant(extent),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![second])
        .expect("a value the graph defines");
    graph
}

/// The merged fragment as one node over the graph's own input.
///
/// # Invariant
/// The merged fragment's inputs are the union — the producer's own, then the consumer's
/// surviving one — and both name the graph's single buffer here, so the node lists that one
/// value twice.
fn merged_graph(fragment: &KernelFragment, extent: usize) -> Graph {
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: fragment.clone(),
                inputs: vec![0, 0],
                count: Count::Constant(extent),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    graph
}

/// Element-for-element equality, naming the position the two first disagreed at.
fn compare(what: &str, found: &[f32], against: &str, expected: &[f32]) {
    assert_eq!(
        found.len(),
        expected.len(),
        "{what} and {against} answer a different number of elements"
    );
    for (position, (found, expected)) in found.iter().zip(expected).enumerate() {
        assert_eq!(
            found, expected,
            "{what} answers {found} at position {position} and {against} answers {expected}"
        );
    }
}

/// The packed bytes of `elements`, one `f32` each.
fn pack(elements: &[f32]) -> Vec<u8> {
    elements
        .iter()
        .flat_map(|element| element.to_le_bytes())
        .collect()
}

/// The chain the example's `RESPELT` constant spells: a sum, then a max.
/// See `examples/fusion_schema.rs`.
const SOURCE: &str = r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutR = struct{.r (compute.Buf Float)}
Par  = struct{.n Int, .in In}
producer_raw = (lane : Par) => {
  at = compute.range lane.n
  r = compute.slice ((compute.Slice _)(.count lane.n, .element Float))
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, ((compute.read ((compute.Read _)(.from lane.in.d, .at s(0) - 1)) : Float) + s(1)) ~ "combine")
  ((OutR)(.r (compute.write ((compute.Write _)(.to r, .at at, .value acc (lane.n, 0.0 ~ "identity"))))))
}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "cpu" OutR
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = struct{.n Int, .in In2}
consumer_raw = (lane : Par2) => {
  running = (compute.read ((compute.Read _)(.from lane.in.p, .at 0)) : Float)
  at = compute.range lane.n
  z = compute.slice ((compute.Slice _)(.count lane.n, .element Float))
  @loop acc = s => if s(0) == 0 then s(1) else acc (s(0) - 1, (max s(1) (((compute.read ((compute.Read _)(.from lane.in.d, .at s(0) - 1)) : Float) - running) ~ "element")) ~ "combine")
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at at, .value acc (lane.n, (0.0 - 1000000.0) ~ "identity"))))))
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

/// Compile and record the chain, keeping the registry instances and the graph it recorded.
fn compile(source: &str) -> Result<(Vec<Instance>, Graph), Vec<String>> {
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
    let mut instances = Vec::new();
    for (name, item) in ["producer", "consumer"]
        .iter()
        .zip(items(&module, root)?.iter())
    {
        let AnyNodeId::Dynamic(node) = item.node else {
            return Err(vec![format!("{name} comes from a frozen artifact")]);
        };
        let id = kernel_id(&module, node).map_err(|refusal| vec![refusal.message()])?;
        let claim = claim_of(id).ok_or_else(|| vec![format!("kernel {id} claims no interface")])?;
        let fragment = fragment_of(id).map_err(|refusal| vec![refusal.message()])?;
        instances.push(Instance {
            kernel: id,
            claim,
            fragment,
        });
    }
    let graph = recorded(&module, root)?;
    Ok((instances, graph))
}

/// The root tuple's items, borrowed from a live module.
fn items(
    module: &Module<LangProgram>,
    root: NodeId,
) -> Result<Vec<lichen_lowlevel::ArrayItem>, Vec<String>> {
    // SAFETY: `root` is a live node of `module`, and nothing here drops a block.
    let items = unsafe { module.array_items(root) }
        .ok_or_else(|| vec!["the program's root is not a tuple".to_string()])?;
    Ok(items.to_vec())
}

/// The graph the program's third root element recorded.
fn recorded(module: &Module<LangProgram>, root: NodeId) -> Result<Graph, Vec<String>> {
    let items = items(module, root)?;
    let Some(item) = items.get(2) else {
        return Err(vec!["the program's root holds no graph".to_string()]);
    };
    let AnyNodeId::Dynamic(node) = item.node else {
        return Err(vec!["the graph comes from a frozen artifact".to_string()]);
    };
    // A compound's element is a slot, so the value is the one its slot carries
    // (`docs/notes/compound-element-slots.md`).
    let value = module
        .element_value_node(AnyNodeId::Dynamic(node))
        .or(Some(AnyNodeId::Dynamic(node)))
        .and_then(|node| module.node_value(node))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        return Err(vec![
            "the program's third root element is not a graph".to_string(),
        ]);
    };
    graph::lookup(id).map_err(|reason| vec![reason])
}

//! Two reads of one producer's output in one body: one cone copy per read.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, graph, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, claim_of, derive_regions, fragment_of, kernel_id,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::placement_pack::{PlacementPack, read_placement_pack};
use lichen_compute::region_merge::{install_region_merge_with_packs, region_merge_pass_with_packs};
use lichen_graph_ir::{Graph, KernelNode, Node};
use lichen_kernel_ir::{KernelBody, KernelInstr, ScalarClass, ValueDef, ValueId, compose};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The lanes the chain runs over.
const COUNT: usize = 8;

/// The consumer's input position the producer's output feeds.
const ABSORBED: u32 = 1;

/// The pack a runner installs for this region.
const PACK: &str = "two-reads";

/// The chain `e[i] = d[i] · 2` feeding `z[i] = e[i] + e[i + 1]`, both claimed elementwise.
///
/// # Invariant
/// The consumer's body holds **two** reads of the absorbed buffer, at `i` and at `i + 1`: the only
/// shape in which a single copy reused for both reads would answer `2 · e[i]`. `In2` declares
/// `k.in.d` beside the buffer it absorbs because the merged node's one surviving input is the
/// producer's, and the port check reads the two lists position for position.
fn program() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {ELEMENTS})))}}
OutE = struct{{.e (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
ParE = struct{{.n Int, .in In}}
scale_raw = (k : ParE) => {{
  i = compute.range k.n
  e = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutE)(.e (compute.write ((compute.Write _)(.to e, .at i, .value (((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0) ~ "element"))))))
}}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" OutE
In2  = struct{{.d (compute.Buf ((compute.BufParam _)(.type Float, .length {ELEMENTS}))), .p (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
OutZ = struct{{.z (compute.Buf ((compute.BufParam _)(.type Float, .length {COUNT})))}}
Par2 = struct{{.n Int, .in In2}}
pair_raw = (k : Par2) => {{
  i = compute.range k.n
  left = (compute.read ((compute.Read _)(.from k.in.p, .at i)) : Float)
  right = (compute.read ((compute.Read _)(.from k.in.p, .at i + 1)) : Float)
  z = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at i, .value ((left + right) ~ "element"))))))
}}
pair = compute.parallel (pair_raw ~ "claim:Elementwise(element)") "gpu" OutZ
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun pair ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}}
built = compute.graph step
"#
    )
}

/// The chain as a recording: the two kernels and the graph they recorded, nothing run.
///
/// # Invariant
/// Each body declares its buffer with `compute.buf` and returns the write as the codomain
/// field's value (`docs/notes/kernel-buffer-declarations.md`).
fn recording() -> String {
    format!("{}(scale, pair, built)\n", program())
}

/// The recording as a program that runs it and answers, over `elements` data elements.
fn running_recording(elements: usize) -> String {
    format!(
        "{recording}\ncount = ({count})\n{fill}data = (compute.plrun filler ((Fill)(.n {elements})) : In)\nout = \
         compute.collect (compute.graphrun built (count, data.d))\nout\n",
        recording = recording(),
        fill = fill(elements),
        count = COUNT,
        elements = elements,
    )
}

/// The fill kernel that builds the launch's data, and the table it writes.
///
/// # Invariant
/// One lane per element and each lane writes the element at its own index, so the buffer holds
/// the list the old `compute.buffer` call held: a host builds its data with a kernel, because
/// there is no array-to-buffer conversion any more (`docs/notes/kernel-buffer-declarations.md`).
fn fill(elements: usize) -> String {
    format!(
        r#"
Fill = struct{{.n Int}}
fill_raw = (c : Fill) => {{
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length {elements}))
  ((In)(.d (compute.write ((compute.Write _)(.to y, .at i, .value {table})))))
}}
filler = compute.parallel fill_raw "cpu" In
"#,
        table = table(elements),
    )
}

/// The data the launch reads: `1.0 … elements`, as the fill kernel's table.
///
/// # Invariant
/// One arm per element and each arm names its own lane, so the sum is the list the old array
/// literal held and not a formula of it: a nested `if` chain is refused
/// (`docs/notes/kernel-buffer-declarations.md`).
fn table(elements: usize) -> String {
    (0..elements)
        .map(|position| {
            let value = position as f32 + 1.0;
            format!("(if i == {position} then {value:.1} else 0.0)")
        })
        .collect::<Vec<String>>()
        .join(" + ")
}

/// How many leading elements of the two-read answer the program defines.
///
/// # Invariant
/// Lane `COUNT - 1` reads the absorbed buffer at `COUNT`, one past the extent the launch binds,
/// and the device answers `0.0` there whatever the data buffer's own length — so both runs hold
/// `2 d[COUNT - 1]` at that lane and neither number is the program's answer. Every lane below
/// reads two elements inside the extent, and those are the ones compared.
const DEFINED: usize = COUNT - 1;

/// The data elements: one more than the lanes, so the excluded lane is not short of data.
const ELEMENTS: usize = COUNT + 1;

/// The answer `z[i] = 2 d[i] + 2 d[i + 1]` has over `d = 1 … ELEMENTS`.
fn pair_answer() -> Vec<f32> {
    (0..COUNT)
        .map(|position| 2.0 * (position + 1) as f32 + 2.0 * (position + 2) as f32)
        .collect()
}

/// The answer `z[i] = 4 d[i]` has over `d = 1 … ELEMENTS`: the second copy at the first's address.
fn shared_answer() -> Vec<f32> {
    (0..COUNT)
        .map(|position| 4.0 * (position + 1) as f32)
        .collect()
}

/// The root tuple's items, read from the value the program answered.
///
/// # Invariant
/// The module's root node is not read back: a program whose root is a value has one node for its
/// value and its type, and `common::run` evaluates the type after the value.
fn root_items(value: &LangValue) -> &[ArrayItem] {
    let LangValue::LowValue(LowValue::Array(array)) = value else {
        panic!("the program's root is a tuple");
    };
    // SAFETY: `array` is the payload of the root value just produced, whose block is alive.
    unsafe { array.items() }
}

/// The two claimed kernels the recording's root tuple holds.
fn instances(module: &Module<LangProgram>, value: &LangValue) -> Vec<Instance> {
    root_items(value)
        .iter()
        .take(2)
        .map(|item| {
            let AnyNodeId::Dynamic(node) = item.node else {
                panic!("a kernel comes from a frozen artifact");
            };
            let kernel = kernel_id(module, node).expect("the item names a kernel");
            Instance {
                kernel,
                claim: claim_of(kernel).expect("the kernel claims an interface"),
                fragment: fragment_of(kernel).expect("the registry holds the dispatched fragment"),
            }
        })
        .collect()
}

/// The graph the recording's third root element names.
fn recorded(module: &Module<LangProgram>, value: &LangValue) -> Graph {
    let item = root_items(value)
        .get(2)
        .expect("the program's root holds a graph");
    let AnyNodeId::Dynamic(node) = item.node else {
        panic!("the graph comes from a frozen artifact");
    };
    let value = module
        .element_value_node(AnyNodeId::Dynamic(node))
        .or(Some(AnyNodeId::Dynamic(node)))
        .and_then(|node| module.node_value(node))
        .and_then(|value| AsEnum::<ComputeValue>::as_enum(&value));
    let Some(ComputeValue::Graph(id, _)) = value else {
        panic!("the program's third root element is not a graph");
    };
    graph::lookup(id).expect("the recorded graph is registered")
}

/// The one schema `recording()` derives, with its module, instances and graph.
fn derived() -> (Module<LangProgram>, Vec<Instance>, RegionSchema, Graph) {
    let (module, value, _root) = common::run(&recording());
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    let regions = derive_regions(&graph, &instances);
    assert_eq!(regions.len(), 1, "the recording is one claimed region");
    let schema = regions
        .into_iter()
        .next()
        .expect("the one region")
        .expect("an elementwise chain derives a schema");
    (module, instances, schema, graph)
}

/// The pack a human writes for `region`, as the JSON artifact's fields spell it.
///
/// # Invariant
/// The hash is the **region's**, in the field §5's sketch gives it, and the node half is absent:
/// this pack places nothing, so it names no node schema and stages nothing.
fn pack_text(region: &str) -> String {
    format!(
        r#"{{
  "schema": null,
  "name": "{PACK}",
  "requires": {{ "class": "float" }},
  "fuse": {{ "region": "{region}", "as": "one_kernel" }},
  "plan": {{ "staging": [] }}
}}"#
    )
}

/// The pack this test installs, read as one a human could have written.
fn pack(region: &str) -> PlacementPack {
    read_placement_pack(&pack_text(region))
        .unwrap_or_else(|refusal| panic!("the pack a human wrote is read: {refusal}"))
}

/// Hand the recorded graph to the pass with `packs` installed, and answer what it decided.
fn consulted(
    module: &Module<LangProgram>,
    graph: &Graph,
    packs: &[PlacementPack],
) -> (Option<Graph>, Vec<String>) {
    clear_graph_pass();
    install_region_merge_with_packs(module, packs);
    let rewritten = consult_graph_pass(graph, &|_node| Some(COUNT));
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    (rewritten, lines)
}

/// The one kernel node a graph holds.
fn only_kernel(graph: &Graph) -> &KernelNode {
    match &graph.nodes()[0] {
        Node::Kernel(node) => node,
    }
}

/// The address each read of `position` names, in table order.
///
/// # Invariant
/// A `BufferReadCall`'s operands are the constant that names the buffer and the value that names
/// the address, and this reads the second: the position alone cannot tell two reads of one buffer
/// apart, which is the whole of what the merge has to get right.
fn read_addresses_at(body: &KernelBody, position: i64) -> Vec<ValueId> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter().copied())
        .filter(|&value| {
            let ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            } = &body.values[value.0 as usize]
            else {
                return false;
            };
            matches!(
                &body.values[args[0].0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::Const(ScalarClass::Int, bits),
                    ..
                } if *bits == position
            )
        })
        .map(|value| match &body.values[value.0 as usize] {
            ValueDef::Instr { args, .. } => args[1],
            _ => unreachable!("a buffer read names its address"),
        })
        .collect()
}

/// The lines the installed pass reported for a module, through the run report's own channel.
fn pass_report(module: &Module<LangProgram>) -> Vec<String> {
    module
        .extension_diagnostics
        .iter()
        .filter(|entry| entry.category.contains("pass"))
        .map(|entry| entry.message.clone())
        .collect()
}

/// The merge whose second read is answered by the first read's address.
///
/// # Invariant
/// The mutation is inside the composed body alone, and it is the one answer a merged body made
/// from a single reused copy would already give: `2 · e[i] + 2 · e[i]` is `4 d[i]`. An answer that
/// moves when the second address is rewritten is therefore an answer the unmutated body could not
/// have come from one copy.
struct SharedAddress(Box<dyn GraphPass>);

impl GraphPass for SharedAddress {
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let rewritten = self.0.apply(graph, extent, lines)?;
        let mut mutated = rewritten.clone();
        let mut fragment = only_kernel(&rewritten).fragment.clone();
        share_address(&mut fragment.body);
        mutated
            .replace_kernel(0, fragment)
            .expect("a rewritten address is the same shape");
        Some(mutated)
    }
}

/// Point the second read of the surviving input at the first one's address.
fn share_address(body: &mut KernelBody) {
    let reads: Vec<ValueId> = body
        .blocks
        .iter()
        .flat_map(|block| block.instrs.iter().copied())
        .filter(|&value| {
            matches!(
                &body.values[value.0 as usize],
                ValueDef::Instr {
                    op: KernelInstr::BufferReadCall(_),
                    ..
                }
            )
        })
        .collect();
    let [_, second] = reads[..] else {
        panic!("the composed body holds two reads of the one surviving input: {reads:?}");
    };
    let first = match &body.values[reads[0].0 as usize] {
        ValueDef::Instr { args, .. } => args[1],
        _ => unreachable!("a buffer read names its address"),
    };
    let ValueDef::Instr { args, .. } = &mut body.values[second.0 as usize] else {
        unreachable!("the read came from a buffer read");
    };
    args[1] = first;
}

/// The diagnostic the merged body prints, so a failure names the body it failed on.
fn dump(label: &str, body: &KernelBody) {
    println!("--- {label}: {} value(s)", body.values.len());
    for (index, definition) in body.values.iter().enumerate() {
        println!("{index}: {definition:?}");
    }
    println!("blocks: {:#?}", body.blocks);
}

/// The consumer's own fragment, before any merge: it reads the absorbed buffer twice.
#[test]
fn the_recording_reads_one_buffer_at_two_addresses() {
    let (_module, instances, _schema, graph) = derived();
    assert_eq!(
        (graph.len(), graph.value_count()),
        (2, 4),
        "the recording is two nodes and four values"
    );
    let consumer = &instances[1].fragment;
    let addresses = read_addresses_at(&consumer.body, ABSORBED as i64);
    assert_eq!(
        addresses.len(),
        2,
        "the consumer reads the absorbed buffer twice, which is the shape under test: {addresses:?}"
    );
    assert_ne!(
        addresses[0], addresses[1],
        "and the two reads name two addresses: one value would answer both"
    );
    dump("the consumer", &consumer.body);
}

/// One copy of the producer's cone per read, each at its own address.
#[test]
fn the_merge_copies_the_producer_cone_once_per_read_at_that_read_s_address() {
    let (module, instances, schema, graph) = derived();
    println!(
        "the region's canonical form:\n{}the region's hash: {}",
        schema.canonical_form(),
        schema.hash
    );
    let (rewritten, lines) = consulted(&module, &graph, &[pack(&schema.hash.to_string())]);
    let rewritten = rewritten.expect("a licensed two-read region answers a graph: {lines:?}");
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 3),
        "the region becomes one node: {lines:?}"
    );
    let node = only_kernel(&rewritten);
    assert_eq!(
        node.inputs.len(),
        node.fragment.inputs,
        "the survivor is given the one input the composed fragment reads"
    );
    assert_eq!(
        node.fragment,
        compose(&instances[0].fragment, &instances[1].fragment, ABSORBED)
            .expect("the recorded two-read chain composes"),
        "the surviving node holds the composed fragment, element for element"
    );
    dump("the merged body", &node.fragment.body);
    let addresses = read_addresses_at(&node.fragment.body, 0);
    assert_eq!(
        addresses.len(),
        2,
        "one copy of the producer's cone per read of the absorbed buffer: {addresses:?}"
    );
    assert_ne!(
        addresses[0], addresses[1],
        "and the two copies are at two addresses: one copy reused for both reads would answer \
         `2 · e[i]`"
    );
}

/// Run the two-read chain three times on a device: as recorded, merged, and merged with both
/// copies at one address.
///
/// # Invariant
/// The three answers are the whole of the evidence, and the **counterfactual is the proof**: a
/// two-elementwise chain computes the same numbers unfused, so only an answer that moves with the
/// composed body says which node the device dispatched. `DEFINED` is how many leading elements
/// both runs define; a test with no device returns without asserting.
fn device_triple() {
    let name = std::thread::current()
        .name()
        .unwrap_or("recompute_reads")
        .to_string();
    let (stub, _guard) = common::stub();
    clear_graph_pass();
    // A stub computes nothing, so each leg proves its **dispatch record**.
    let before = stub.calls().len();
    let (base_module, _base_value, _root) = common::run(&running_recording(ELEMENTS));
    let recorded = stub.calls()[before..].to_vec();
    assert!(
        !recorded.is_empty(),
        "the program dispatches its own two kernels"
    );

    let (_module, _instances, schema, _graph) = derived();
    let packs = [pack(&schema.hash.to_string())];

    install_region_merge_with_packs(&base_module, &packs);
    let before = stub.calls().len();
    let (merged_module, _merged_value, _root) = common::run(&running_recording(ELEMENTS));
    let merged = stub.calls()[before..].to_vec();
    let report = pass_report(&merged_module);
    assert!(
        report.iter().any(|line| line.contains("licensed by pack")),
        "the run's own report says the pack licensed the merge: {report:?}"
    );
    // The merged leg's last dispatch is a node the recording did not: the composed body.
    let composed = merged.last().expect("the merged leg dispatches");
    for call in &recorded {
        assert_ne!(
            composed.fragment, call.fragment,
            "the composed node is not one the recording dispatched"
        );
    }

    set_graph_pass(Box::new(SharedAddress(region_merge_pass_with_packs(
        &base_module,
        &packs,
    ))));
    let before = stub.calls().len();
    let (_shared_module, _shared_value, _root) = common::run(&running_recording(ELEMENTS));
    let shared = stub.calls()[before..].to_vec();
    clear_graph_pass();
    let shared_composed = shared.last().expect("the shared address dispatches");
    assert_ne!(
        shared_composed.fragment, composed.fragment,
        "the answer moved with the second address, so the composed node is what the run dispatched"
    );
}

/// With the pack the device answers the program's own answer, and the counterfactual moves it.
#[test]
fn two_reads_merge_and_the_device_answers_the_program() {
    device_triple();
}

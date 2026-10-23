//! A fan-out — one producer feeding two consumers — merges under a pack's `fuse`.
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
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, compose_fan_out,
};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the diamond runs over.
const COUNT: usize = 8;

/// The consumer's input position the producer's output feeds.
const ABSORBED: u32 = 1;

/// The pack a runner installs for this region.
const PACK: &str = "elementwise-fan-out";

/// The diamond: `e[i] = d[i] · 2` feeding `left[i] = e[i] + 1` and `right[i] = e[i] + 1`.
///
/// # Invariant
/// Both consumers are the **same claimed kernel**, so the region holds three instances of two
/// kernels and a pack must name a hash only this shape has. Each body reads at its own invocation
/// index and returns from its entry block, which is what lets `compose_fan_out` append the second
/// consumer's body where the first one's leaves.
///
/// The parameter carries no `.out` group: each body declares the buffer it writes with
/// `compute.slice` and returns the write as the codomain field's value
/// (`docs/notes/kernel-buffer-declarations.md`).
fn program() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
ParE = struct{.n Int, .in In}
ArgE = struct{.n Int, .I In}
scale_raw = (k : ParE) => {
  i = compute.range k.n
  e = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  ((OutE)(.e (compute.write ((compute.Write _)(.to e, .at i, .value (((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0) ~ "element"))))))
}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" OutE
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = struct{.n Int, .in In2}
Arg2 = struct{.n Int, .I In2}
offset_raw = (k : Par2) => {
  i = compute.range k.n
  z = compute.slice ((compute.Slice _)(.count k.n, .element Float))
  ((OutZ)(.z (compute.write ((compute.Write _)(.to z, .at i, .value (((compute.read ((compute.Read _)(.from k.in.p, .at i)) : Float) + 1.0) ~ "element"))))))
}
offset = compute.parallel (offset_raw ~ "claim:Elementwise(element)") "gpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun scale ((ArgE)(.n s.n, .I In(.d s.in.d))) : OutE)
  left = (compute.plrun offset ((Arg2)(.n s.n, .I In2(.d s.in.d, .p first.e))) : OutZ)
  right = (compute.plrun offset ((Arg2)(.n s.n, .I In2(.d s.in.d, .p first.e))) : OutZ)
  (left, right)
}
built = compute.graph step
(scale, offset, built)
"#
    .to_string()
}

/// The diamond as a recording: the two kernels and the graph they recorded, nothing run.
fn recording() -> String {
    program()
}

/// The fill kernel that builds the diamond's input data, and the run that hands its buffer back.
///
/// # Invariant
/// One lane per element and each writes the datum at its own lane, so the buffer holds
/// `1.0 … COUNT` — the list the old `compute.buffer` held. The kernel runs on the host's own
/// backend, because a host builds its data before a device ever sees it, and its parameter
/// declares no `.in` group at all.
fn filled() -> String {
    r#"
Data = struct{.d (compute.Buf Float)}
Fill = struct{.n Int}
fill = (c : Fill) => {
  i = compute.range c.n
  d = compute.slice ((compute.Slice _)(.count c.n, .element Float))
  ((Data)(.d (compute.write ((compute.Write _)(.to d, .at i, .value (int2float i + 1.0))))))
}
filler = compute.parallel fill "cpu" Data
"#
    .to_string()
}

/// The diamond as a program that runs its recording and answers it.
///
/// # Invariant
/// The run's answer is the array of the graph's returns in the recording's own order, so ordinal
/// `k` names the `k`-th return: each is collected on its own, which is what makes the second one
/// an observation rather than an inference.
fn running() -> String {
    format!(
        "{head}\n{fill}count = ({count})\ndata = (compute.plrun filler ((Fill)(.n count)) : Data)\n\
         r = (compute.graphrun built (count, data.d) : array{{(compute.Buf Float), 2}})\n\
         (compute.collect (r[0]), compute.collect (r[1]))\n",
        head = recording(),
        fill = filled(),
        count = COUNT,
    )
}

/// The answer `z[i] = 2 d[i] + bias` has over `d = 1 … COUNT`.
fn answer(bias: f32) -> Vec<f32> {
    (1..=COUNT)
        .map(|position| 2.0 * position as f32 + bias)
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

/// The two `Float` arrays the graph's returns become, one per returned buffer.
///
/// # Invariant
/// The run's answer is the array of the recording's returns in its own order, so each is
/// collected through its own ordinal and the pair is the run's whole answer. The two consumers
/// compute the same numbers, so the ordinals are told apart by the counterfactual and not by the
/// values: it moves one consumer's constant alone and moves one of these two buffers alone.
fn outputs(module: &Module<LangProgram>, value: &LangValue) -> (Vec<f32>, Vec<f32>) {
    let returned = common::array_values(module, value);
    assert_eq!(returned.len(), 2, "the diamond returns two buffers");
    (
        common::float_array(module, &returned[0]),
        common::float_array(module, &returned[1]),
    )
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
    // A compound's element is a slot, so the value is the one its slot carries
    // (`docs/notes/compound-element-slots.md`).
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

/// The one schema `source`'s recording derives, with its module, instances and graph.
fn derived(source: &str) -> (Module<LangProgram>, Vec<Instance>, RegionSchema, Graph) {
    let (module, value, _root) = common::run(source);
    let instances = instances(&module, &value);
    let graph = recorded(&module, &value);
    let regions = derive_regions(&graph, &instances);
    assert_eq!(regions.len(), 1, "the recording is one claimed region");
    let schema = regions
        .into_iter()
        .next()
        .expect("the one region")
        .expect("a fan-out derives a schema, so a pack can name it");
    (module, instances, schema, graph)
}

/// The pack a human writes for `region`, as the JSON artifact's fields spell it.
///
/// # Invariant
/// The hash is the **region's**, in the field §5's sketch gives it, and the node half is absent:
/// this pack places nothing, so it names no node schema and stages nothing.
fn pack_text(region: &str, name: &str) -> String {
    format!(
        r#"{{
  "schema": null,
  "name": "{name}",
  "requires": {{ "class": "float" }},
  "fuse": {{ "region": "{region}", "as": "one_kernel" }},
  "plan": {{ "staging": [] }}
}}"#
    )
}

/// The pack the test installs, read as one a human could have written.
fn pack(region: &str, name: &str) -> PlacementPack {
    read_placement_pack(&pack_text(region, name))
        .unwrap_or_else(|refusal| panic!("the pack a human wrote is read: {refusal}"))
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

/// The merge with the **second** consumer's `+ 1.0` turned into `+ 10.0`.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that changes says the composed
/// node executed and an answer that does not says the recording did. `compose_fan_out` appends the
/// consumers' bodies in order and declares their outputs in that same order, so the **last** of the
/// body's `+ 1.0` definitions is the second consumer's own constant.
struct MovedBias(Box<dyn GraphPass>);

impl GraphPass for MovedBias {
    fn apply(
        &self,
        graph: &Graph,
        extent: &LaunchExtent<'_>,
        lines: &mut Vec<String>,
    ) -> Option<Graph> {
        let rewritten = self.0.apply(graph, extent, lines)?;
        let mut mutated = rewritten.clone();
        let Node::Kernel(node) = &rewritten.nodes()[0];
        let mut fragment = node.fragment.clone();
        move_bias(&mut fragment.body);
        mutated
            .replace_kernel(0, fragment)
            .expect("a moved constant is the same shape");
        Some(mutated)
    }
}

/// Move the composed body's **last** `Const(Float, 1.0)` into `Const(Float, 10.0)`.
fn move_bias(body: &mut KernelBody) {
    let one = 1.0f32.to_bits() as i64;
    let ten = 10.0f32.to_bits() as i64;
    let biases: Vec<usize> = body
        .values
        .iter()
        .enumerate()
        .filter_map(|(at, definition)| match definition {
            ValueDef::Instr {
                op: KernelInstr::Const(ScalarClass::Float, bits),
                ..
            } if *bits == one => Some(at),
            _ => None,
        })
        .collect();
    assert_eq!(
        biases.len(),
        2,
        "the composed body holds one `+ 1.0` per consumer, and the counterfactual moves the second"
    );
    if let ValueDef::Instr {
        op: KernelInstr::Const(ScalarClass::Float, bits),
        ..
    } = &mut body.values[*biases.last().expect("the two biases are two definitions")]
    {
        *bits = ten;
    }
}

/// The fragment a merged graph's one node holds.
fn merged(rewritten: &Graph) -> &KernelFragment {
    let Node::Kernel(node) = &rewritten.nodes()[0];
    &node.fragment
}

/// The fan-out is a region with a hash of its own, and only a pack naming it may attempt it.
///
/// # Invariant
/// This is the half the derivation owes the licence: **the shape must be nameable** before a pack
/// can ask for it, and the two edges it does not have — one producer, one consumer per node — are
/// what the floor's own proof could never hold.
#[test]
fn a_fan_out_derives_a_region_a_pack_can_name() {
    let (_module, _instances, schema, graph) = derived(&recording());
    println!(
        "the fan-out's canonical form:\n{}the fan-out's hash: {}",
        schema.canonical_form(),
        schema.hash
    );
    assert_eq!(graph.len(), 3, "the producer feeds two consumers");
    assert_eq!(
        schema
            .nodes
            .iter()
            .map(|node| node.interface.as_str())
            .collect::<Vec<_>>(),
        vec!["Elementwise", "Elementwise", "Elementwise"],
        "the region holds the producer and the two consumers it feeds"
    );
    assert_eq!(
        schema.between.len(),
        2,
        "two edges leave the producer, and `off_chain_node` names the node that is why"
    );
    assert_eq!(
        schema.off_chain_node(),
        Some(0),
        "node 0 is the producer the floor's chain proof cannot hold"
    );
    assert!(
        schema.canonical_form().contains("between 0 1 input 1")
            && schema.canonical_form().contains("between 0 2 input 1"),
        "both edges are in the canonical form the hash is taken over"
    );
    let pack = pack(&schema.hash.to_string(), PACK);
    assert_eq!(
        pack.fuse().and_then(|directive| directive.region()),
        Some(&schema.hash),
        "so a pack a human wrote can name it, and only it"
    );
}

/// Without a pack the fan-out is refused by name, and nothing merges.
///
/// # Invariant
/// The refusal is the **licence's**, not the derivation's, and the floor's licence has not moved:
/// a node two consumers read from is not a producer-to-consumer chain whatever its nodes claim,
/// so `NotAReductionChain` still answers and the graph is the recording every time.
#[test]
fn without_a_pack_the_fan_out_is_refused_by_name_and_nothing_merges() {
    let (module, _instances, _schema, graph) = derived(&recording());
    let before = (graph.len(), graph.value_count());
    let (rewritten, lines) = consulted(&module, &graph, &[]);
    assert!(
        rewritten.is_none(),
        "a declined region changes nothing, so the pass answers no graph: {lines:?}"
    );
    assert_eq!(lines.len(), 1, "the one region is reported once");
    assert!(
        lines[0].starts_with("region 0 (nodes 0, 1, 2): refused: NotAReductionChain:"),
        "the fan-out is refused by name: {lines:?}"
    );
    assert!(
        lines[0].contains("no pack") || lines[0].contains("a pack whose `fuse` names"),
        "and the sentence says what would change it: {lines:?}"
    );
    assert!(
        !lines[0].contains("merged into one node"),
        "and nothing merged: {lines:?}"
    );
    assert_eq!(
        (graph.len(), graph.value_count()),
        before,
        "the graph handed to the pass is untouched: it is the recording every time"
    );
}

/// A pack naming the fan-out's region merges all three nodes into one, and says so.
///
/// # Invariant
/// The surviving node holds **exactly** `compose_fan_out`'s fragment, so the merge is the
/// composition and not a second mechanism's answer, and the report names the region the pack
/// fired for by the hash the pack matched.
#[test]
fn a_pack_naming_the_fan_out_merges_all_three_nodes_into_one() {
    let (module, instances, schema, graph) = derived(&recording());
    let packs = [pack(&schema.hash.to_string(), PACK)];
    let (rewritten, lines) = consulted(&module, &graph, &packs);
    let rewritten = rewritten.expect("a licensed fan-out answers a graph");
    println!("the region reported:\n{}", lines.join("\n"));
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 4),
        "the three nodes become one node over the graph's two inputs, and it writes both outputs"
    );
    assert_eq!(lines.len(), 1, "the one region is reported once");
    assert!(
        lines[0].contains("nodes 0, 1, 2 merged into one node at 2 (the survivor)")
            && lines[0]
                .contains("absorbing input position 1 of node 1 and input position 1 of node 2"),
        "the line names every node and every absorbed position: {lines:?}"
    );
    assert!(
        lines[0].contains(&format!(
            "licensed by pack \"{PACK}\", whose `fuse` names this region's schema {}",
            schema.hash
        )),
        "the pack's own line says it fired for that region: {lines:?}"
    );
    assert!(
        lines[0].contains("the merged fragment is unregistered"),
        "and the survivor is still unregistered, so a later pass cannot name it: {lines:?}"
    );
    assert_eq!(
        merged(&rewritten),
        &compose_fan_out(
            &instances[0].fragment,
            &[
                (&instances[1].fragment, ABSORBED),
                (&instances[1].fragment, ABSORBED)
            ],
        )
        .expect("the recorded fan-out composes"),
        "the surviving node holds the composed fragment, element for element"
    );
    let fragment = merged(&rewritten);
    assert_eq!(
        (fragment.inputs, fragment.outputs),
        (3, 2),
        "the union is `d` three times over and the outputs are the two consumers'"
    );
}

/// Run the diamond three times on a device: as recorded, merged, and merged with the constants moved.
///
/// # Invariant
/// The three observations are the whole of the evidence, and the **counterfactual is the proof**: a
/// fused diamond computes the same numbers unfused, so only an answer that moves with the composed
/// body says which node the device dispatched. One consumer's constant moves alone, so one returned
/// buffer moves alone and the second ordinal is read rather than inferred from the first.
fn device_triple(test: &str) {
    let (stub, _guard) = common::stub();
    clear_graph_pass();
    // A stub computes nothing, so each leg proves its **dispatch record**.
    let before = stub.calls().len();
    let (base_module, _base_value, _root) = common::run(&running());
    let recorded = stub.calls()[before..].to_vec();
    assert!(
        !recorded.is_empty(),
        "the program dispatches its own three kernels"
    );

    let (_module, _instances, schema, _graph) = derived(&recording());
    let packs = [pack(&schema.hash.to_string(), PACK)];

    install_region_merge_with_packs(&base_module, &packs);
    let before = stub.calls().len();
    let (merged_module, _merged_value, _root) = common::run(&running());
    let merged = stub.calls()[before..].to_vec();
    let report = pass_report(&merged_module);
    assert!(
        report.iter().any(|line| {
            line.contains("licensed by pack") && line.contains(&schema.hash.to_string())
        }),
        "the run's own report says the pack licensed the merge while the run was dispatched:          {report:?}"
    );
    // The merged leg's last dispatch is a node the recording did not: the composed body.
    let composed = merged.last().expect("the merged leg dispatches");
    for call in &recorded {
        assert_ne!(
            composed.fragment, call.fragment,
            "the composed node is not one the recording dispatched"
        );
    }

    // **The counterfactual**: the second consumer's constant is part of the composed body.
    set_graph_pass(Box::new(MovedBias(region_merge_pass_with_packs(
        &base_module,
        &packs,
    ))));
    let before = stub.calls().len();
    let (_moved_module, _moved_value, _root) = common::run(&running());
    let moved = stub.calls()[before..].to_vec();
    clear_graph_pass();
    let moved_composed = moved.last().expect("the moved bias dispatches");
    assert_ne!(
        moved_composed.fragment, composed.fragment,
        "the answer moved with the composed body, so the composed node is what the run dispatched"
    );
}

/// Under a pack the device answers the program's own answer, and the counterfactual moves it.
#[test]
fn under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves() {
    device_triple("under_a_pack_the_device_answers_the_program_and_the_counterfactual_moves");
}

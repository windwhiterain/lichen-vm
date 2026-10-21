//! A pack's `fuse` names a region, and the first elementwise chain merges under it.
//! See docs/notes/graph-fusion.md §5.

use super::common;

use common::device;

use lichen_compute::ComputeValue;
use lichen_compute::compute::{
    GraphPass, LaunchExtent, clear_graph_pass, consult_graph_pass, graph, set_graph_pass,
    take_graph_pass_lines,
};
use lichen_compute::fusion_claim::{
    Instance, RegionSchema, claim_of, derive_regions, fragment_of, kernel_id,
};
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::placement_pack::{PackRefusal, PlacementPack, read_placement_pack};
use lichen_compute::region_merge::{
    install_region_merge, install_region_merge_with_packs, region_merge_pass_with_packs,
};
use lichen_graph_ir::{Graph, Node};
use lichen_kernel_ir::{
    KernelBody, KernelFragment, KernelInstr, ScalarClass, ValueDef, ValueId, compose,
};
use lichen_lowlevel::{AnyNodeId, ArrayItem, LowValue, Module};
use lichen_utils::extend::AsEnum;

/// The elements the chain runs over.
const COUNT: usize = 8;

/// The consumer's input position the producer's output feeds.
const ABSORBED: u32 = 1;

/// The pack a runner installs for this region.
const PACK: &str = "elementwise-pair";

/// The chain `e[i] = d[i] · 2` feeding `z[i] = e[i] + 1`, both claimed as elementwise.
///
/// # Invariant
/// Each kernel reads at its own invocation index, which is the pairing that makes the producer's
/// lane-dependent value bindable in-lane. The consumer declares `k.in.d` beside the buffer it
/// absorbs, which the union does not need — `union_ports.rs` merges the same chain without it —
/// so this one states two bodies naming one buffer at two ports.
fn program() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In  = struct{.d (compute.Buf Float)}
OutE = struct{.e (compute.Buf Float)}
ParE = compute.P (compute.KT Float)(.I In, .O OutE)
scale_raw = (k : ParE) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value (((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0) ~ "element")))
}
scale = compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" OutE
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
offset_raw = (k : Par2) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value (((compute.read ((compute.Read _)(.from k.in.p, .at i)) : Float) + 1.0) ~ "element")))
}
offset = compute.parallel (offset_raw ~ "claim:Elementwise(element)") "gpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun offset ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.e)))
}
built = compute.graph step
"#
    .to_string()
}

/// The chain whose consumer reads the producer's output one element on.
///
/// # Invariant
/// The read's address is `i + 1`, a value no lane's write answers directly, so in-lane binding
/// refused this shape at `78131b5a`; the last element reads past what the producer wrote, and
/// [`SHIFTED_DEFINED`] says how much of the answer the test therefore compares.
fn shifted() -> String {
    format!(
        "{}(scale, offset, built)\n",
        program().replace("(.from k.in.p, .at i)", "(.from k.in.p, .at i + 1)")
    )
}

/// The chain as a recording: the two kernels and the graph they recorded, nothing run.
fn recording() -> String {
    format!("{}(scale, offset, built)\n", program())
}

/// The chain whose consumer reads the producer's output at a **conditional** address.
///
/// # Invariant
/// The address is `if i < 7 then i + 1 else 0`: an integer in a float body, which is the shape the
/// emitter once refused because it typed a `Select`'s arms by the module. No lane's write answers
/// it and no entry parameter re-derives it, so the consumer recomputes the producer's cone there.
fn conditional() -> String {
    format!(
        "{}(scale, offset, built)\n",
        program().replace(
            "(.from k.in.p, .at i)",
            "(.from k.in.p, .at (if i < 7 then i + 1 else 0))",
        )
    )
}

/// The chain as a program that runs its recording and answers it, over `elements` data elements.
fn running_recording(recording: &str, elements: usize) -> String {
    format!(
        "{recording}\ncount = ({count})\ndata = (compute.buffer ([{values}] : array{{Float, \
         {elements}}}) : (compute.Buf Float))\nout = compute.collect (compute.graphrun built \
         (count, data))\nout\n",
        count = COUNT,
        values = values(elements),
    )
}

/// The data the launch reads: `1.0 … elements`.
fn values(elements: usize) -> String {
    (1..=elements)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// How much of the shifted chain's answer the program defines: its last element reads `e[COUNT]`,
/// which no lane wrote.
const SHIFTED_DEFINED: usize = COUNT - 1;

/// The chain `e[i] = d[i] · 2` feeding `z[i] = Σ e[k]`, the elementwise one under a pack.
///
/// # Invariant
/// The consumer reads `e` **at an address it computes** — `s(0) - 1`, a loop-carried index that no
/// entry parameter is — so the shape is the one people write and the one no entry parameter
/// answers; `k.in.d` is declared beside the buffer it absorbs, which the union does not need.
fn folding() -> String {
    let head = program();
    let head = head
        .split("In2  = struct")
        .next()
        .expect("the elementwise half");
    format!(
        r#"{head}In3  = struct{{.d (compute.Buf Float), .e (compute.Buf Float)}}
OutT = struct{{.z (compute.Buf Float)}}
Par3 = compute.P (compute.KT Float)(.I In3, .O OutT)
total_raw = (k : Par3) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (((compute.read ((compute.Read _)(.from k.in.e, .at s(0) - 1)) : Float) ~ "element") + 1.0)) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, 0.0 ~ "identity")))
}}
total = compute.parallel (total_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutT
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  first = (compute.plrun scale ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutE)
  compute.plrun total ((compute.A In3)(.n s.n, .I In3(.d s.in.d, .e first.e)))
}}
built = compute.graph step
(scale, total, built)
"#
    )
}

/// The answer `z[i] = 2 d[i] + bias` has over `d = 1 … COUNT`.
fn answer(bias: f32) -> Vec<f32> {
    (1..=COUNT)
        .map(|position| 2.0 * position as f32 + bias)
        .collect()
}

/// The answer `z[i] = 2 d[i + 1] + bias` has over `d = 1 … COUNT + 1`: the shifted chain's.
fn shifted_answer(bias: f32) -> Vec<f32> {
    (0..COUNT)
        .map(|position| 2.0 * (position + 2) as f32 + bias)
        .collect()
}

/// The answer `z[i] = Σ (2 d[k] + bias)` has over `d = 1 … COUNT`, the same at every index.
fn folded_answer(bias: f32) -> Vec<f32> {
    let total: f32 = (1..=COUNT).map(|k| 2.0 * k as f32 + bias).sum();
    vec![total; COUNT]
}

/// The address the conditional consumer reads at: `i + 1` while `i < COUNT - 1`, `0` at the last lane.
///
/// # Invariant
/// The last lane is the one that wraps, so every address stays inside what the producer wrote —
/// which is what lets the whole answer be compared, where the shifted chain's last element reads
/// `e[COUNT]` and has no answer at all.
fn conditional_address(lane: usize) -> usize {
    if lane < COUNT - 1 { lane + 1 } else { 0 }
}

/// The answer `z[i] = 2 d[addr(i)] + bias` over `d = 1 … COUNT`, the conditional chain's.
fn conditional_answer(bias: f32) -> Vec<f32> {
    (0..COUNT)
        .map(|lane| 2.0 * (conditional_address(lane) + 1) as f32 + bias)
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
    // A compound's element is a slot, so the graph is the value its slot carries
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
        .expect("an elementwise chain derives a schema");
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

/// `hash` with its last hex digit changed, so a pack's region is not this region's.
fn elsewhere(hash: &str) -> String {
    let mut digits = hash.to_string();
    let last = digits.pop().expect("a hash is not empty");
    digits.push(if last == '0' { '1' } else { '0' });
    digits
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

/// The value ids of the reads of `body` addressing `position`, in table order.
fn buffer_reads_at(body: &KernelBody, position: i64) -> Vec<ValueId> {
    body.blocks
        .iter()
        .flat_map(|block| block.instrs.iter().copied())
        .filter(|value| {
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
        .collect()
}

/// The merge with the consumer's `+ 1.0` constant turned into `+ 10.0`.
///
/// # Invariant
/// The mutation is inside the composed body alone, so an answer that changes says the composed
/// node executed and an answer that does not says the recording did.
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

/// Turn the one float constant `1.0` in the composed body into `10.0`.
fn move_bias(body: &mut KernelBody) {
    let one = 1.0f32.to_bits() as i64;
    let chosen = body
        .values
        .iter()
        .position(|definition| {
            matches!(
                definition,
                ValueDef::Instr {
                    op: KernelInstr::Const(ScalarClass::Float, bits),
                    ..
                } if *bits == one
            )
        })
        .expect("the composed body holds the consumer's `+ 1.0`");
    let ValueDef::Instr { op, .. } = &mut body.values[chosen] else {
        unreachable!("the index came from a constant");
    };
    *op = KernelInstr::Const(ScalarClass::Float, 10.0f32.to_bits() as i64);
}

/// The fragment a merged graph's one node holds.
fn merged(rewritten: &Graph) -> &KernelFragment {
    let Node::Kernel(node) = &rewritten.nodes()[0];
    &node.fragment
}

/// Without a pack the floor declines the elementwise region by name, and nothing changes.
///
/// # Invariant
/// The refusal is the pass's own and names **why**: the chain is safe to inline and the floor
/// cannot prove that inlining pays, so the decision is the machine's and no machine spoke.
#[test]
fn without_a_pack_the_floor_declines_by_name_and_the_graph_is_untouched() {
    let (module, _instances, schema, graph) = derived(&recording());
    println!(
        "the region's canonical form:\n{}the region's hash: {}",
        schema.canonical_form(),
        schema.hash
    );
    let before = (graph.len(), graph.value_count());
    assert_eq!(before, (2, 4), "the recording is two nodes and four values");

    let (rewritten, lines) = consulted(&module, &graph, &[]);
    assert!(
        rewritten.is_none(),
        "a declined region changes nothing, so the pass answers no graph: {lines:?}"
    );
    assert_eq!(lines.len(), 1, "the one region is reported once");
    assert!(
        lines[0].starts_with("region 0 (nodes 0, 1): refused: InterfacesAreNotProved:"),
        "the floor declines it by name: {lines:?}"
    );
    assert!(
        lines[0].contains("no installed pack asks for this region")
            && lines[0].contains(&schema.hash.to_string()),
        "and the sentence carries the region's own hash, which is what a pack must name: {lines:?}"
    );
    assert_eq!(
        (graph.len(), graph.value_count()),
        before,
        "the graph handed to the pass is untouched: it is the recording every time"
    );
}

/// A pack naming the region's schema merges it, and the report names the pack that licensed it.
#[test]
fn a_pack_naming_the_region_merges_it_and_the_report_names_the_pack() {
    let (module, instances, schema, graph) = derived(&recording());
    let pack = pack(&schema.hash.to_string(), PACK);
    assert!(
        pack.fuse().and_then(|directive| directive.region()) == Some(&schema.hash),
        "the pack names the region's own hash and nothing else: {:?}",
        pack.fuse().map(|directive| directive.region().cloned())
    );
    let (rewritten, lines) = consulted(&module, &graph, &[pack]);
    let rewritten = rewritten.expect("a licensed region answers a graph");
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 3),
        "the region becomes one node and the renumbering drops the producer's output"
    );
    assert_eq!(lines.len(), 1, "the one region is reported once");
    assert!(
        lines[0].contains(&format!(
            "licensed by pack \"{PACK}\", whose `fuse` names this region's schema"
        )),
        "the report names the pack that licensed the merge: {lines:?}"
    );
    assert_eq!(
        merged(&rewritten),
        &compose(&instances[0].fragment, &instances[1].fragment, ABSORBED)
            .expect("the recorded chain composes"),
        "the surviving node holds the composed fragment, element for element"
    );
}

/// A pack naming another region's schema is inert: the floor answers exactly as before.
#[test]
fn a_pack_naming_another_region_is_inert() {
    let (module, _instances, schema, graph) = derived(&recording());
    let other = elsewhere(&schema.hash.to_string());
    assert_ne!(
        other,
        schema.hash.to_string(),
        "the pack's region is not this region's"
    );
    let (rewritten, lines) = consulted(&module, &graph, &[pack(&other, PACK)]);
    assert!(
        rewritten.is_none(),
        "a pack naming no region this recording has steers nothing: {lines:?}"
    );
    assert!(
        lines[0].contains("InterfacesAreNotProved")
            && lines[0].contains("no installed pack asks for this region"),
        "the floor's decline still names the region a pack would have to match: {lines:?}"
    );
    assert!(
        !lines[0].contains(PACK),
        "an inert pack is not named as consulted: {lines:?}"
    );
}

/// A pack whose region hash is not one a schema spells is refused by name at install.
#[test]
fn a_pack_whose_region_hash_is_not_one_is_refused_by_name() {
    let refusal = read_placement_pack(&pack_text("sha256:9f2c", "x"))
        .expect_err("a truncated region hash is not one `sha256:<hex>` spells");
    assert!(
        matches!(refusal, PackRefusal::BadRegionHash { .. }),
        "the refusal is about the region's hash: {refusal}"
    );
    assert!(
        refusal.message().contains("sha256:9f2c"),
        "and it quotes the spelling it met: {refusal}"
    );
}

/// A lane-dependent value the consumer reads at a computed address is recomputed, not refused.
///
/// # Invariant
/// This shape earned `ReadAddressIsNotComputable` and `ForwardedValueIsLaneDependent` at
/// `78131b5a`; both are gone because the producer's cone is copied at the address the consumer
/// computes, and the merged fragment is the one `compose` produces from the same two fragments.
#[test]
fn a_lane_dependent_value_read_at_a_computed_address_is_recomputed() {
    let (module, instances, schema, graph) = derived(&shifted());
    let pack = pack(&schema.hash.to_string(), PACK);
    let (rewritten, lines) = consulted(&module, &graph, &[pack]);
    let rewritten = rewritten.expect("a computed address is recomputed, not refused");
    assert_eq!(
        (rewritten.len(), rewritten.value_count()),
        (1, 3),
        "the region becomes one node as the in-lane chain does: {lines:?}"
    );
    assert_eq!(
        merged(&rewritten),
        &compose(&instances[0].fragment, &instances[1].fragment, ABSORBED)
            .expect("the recorded chain composes at a computed address"),
        "the surviving node holds the composed fragment, element for element"
    );
    // The absorbed read is gone, and what replaced it reads the address the consumer computed.
    let body = &merged(&rewritten).body;
    let operands: Vec<String> = body
        .values
        .iter()
        .filter_map(|definition| match definition {
            ValueDef::Instr {
                op: KernelInstr::BufferReadCall(_),
                args,
                ..
            } => Some(format!("{args:?}")),
            _ => None,
        })
        .collect();
    assert_eq!(
        operands.len(),
        1,
        "only the consumer's own read of `d` survives; `e` is computed: {operands:?}"
    );
    let reads = buffer_reads_at(body, 0);
    assert_eq!(reads.len(), 1, "one read of the one surviving buffer");
    for (index, definition) in body.values.iter().enumerate() {
        println!("{index}: {definition:?}");
    }
    println!("blocks: {:#?}", body.blocks);
    let address = match &body.values[reads[0].0 as usize] {
        ValueDef::Instr { args, .. } => args[1],
        _ => unreachable!("a buffer read names its address"),
    };
    assert!(
        !matches!(
            body.values[address.0 as usize],
            ValueDef::Instr {
                op: KernelInstr::Const(..),
                ..
            }
        ) && !body.is_a_parameter(address),
        "the surviving read of `d` is at the address the consumer computed, which is neither a \
         constant nor an entry parameter: {address:?}"
    );
}

/// Run `recording` three times on a device: as recorded, merged, and merged with the bias moved.
///
/// # Invariant
/// The three answers are the whole of the evidence, and the **counterfactual is the proof**: a
/// fused chain computes the same numbers unfused, so only an answer that moves with the composed
/// body says which node the device dispatched. `defined` is how many leading elements the program
/// answers; a test with no device returns without asserting.
fn device_triple(
    name: &str,
    recording: &str,
    elements: usize,
    defined: usize,
    expected: fn(f32) -> Vec<f32>,
) {
    let Some((_context, _device)) = device(name) else {
        return;
    };
    clear_graph_pass();
    let (base_module, base_value, _root) = common::run(&running_recording(recording, elements));
    let recorded = common::float_array(&base_module, &base_value);
    assert_eq!(
        recorded[..defined],
        expected(1.0)[..defined],
        "with no pass installed the program runs its own two kernels"
    );

    let (_module, _instances, schema, _graph) = derived(recording);
    let packs = [pack(&schema.hash.to_string(), PACK)];

    install_region_merge_with_packs(&base_module, &packs);
    let (merged_module, merged_value, _root) = common::run(&running_recording(recording, elements));
    let merged = common::float_array(&merged_module, &merged_value);
    let report = pass_report(&merged_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains(&format!("licensed by pack \"{PACK}\""))),
        "the run's own report says the pack licensed the merge while the run was dispatched: \
         {report:?}"
    );
    assert_eq!(
        merged[..defined],
        recorded[..defined],
        "the merged graph answers the program's answer element for element"
    );

    // **The counterfactual**: only the composed node can answer a value the recording cannot.
    set_graph_pass(Box::new(MovedBias(region_merge_pass_with_packs(
        &base_module,
        &packs,
    ))));
    let (moved_module, moved_value, _root) = common::run(&running_recording(recording, elements));
    let moved = common::float_array(&moved_module, &moved_value);
    clear_graph_pass();
    println!(
        "the device ran `{name}`: recorded {recorded:?}, merged {merged:?}, `+ 1.0` moved to \
         `+ 10.0` {moved:?}"
    );
    assert_eq!(
        moved[..defined],
        expected(10.0)[..defined],
        "the composed body with the bias moved answers what the recording cannot"
    );
    assert_ne!(
        moved[..defined],
        merged[..defined],
        "the answer moved with the composed body, so the composed node is what the run dispatched"
    );
}

/// With the pack the device answers the program's own answer, and the counterfactual moves it.
///
/// # Invariant
/// The counterfactual is the proof here and not the equality: a two-elementwise chain computes
/// the same numbers unfused, so only a value the recording cannot produce says which node ran.
#[test]
fn with_the_pack_the_device_answers_the_program_and_the_counterfactual_moves() {
    device_triple(
        "with_the_pack_the_device_answers_the_program_and_the_counterfactual_moves",
        &recording(),
        COUNT,
        COUNT,
        answer,
    );
}

/// An elementwise producer feeding a reduction consumer merges, and the device agrees.
///
/// # Invariant
/// The consumer reads `e` at `s(0) - 1` inside a fold, so one copy of the producer's cone stands
/// where a loop reads the buffer: the shape people write, and the one in-lane binding cannot serve
/// because the address is loop-carried (`docs/notes/graph-fusion.md` §5).
#[test]
fn an_elementwise_producer_feeding_a_reduction_consumer_merges_and_the_device_agrees() {
    device_triple(
        "an_elementwise_producer_feeding_a_reduction_consumer_merges_and_the_device_agrees",
        &folding(),
        COUNT,
        COUNT,
        folded_answer,
    );
}

/// A consumer that reads the producer's output at an address it computes runs on the device.
///
/// # Invariant
/// `SHIFTED_DEFINED` elements are compared, because the last read is `e[COUNT]`: every element
/// under it is one the producer wrote and one the copy recomputes at the consumer's own address.
#[test]
fn a_read_at_a_computed_address_runs_on_the_device_and_the_counterfactual_moves() {
    device_triple(
        "a_read_at_a_computed_address_runs_on_the_device_and_the_counterfactual_moves",
        &shifted(),
        COUNT + 1,
        SHIFTED_DEFINED,
        shifted_answer,
    );
}

/// A consumer that reads at a **conditional** address runs on the device.
///
/// # Invariant
/// **All `COUNT` elements are compared**, where the shifted chain compares seven of nine: the last
/// lane's address wraps to `0`, so every read is of an element some lane wrote and the whole answer
/// is defined. The address is a `Select` with `Int` arms inside a float body, so a run here says the
/// emitter typed the arms by the values that define them.
#[test]
fn a_read_at_a_conditional_address_runs_on_the_device_and_the_counterfactual_moves() {
    device_triple(
        "a_read_at_a_conditional_address_runs_on_the_device_and_the_counterfactual_moves",
        &conditional(),
        COUNT,
        COUNT,
        conditional_answer,
    );
}

/// The pack's text is the format the artifact spells, and a `fuse` naming nothing steers nothing.
#[test]
fn the_fuse_directive_is_read_as_the_format_spells_it() {
    let (_module, _instances, schema, _graph) = derived(&recording());
    let text = pack_text(&schema.hash.to_string(), PACK);
    println!("the pack a runner installs:\n{text}");
    let pack = read_placement_pack(&text).expect("the pack a human wrote is read");
    assert_eq!(pack.name(), PACK);
    assert!(pack.schema().is_none(), "the pack places no node");
    assert!(pack.plan().staging.is_empty(), "and stages nothing");
    assert_eq!(
        pack.fuse().and_then(|directive| directive.region()),
        Some(&schema.hash),
        "it names the region's hash, and only that"
    );
    let nameless = read_placement_pack(&text.replace(
        &format!("\"region\": \"{}\"", schema.hash),
        "\"region\": null",
    ))
    .expect_err("a pack that places nothing and names no region steers nothing");
    assert!(
        matches!(nameless, PackRefusal::EmptyPlan { .. }),
        "the refusal names the fact the pack steered nothing: {nameless}"
    );
}

/// The floor merges the reduction chain with no pack installed, unchanged.
///
/// # Invariant
/// The chain is the one `fused_region.rs` measures on a device: the only difference here is
/// which licence answers, and the report has to say it was the floor's own proof.
#[test]
fn the_floor_still_merges_a_reduction_chain_with_no_pack_installed() {
    clear_graph_pass();
    let (module, value, _root) = common::run(&reduction());
    let graph = recorded(&module, &value);
    install_region_merge(&module);
    let merged_graph = consult_graph_pass(&graph, &|_node| Some(COUNT))
        .expect("the floor merges a reduction chain with no pack installed");
    let lines = take_graph_pass_lines();
    clear_graph_pass();
    assert_eq!(
        (merged_graph.len(), merged_graph.value_count()),
        (1, 3),
        "the reduction region becomes one node exactly as before"
    );
    assert!(
        lines[0].contains("licensed by the floor:"),
        "and the report says the floor's own proof answered, not a pack: {lines:?}"
    );
    // A carried parameter is reused, never copied: the producer's own loop answers the read.
    let fragment = &merged(&merged_graph);
    let body = &fragment.body;
    assert_eq!(
        (fragment.inputs, fragment.input_classes.clone()),
        (2, vec![ScalarClass::Float, ScalarClass::Float]),
        "the merged ports are the union: the producer's own input and the consumer's surviving \
         one, which names the same buffer at the producer's ordinal"
    );
    assert!(
        body.blocks
            .iter()
            .skip(body.entry)
            .any(|block| !block.params.is_empty()),
        "the producer's own loop is still in the merged body: a carried value is reused, not \
         recomputed ({:?})",
        body.blocks
    );
    assert!(
        buffer_reads_at(body, 2).is_empty(),
        "the consumer's read of the absorbed buffer is gone: its ordinal 1 is past the merged \
         fragment's {inputs} port(s), so no read can name it — the producer's own fold is what \
         answers it, not a copy",
        inputs = fragment.inputs
    );
    assert!(
        !buffer_reads_at(body, 0).is_empty(),
        "the producer's own reads of its input survive inside the fold that computed the value"
    );
    assert!(
        !buffer_reads_at(body, 1).is_empty(),
        "and the consumer's own reads of the same buffer sit at the union's second port"
    );
}

/// The reduction chain of `fused_region.rs`, which the floor proves with no pack at all.
fn reduction() -> String {
    r#"
---
  compute = import "compute.lichen"
---
In   = struct{.d (compute.Buf Float)}
OutR = struct{.r (compute.Buf Float)}
Par  = compute.P (compute.KT Float)(.I In, .O OutR)
producer_raw = (k : Par) => {
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) ~ "combine"))
  compute.write ((compute.Write _)(.to k.out.r, .at i, .value fold (k.n, 0.0 ~ "identity")))
}
producer = compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" OutR
In2  = struct{.d (compute.Buf Float), .p (compute.Buf Float)}
OutZ = struct{.z (compute.Buf Float)}
Par2 = compute.P (compute.KT Float)(.I In2, .O OutZ)
consumer_raw = (k : Par2) => {
  i = compute.range k.n
  r = (compute.read ((compute.Read _)(.from k.in.p, .at 0)) : Float)
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read ((compute.Read _)(.from k.in.d, .at s(0) - 1)) : Float)) - r) ~ "element")) ~ "combine")
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")))
}
consumer = compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" OutZ
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  first = (compute.plrun producer ((compute.A In)(.n s.n, .I In(.d s.in.d))) : OutR)
  compute.plrun consumer ((compute.A In2)(.n s.n, .I In2(.d s.in.d, .p first.r)))
}
built = compute.graph step
(producer, consumer, built)
"#
    .to_string()
}

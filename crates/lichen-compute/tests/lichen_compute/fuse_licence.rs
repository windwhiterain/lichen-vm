//! A pack's `fuse` names a region, and the first elementwise chain merges under it.
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
const PACK: &str = "elementwise-expr";

/// The recording's producer, in the stub's own call order.
///
/// # Invariant
/// Call 0 is the fill that built the data, so the producer is call 1 and the consumer is
/// call 2: a host builds its data with a kernel before the device ever sees it
/// (`docs/notes/kernel-buffer-declarations.md`).
const PRODUCER_CALL: usize = 1;

/// The chain `e[i] = d[i] · 2` feeding `z[i] = e[i] + 1`, both claimed as elementwise.
///
/// # Invariant
/// Each kernel reads at its own invocation index, the pairing that makes the producer's
/// lane-dependent value bindable in-lane. The consumer declares `k.in.d` beside the buffer it
/// absorbs, which the union does not need.
///
/// No parameter carries an `.out` group: each body declares its buffer with `compute.buf` and
/// returns the write as the codomain field's value.
fn program(elements: usize) -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {elements}}}))}}
OutE := _x => {{.e (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
ParE := _x => {{.n @any : Int, .in In _}}
scale_raw := (k <: ParE) => {{
  i := compute.range k.n
  e := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  ((OutE){{.e (compute.write (compute.Write{{.to e, .at i, .value (((compute.read (compute.Read{{.from k.in.d, .at i}}) : Float) * 2.0) ~ "element")}}))}})
}}
scale := compute.parallel (scale_raw ~ "claim:Elementwise(element)") "gpu" (OutE _)
In2  := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {elements}}})), .p (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutZ := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par2 := _x => {{.n @any : Int, .in In2 _}}
offset_raw := (k <: Par2) => {{
  i := compute.range k.n
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  ((OutZ){{.z (compute.write (compute.Write{{.to z, .at i, .value (((compute.read (compute.Read{{.from k.in.p, .at i}}) : Float) + 1.0) ~ "element")}}))}})
}}
offset := compute.parallel (offset_raw ~ "claim:Elementwise(element)") "gpu" (OutZ _)
GArg := _x => {{.n @any : Int, .in In _}}
step := (s <: GArg) => {{
  first := (compute.plrun scale (compute.A{{.n s.n, .I In{{.d s.in.d}}}}) : Struct)
  compute.plrun offset (compute.A{{.n s.n, .I In2{{.d s.in.d, .p first.e}}}})
}}
built := compute.graph step
"#
    )
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
        program(COUNT + 1).replace("(.from k.in.p, .at i)", "(.from k.in.p, .at i + 1)")
    )
}

/// The chain as a recording: the two kernels and the graph they recorded, nothing run.
fn recording() -> String {
    format!("{}(scale, offset, built)\n", program(COUNT))
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
        program(COUNT).replace(
            "(.from k.in.p, .at i)",
            "(.from k.in.p, .at (if i < 7 then i + 1 else 0))",
        )
    )
}

/// The chain as a program that runs its recording and answers it, over `elements` data elements.
fn running_recording(recording: &str, elements: usize) -> String {
    format!(
        "{recording}\n{fill}\ncount := ({count})\ndata := (compute.plrun filler ((Fill){{.n {elements}}}) : Struct)\n\
         out := compute.collect (compute.graphrun built (count, data.d))\nout\n",
        fill = filled(elements),
        count = COUNT,
    )
}

/// The fill kernel that builds the launch's input data, and the run that hands its buffer back.
///
/// # Invariant
/// One lane per element and each writes its own element, so the buffer holds `1.0 … elements`.
/// The kernel runs on the host's own backend, because a host builds its data before a device ever
/// sees it, and its parameter declares no `.in` group at all
/// (`docs/notes/kernel-buffer-declarations.md`).
fn filled(elements: usize) -> String {
    format!(
        r#"
Data := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {elements}}}))}}
Fill := _x => {{.n @any : Int}}
fill := (c <: Fill) => {{
  i := compute.range c.n
  d := compute.buf (compute.BufParam{{.type Float, .length {elements}}})
  ((Data){{.d (compute.write (compute.Write{{.to d, .at i, .value (int2float i + 1.0)}}))}})
}}
filler := compute.parallel fill "cpu" (Data _)
"#
    )
}

/// The chain `e[i] = d[i] · 2` feeding `z[i] = Σ e[k]`, the elementwise one under a pack.
///
/// # Invariant
/// The consumer reads `e` **at an address it computes** — `s(0) - 1`, a loop-carried index that no
/// entry parameter is — so the shape is the one people write and the one no entry parameter
/// answers; `k.in.d` is declared beside the buffer it absorbs, which the union does not need.
fn folding() -> String {
    let head = program(COUNT);
    let head = head
        .split("In2  = struct")
        .next()
        .expect("the elementwise half");
    format!(
        r#"{head}In3 := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}})), .e (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutT := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par3 := _x => {{.n @any : Int, .in In3 _}}
total_raw := (k <: Par3) => {{
  i := compute.range k.n
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (((compute.read (compute.Read{{.from k.in.e, .at s(0) - 1}}) : Float) ~ "element") + 1.0)) ~ "combine")
  ((OutT){{.z (compute.write (compute.Write{{.to z, .at i, .value fold (k.n, 0.0 ~ "identity")}}))}})
}}
total := compute.parallel (total_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (OutT _)
GArg := _x => {{.n @any : Int, .in In _}}
step := (s <: GArg) => {{
  first := (compute.plrun scale (compute.A{{.n s.n, .I In{{.d s.in.d}}}}) : Struct)
  compute.plrun total (compute.A{{.n s.n, .I In3{{.d s.in.d, .e first.e}}}})
}}
built := compute.graph step
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

/// How much of the shifted chain's answer the program defines: its last element reads `e[COUNT]`,
/// which no lane wrote.
const SHIFTED_DEFINED: usize = COUNT - 1;

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
fn without_a_pack_the_floor_proves_the_elementwise_region() {
    let (module, _instances, schema, graph) = derived(&recording());
    println!(
        "the region's canonical form:\n{}the region's hash: {}",
        schema.canonical_form(),
        schema.hash
    );
    let before = (graph.len(), graph.value_count());
    assert_eq!(before, (2, 4), "the recording is two nodes and four values");

    let (rewritten, lines) = consulted(&module, &graph, &[]);
    let Some(rewritten) = rewritten else {
        panic!("the floor proves the region, so the pass answers a graph: {lines:?}");
    };
    assert_eq!(lines.len(), 1, "the one region is reported once");
    assert!(
        lines[0].starts_with("region 0: nodes 0 and 1 merged into one node at 1 (the survivor)"),
        "the floor proves the merge: {lines:?}"
    );
    assert!(
        lines[0].contains("licensed by the floor: every node of the region claims Elementwise"),
        "and the sentence names the proof that licensed it: {lines:?}"
    );
    assert_ne!(
        (rewritten.len(), rewritten.value_count()),
        before,
        "the graph the pass answers is the merged one"
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
        rewritten.is_some(),
        "the floor proves the region whatever the pack names: {lines:?}"
    );
    assert!(
        lines[0].contains("licensed by the floor"),
        "the licence is the floor's own proof, not the inert pack: {lines:?}"
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
    _defined: usize,
    _expected: fn(f32) -> Vec<f32>,
) {
    let (stub, _guard) = common::stub();
    clear_graph_pass();
    let before = stub.calls().len();
    let (base_module, _base_value, _root) = common::run(&running_recording(recording, elements));
    // The stub computes nothing, so what the legs prove is the **record**.
    let recorded = stub.calls()[before..].to_vec();
    assert!(
        !recorded.is_empty(),
        "with no pass installed the program dispatches its own kernels: {name}"
    );

    let (_module, _instances, schema, _graph) = derived(recording);
    let packs = [pack(&schema.hash.to_string(), PACK)];

    install_region_merge_with_packs(&base_module, &packs);
    let before = stub.calls().len();
    let (merged_module, _merged_value, _root) =
        common::run(&running_recording(recording, elements));
    let merged = stub.calls()[before..].to_vec();
    let report = pass_report(&merged_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains(&format!("licensed by pack \"{PACK}\""))),
        "the run's own report says the pack licensed the merge while the run was dispatched: \
         {report:?}"
    );
    // The merged leg's last dispatch is a node the recording did not: the composed body.
    let composed = merged.last().expect("the merged leg dispatches");
    for call in &recorded {
        assert_ne!(
            composed.fragment, call.fragment,
            "the composed node is not either kernel the recording dispatched: {name}"
        );
    }
    assert_eq!(
        composed.count, recorded[PRODUCER_CALL].count,
        "and it is dispatched over the recording's extent: {name}"
    );

    // **The counterfactual**: only the composed node can answer a value the recording cannot.
    set_graph_pass(Box::new(MovedBias(region_merge_pass_with_packs(
        &base_module,
        &packs,
    ))));
    let before = stub.calls().len();
    let (_moved_module, _moved_value, _root) = common::run(&running_recording(recording, elements));
    let moved = stub.calls()[before..].to_vec();
    clear_graph_pass();
    let moved_composed = moved.last().expect("the moved bias dispatches");
    // The staged bias is part of the composed body, so its fragment differs.
    assert_ne!(
        moved_composed.fragment, composed.fragment,
        "the bias moved the composed body, so the composed node is what the run dispatched: {name}"
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
    let nameless = read_placement_pack(
        &text
            .replace(
                &format!("\"region\": \"{}\"", schema.hash),
                "\"region\": null",
            )
            .replace("\"class\": \"float\"", "\"class\": null"),
    )
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
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In   := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutR := _x => {{.r (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par  := _x => {{.n @any : Int, .in In _}}
producer_raw := (k <: Par) => {{
  i := compute.range k.n
  r := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float)) ~ "combine"))
  ((OutR){{.r (compute.write (compute.Write{{.to r, .at i, .value fold (k.n, 0.0 ~ "identity")}}))}})
}}
producer := compute.parallel (producer_raw ~ "claim:Reduction(combine,identity)") "gpu" (OutR _)
In2  := _x => {{.d (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}})), .p (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
OutZ := _x => {{.z (compute.Buf (compute.BufParam{{.type Float, .length {COUNT}}}))}}
Par2 := _x => {{.n @any : Int, .in In2 _}}
consumer_raw := (k <: Par2) => {{
  i := compute.range k.n
  r := (compute.read (compute.Read{{.from k.in.p, .at 0}}) : Float)
  z := compute.buf (compute.BufParam{{.type Float, .length k.n}})
  @loop fold := s => if s(0) == 0 then s(1) else fold (s(0) - 1, (max s(1) ((((compute.read (compute.Read{{.from k.in.d, .at s(0) - 1}}) : Float)) - r) ~ "element")) ~ "combine")
  ((OutZ){{.z (compute.write (compute.Write{{.to z, .at i, .value fold (k.n, (0.0 - 1000000.0) ~ "identity")}}))}})
}}
consumer := compute.parallel (consumer_raw ~ "claim:Reduction(combine,identity,element)") "gpu" (OutZ _)
GArg := _x => {{.n @any : Int, .in In _}}
step := (s <: GArg) => {{
  first := (compute.plrun producer (compute.A{{.n s.n, .I In{{.d s.in.d}}}}) : Struct)
  compute.plrun consumer (compute.A{{.n s.n, .I In2{{.d s.in.d, .p first.r}}}})
}}
built := compute.graph step
(producer, consumer, built)
"#
    )
}

//! A pack's plan is matched to a node by schema and steers its placement.
//! See docs/notes/graph-fusion.md §4 and §5.

use super::common;

use super::common::LANES;
use lichen_compute::automatic_tiling::{install_tiling, install_tiling_with_packs};
use lichen_compute::compute::{clear_graph_pass, consult_graph_pass, take_graph_pass_lines};
use lichen_compute::fusion_claim::{RoleProjection, SchemaHash, SchemaNode, kernel_id};
use lichen_compute::kernel_fragment;
use lichen_compute::language::{LangProgram, LangValue};
use lichen_compute::placement_pack::{PackRefusal, read_placement_pack};
use lichen_fusion::{BinaryOp, Expr, Variable};
use lichen_graph_ir::{Count, Graph, KernelNode, Node};
use lichen_kernel_ir::{KernelFragment, ScalarClass, Slice, Space, fragment_digest};
use lichen_lowlevel::Module;

/// A launch the tile divides: three workgroups cover `[0, 192)`.
const COUNT: usize = 3 * LANES as usize;

/// The per-lane answer at `out[0]`: the body folds the lane's own one element there.
const PER_LANE: f32 = 1.0;

/// The name the packs here stage under, which is the name the tiled body publishes.
const STAGING: &str = "tile";

/// The answer a sum over `1 ..= COUNT` has.
///
/// # Invariant
/// `COUNT` and `COUNT + 1` are exact in `f32` and their product is below `2^53`, so the closed
/// form is one exact number however it is spelled.
fn closed_form() -> f32 {
    (COUNT * (COUNT + 1) / 2) as f32
}

/// A program whose one kernel declares a reduction over a **runtime** extent.
///
/// # Invariant
/// The fold's bound is `i + 1`, the lane's own index plus one, so the program's own body is the
/// per-lane schedule the pass replaces, and the seed is the marked literal both halves of the
/// algebra survive lowering through (`docs/notes/graph-fusion.md` §3).
fn declared() -> String {
    format!(
        r#"
---
  compute = import "compute.lichen"
---
In  = struct{{.b (compute.Buf Float)}}
Out = struct{{.z (compute.Buf Float)}}
Par = compute.P (compute.KT Float)(.I In, .O Out)
reduction = (k : Par) => {{
  i = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from k.in.b, .at s(0) - 1)) : Float)) ~ "combine" ~ "accumulator"))
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value fold (i + 1, 0.0 ~ "identity")))
}}
kernel = compute.parallel (reduction ~ "claim:Reduction(combine,identity)") "gpu" Out
step = (s : Par) => {{
  compute.plrun kernel ((compute.A In)(.n s.n, .I In(.b s.in.b))) : Out
}}
built = compute.graph step
data = (compute.buffer ([{elements}] : array{{Float, LENGTH}}) : (compute.Buf _))
out = compute.collect (compute.graphrun built ({count}, data))
out
"#,
        elements = data(),
        count = COUNT,
    )
    .replace("array{Float, LENGTH}", &format!("array{{Float, {COUNT}}}"))
}

/// The elements a launch folds: `1 ..= count`, which is what makes the closed form the answer.
fn data() -> String {
    (1..=COUNT)
        .map(|position| format!("{position}.0"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// The kernel id the program dispatched, as the registry knows it.
fn dispatched(module: &Module<LangProgram>) -> usize {
    module
        .nodes
        .iter()
        .find_map(|(id, _node)| kernel_id(module, id).ok())
        .expect("the program dispatched one parallel kernel")
}

/// The fragment the registry holds for the kernel a program dispatched.
///
/// # Invariant
/// A registry read and never a read of the graph's node: a landed node stops being the recorded
/// fragment, so this is the only place the recording can still be compared.
fn registered(module: &Module<LangProgram>) -> KernelFragment {
    kernel_fragment(dispatched(module)).expect("the registry holds the dispatched fragment")
}

/// The `f32` the program's root value holds at element zero.
fn answer(module: &Module<LangProgram>, value: &LangValue) -> f32 {
    common::float_array(module, value)[0]
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

/// The `sha256:<hex>` a node's line prints, which is what a pack author copies.
fn reported_schema(report: &[String]) -> String {
    let line = report
        .iter()
        .find(|line| line.contains("sha256:"))
        .unwrap_or_else(|| panic!("a node's line prints its schema: {report:?}"));
    let start = line.find("sha256:").expect("the line carries the prefix");
    line[start..start + "sha256:".len() + 64].to_string()
}

/// The pack a human writes for `schema`, as the JSON artifact's fields spell it.
fn pack_text(schema: &str, name: &str, class: &str, space: &str, width: u32) -> String {
    format!(
        r#"{{
  "schema": "{schema}",
  "name": "{name}",
  "requires": {{ "class": "{class}" }},
  "plan": {{ "staging": [ {{ "name": "{STAGING}", "space": "{space}", "group_width": {width} }} ] }}
}}"#
    )
}

/// `hash` with its last hex digit changed, so a pack's schema is not the node's.
fn elsewhere(hash: &str) -> String {
    let mut digits = hash.to_string();
    let last = digits.pop().expect("a hash is not empty");
    let flipped = if last == '0' { '1' } else { '0' };
    digits.push(flipped);
    digits
}

/// The fragment the installed pass landed for a graph over the recorded body, and its lines.
///
/// # Invariant
/// The pass is handed the **recording**'s own fragment, and the graph a run rewrote is stored
/// and never handed back (`crates/lichen-compute/src/compute.rs`), so this is the one way a test
/// reads the fragment a plan produced rather than the body it started from.
fn landed(recorded: &KernelFragment, extent: usize) -> (KernelFragment, Vec<String>) {
    let mut graph = Graph::with_inputs(1);
    let node = graph
        .push(
            Node::Kernel(KernelNode {
                fragment: recorded.clone(),
                inputs: vec![0],
                count: Count::Constant(extent),
                scalars: Vec::new(),
            }),
            1,
        )
        .expect("one output, one declared");
    graph
        .returning(vec![node])
        .expect("a value the graph defines");
    let count = |_node: usize| Some(extent);
    let rewritten = consult_graph_pass(&graph, &count).expect("the installed pass answers a graph");
    let Node::Kernel(kernel) = &rewritten.nodes()[0];
    (kernel.fragment.clone(), take_graph_pass_lines())
}

/// A matching pack fires, lands its plan, and the device answers the closed form.
///
/// # Invariant
/// The hash the pack spells is read off a report rather than computed here, so what is tested is
/// §4's rule: what a report prints is what a pack author recomputes the hash from, and the hash
/// is the whole of the match.
#[test]
fn a_matching_pack_fires_and_the_plan_it_states_is_what_lands() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = declared();
    let (module, _value, _root) = common::run(&source);

    // The floor: its line prints the node's schema, and its landed fragment publishes no mark.
    install_tiling(&module);
    let (floor_module, _floor_value, _root) = common::run(&source);
    let floor_report = pass_report(&floor_module);
    let schema = reported_schema(&floor_report);
    assert!(
        floor_report
            .iter()
            .any(|line| line.contains("no pack matched its schema")),
        "no pack is installed, and the report says so: {floor_report:?}"
    );
    let (floor_landed, _) = landed(&registered(&module), COUNT);
    assert!(
        floor_landed.marks.is_empty(),
        "the floor's own fragment publishes no mark"
    );

    // The pack a human would write for that schema.
    let text = pack_text(&schema, "sum-in-shared", "float", "shared", LANES);
    let pack = read_placement_pack(&text)
        .unwrap_or_else(|refusal| panic!("the pack a human wrote is read: {refusal}"));

    install_tiling_with_packs(&module, &[pack]);
    let (packed_landed, _) = landed(&registered(&module), COUNT);
    assert_eq!(
        packed_landed.slices,
        vec![Slice {
            class: ScalarClass::Float,
            count: LANES,
            space: Space::Shared,
        }],
        "the landed fragment's slice states the plan's space, and its count is the body's"
    );
    assert!(
        packed_landed
            .marks
            .iter()
            .any(|mark| mark.text() == Some(STAGING)),
        "the landed fragment publishes the plan's own staging name: {:?}",
        packed_landed.marks
    );
    assert_ne!(
        fragment_digest(&packed_landed),
        fragment_digest(&floor_landed),
        "the plan steered the emit, so the fragment it landed is not the floor's"
    );

    let (_packed_module, _packed_value, _root) = common::run(&source);
}

/// A pack whose schema is not the node's is inert, and the floor answers exactly as before.
#[test]
fn an_unmatched_pack_is_inert() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = declared();
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (floor_module, floor_value, _root) = common::run(&source);
    let floor = common::float_array(&floor_module, &floor_value);
    let schema = reported_schema(&pass_report(&floor_module));

    let other = elsewhere(&schema);
    assert_ne!(other, schema, "the pack's schema is not the node's");
    let pack = read_placement_pack(&pack_text(
        &other,
        "sum-in-shared",
        "float",
        "shared",
        LANES,
    ))
    .expect("the pack is well formed; it is its schema that differs");

    install_tiling_with_packs(&module, &[pack]);
    let (inert_module, inert_value, _root) = common::run(&source);
    assert_eq!(
        common::float_array(&inert_module, &inert_value),
        floor,
        "with no pack firing the automatic floor answers exactly as it did without one"
    );
    let report = pass_report(&inert_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains("no pack matched its schema") && !line.contains("fired")),
        "the report says no pack matched: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("fired")),
        "and no line reports a pack firing: {report:?}"
    );
    assert_eq!(
        reported_schema(&report),
        schema,
        "the report still prints the node's own schema, which the pack did not spell"
    );
}

/// A pack whose constraints the fragment does not satisfy is inert too.
#[test]
fn a_pack_whose_constraints_do_not_hold_is_inert() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = declared();
    let (module, _value, _root) = common::run(&source);

    install_tiling(&module);
    let (floor_module, floor_value, _root) = common::run(&source);
    let floor = common::float_array(&floor_module, &floor_value);
    let schema = reported_schema(&pass_report(&floor_module));

    // The hash is the node's and the constraint is not: the node's first input is a `Float`.
    let pack = read_placement_pack(&pack_text(&schema, "sum-in-shared", "int", "shared", LANES))
        .expect("the pack is well formed; it is its constraint that does not hold");

    install_tiling_with_packs(&module, &[pack]);
    let (inert_module, inert_value, _root) = common::run(&source);
    assert_eq!(
        common::float_array(&inert_module, &inert_value),
        floor,
        "a constraint that does not hold leaves the floor answering"
    );
    let report = pass_report(&inert_module);
    assert!(
        report
            .iter()
            .any(|line| line.contains("pack \"sum-in-shared\" was consulted")
                && line.contains("constraints did not hold")),
        "the report names the pack it consulted and why it did not fire: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("fired")),
        "and no line reports a pack firing: {report:?}"
    );
}

/// A pack whose group width the target does not have is refused, and nothing lands.
#[test]
fn a_pack_asking_for_a_width_the_target_lacks_is_refused() {
    let (_stub, _guard) = common::stub();
    clear_graph_pass();
    let source = declared();
    let (module, value, _root) = common::run(&source);
    let per_lane = common::float_array(&module, &value);
    let recorded = registered(&module);

    install_tiling(&module);
    let (floor_module, _floor_value, _root) = common::run(&source);
    let schema = reported_schema(&pass_report(&floor_module));

    // Half a workgroup is not a group this target has.
    let narrow = LANES / 2;
    assert_ne!(narrow, LANES, "the width asked for is not the target's");
    let pack = read_placement_pack(&pack_text(
        &schema,
        "sum-in-a-narrow-group",
        "float",
        "shared",
        narrow,
    ))
    .expect("the pack is well formed; it is its group width the fragment refutes");

    install_tiling_with_packs(&module, &[pack]);
    let (refused_module, refused_value, _root) = common::run(&source);
    assert_eq!(
        common::float_array(&refused_module, &refused_value),
        per_lane,
        "nothing landed, so the node ran the body the program wrote"
    );
    let report = pass_report(&refused_module);
    assert!(
        report.iter().any(
            |line| line.contains("pack \"sum-in-a-narrow-group\" was refused")
                && line.contains("StagingWidthNotAWidth")
                && line.contains(&narrow.to_string())
        ),
        "the refusal names the pack, the check and the width it asked for: {report:?}"
    );
    assert!(
        report.iter().all(|line| !line.contains("fired")),
        "and no line reports a pack firing: {report:?}"
    );
    assert_eq!(
        fragment_digest(&registered(&refused_module)),
        fragment_digest(&recorded),
        "a refused pack's node keeps the fragment the program recorded"
    );
    let (unlanded, _) = landed(&recorded, COUNT);
    assert_eq!(
        fragment_digest(&unlanded),
        fragment_digest(&recorded),
        "and the pass landing over the recording leaves the node's fragment as it was"
    );
}

/// A node's schema hash is recomputable from the text the node prints, and carries no position.
#[test]
fn a_node_schema_hashes_the_text_it_prints() {
    let node = SchemaNode {
        position: 3,
        kernel: 9,
        interface: "Reduction".to_string(),
        projections: vec![
            RoleProjection {
                role: "combine".to_string(),
                expression: combine(),
                canonical: combine(),
            },
            RoleProjection {
                role: "identity".to_string(),
                expression: Expr::Constant(0.0),
                canonical: Expr::Constant(0.0),
            },
        ],
    };
    assert_eq!(
        node.canonical_form(),
        "interface Reduction\nrole combine (+ d r)\nrole identity 0.0\n",
        "the node's form is its interface and its roles' canonical projections, in name order"
    );
    assert_eq!(
        SchemaHash::parse(&node.hash().to_string()),
        Some(node.hash()),
        "the hash a report prints is one a reader takes back, so a pack can spell it"
    );
    let moved = SchemaNode {
        position: 11,
        ..node.clone()
    };
    assert_eq!(
        moved.hash(),
        node.hash(),
        "the node's form carries no position, so one pack serves a node wherever it sits"
    );
}

/// A pack that is not one is refused by name: a bad hash, an unknown field, a missing field.
#[test]
fn a_pack_that_is_not_one_is_refused_by_name() {
    let refusal = read_placement_pack(&pack_text("sha256:9f2c", "x", "float", "shared", 64))
        .expect_err("a truncated hash is not one `sha256:<hex>` spells");
    assert!(
        matches!(refusal, PackRefusal::BadSchemaHash { .. }),
        "the refusal is about the hash: {refusal}"
    );
    assert!(
        refusal.message().contains("sha256:9f2c"),
        "and it quotes the spelling it met: {refusal}"
    );

    let schema = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    let unknown = pack_text(schema, "x", "float", "shared", 64)
        .replace("\"name\": \"x\"", "\"name\": \"x\", \"element\": \"float\"");
    let refusal = read_placement_pack(&unknown).expect_err("`element` is not a field this has");
    let message = refusal.message();
    assert!(
        message.contains("element"),
        "the refusal quotes the field it met: {message}"
    );
    assert!(
        message.contains("schema") || message.contains("pack"),
        "and says where it met it: {message}"
    );

    let missing = pack_text(schema, "x", "float", "shared", 64).replace("\"name\": \"x\",", "");
    let refusal = read_placement_pack(&missing).expect_err("a pack without a name names none");
    assert!(
        refusal.message().contains("name"),
        "the refusal names the field that is missing: {refusal}"
    );

    let empty = pack_text(schema, "x", "float", "shared", 64)
        .replace(&format!("\"staging\": [ {{ \"name\": \"{STAGING}\", \"space\": \"shared\", \"group_width\": 64 }} ]"), "\"staging\": []");
    let refusal =
        read_placement_pack(&empty).expect_err("a plan that stages nothing steers nothing");
    assert!(
        matches!(refusal, PackRefusal::EmptyPlan { .. }),
        "the refusal is about the plan: {refusal}"
    );
}

/// The combine projection both forms of the node carry.
fn combine() -> Expr {
    BinaryOp::Add.apply(
        Expr::Variable(Variable::DATUM),
        Expr::Variable(Variable::PRODUCER),
    )
}

/// The class a pack states is the class the landed fragment reads.
///
/// # Invariant
/// The landed fragment is what the dispatcher compiles, so this is the one reading that separates "the
/// pack's class was lost" from "the class arrived and the emitter ignored it": the former fails here, and
/// the latter passes here and fails in the numbers.
#[test]
fn the_landed_fragment_reads_the_class_the_pack_states() {
    clear_graph_pass();
    let source = declared();
    let (module, _value, _root) = common::run(&source);
    let recorded = registered(&module);
    install_tiling(&module);
    let (floor_module, _value, _root) = common::run(&source);
    let schema = reported_schema(&pass_report(&floor_module));
    let pack = read_placement_pack(&pack_text(&schema, "narrow", "bfloat16", "shared", LANES))
        .expect("the pack reads");
    install_tiling_with_packs(&module, &[pack]);
    let (landed_fragment, _lines) = landed(&recorded, COUNT);
    assert_eq!(
        recorded.input_classes,
        vec![ScalarClass::Float],
        "the recorded fragment reads what the program declared"
    );
    assert_eq!(
        landed_fragment.input_classes,
        vec![ScalarClass::BFloat16],
        "the landed fragment reads the class the pack states"
    );
    assert_eq!(
        landed_fragment.output_classes,
        vec![ScalarClass::BFloat16],
        "the landed fragment writes the class the pack states"
    );
    let accesses: Vec<ScalarClass> = landed_fragment
        .body
        .values
        .iter()
        .filter_map(|value| match value {
            lichen_kernel_ir::ValueDef::Instr { op, .. } => match op {
                lichen_kernel_ir::KernelInstr::BufferReadCall(class)
                | lichen_kernel_ir::KernelInstr::BufferWriteCall(class) => Some(*class),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        accesses,
        vec![ScalarClass::BFloat16, ScalarClass::BFloat16],
        "every buffer access the landed body makes is at the class the pack states"
    );
}

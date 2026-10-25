//! The class pack and the sibling merge: the merged node's ports, and the answers it gives
//! beside the unmerged step's.
//!
//! # Invariant
//! **The two preparations differ in one thing only**: the same program, the same class pack and
//! the same host inputs, and the merge is what the second one has.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_compute::fusion_file::{FusionPlan, read_fusion_file};
use lichen_compute::placement_pack::read_placement_pack;
use lichen_graph_ir::Node;
use lichen_kernel_ir::ScalarClass;
use lichen_serve::{PRE_STEP, Session};

/// The class pack the composed step is prepared with: bf16 for the three weight slots.
const CLASS_PACK: &str = r#"{"name":"narrow","requires":{"class":null,"nodes":[2,6,7],"classes":[{"at":1,"class":"bfloat16"}]},"plan":{"staging":[]}}"#;

/// The sibling region **as the classes left it**: a class pack moves the region's hash.
const CLASSED_REGION: &str =
    "sha256:ff1d97d1c84d07e6bebdad468606d904336105fcafbe62fda85f5b7ae5ebf9d8";

/// The fusion file beside this example: the plan that names the sibling region.
const MANIFEST: &str = "fusion_file.json";

/// The plan the fusion file answers.
fn plan() -> FusionPlan {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(MANIFEST);
    let text = std::fs::read_to_string(&path).expect("the fusion file is beside the probe");
    read_fusion_file(&text)
        .expect("the fusion file reads")
        .plan()
        .expect("the file answers a plan")
}

/// The numbers the buffers hold.
fn numbers() -> (
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
    Vec<f32>,
) {
    let hidden: Vec<f32> = (0..2048).map(|at| (at % 11) as f32 * 0.001).collect();
    let gain: Vec<f32> = (0..2048).map(|at| (at % 7) as f32 * 0.001).collect();
    let k: Vec<f32> = (0..1024 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    let knorm: Vec<f32> = (0..128).map(|at| (at % 3) as f32 * 0.001).collect();
    let q: Vec<f32> = (0..2048 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    let qnorm: Vec<f32> = (0..128).map(|at| (at % 3) as f32 * 0.001).collect();
    let v: Vec<f32> = (0..1024 * 2048).map(|at| (at % 5) as f32 * 0.001).collect();
    (hidden, gain, k, knorm, q, qnorm, v)
}

/// The host's numbers, with the weight buffers as brain floats.
fn inputs() -> Vec<RunArgument> {
    let (hidden, gain, k, knorm, q, qnorm, v) = numbers();
    vec![
        RunArgument::Count(0),
        floats(&hidden),
        floats(&gain),
        halves(&k),
        floats(&knorm),
        halves(&q),
        floats(&qnorm),
        halves(&v),
    ]
}

/// The same numbers, with every buffer a full-precision one.
fn wide_inputs() -> Vec<RunArgument> {
    let (hidden, gain, k, knorm, q, qnorm, v) = numbers();
    vec![
        RunArgument::Count(0),
        floats(&hidden),
        floats(&gain),
        floats(&k),
        floats(&knorm),
        floats(&q),
        floats(&qnorm),
        floats(&v),
    ]
}

/// A buffer of full-precision elements.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_bits().to_le_bytes())
            .collect(),
    }
}

/// A buffer of brain-float elements: the top sixteen bits of the float it rounds to.
fn halves(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::BFloat16,
        data: values
            .iter()
            .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
            .collect(),
    }
}

/// The merged node's own ports: the classes it reads its inputs at.
fn merged_ports(prepared: &lichen_serve::Prepared<'_>) -> Option<(usize, Vec<ScalarClass>)> {
    prepared
        .graph()
        .nodes()
        .iter()
        .enumerate()
        .find_map(|(position, node)| {
            let Node::Kernel(kernel) = node;
            (kernel.fragment.outputs == 2)
                .then(|| (position, kernel.fragment.input_classes.clone()))
        })
}

/// The answers one run gave, as the floats it fetched.
fn answers(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<(usize, Vec<f32>)> {
    stepped
        .results
        .iter()
        .map(|result| {
            let RunResult::Resident(resident) = result else {
                panic!("no resident");
            };
            let answer = session.fetch(*resident).expect("the answer reads");
            session.release(*resident);
            match answer {
                lichen_kernel_ir::ScalarData::Float(values) => (values.len(), values),
                other => panic!("unexpected {other:?}"),
            }
        })
        .collect()
}

fn main() {
    let session = Session::open().expect("a device");
    let class_pack = read_placement_pack(CLASS_PACK).expect("the class pack");
    let plan = plan();

    // The sibling merge alone: the fusion file's pack names the region.
    lichen_compute::compute::clear_graph_pass();
    let licensed = session
        .prepare_with_plan(PRE_STEP, &[], &plan)
        .expect("the plan prepares");
    println!("=== the fusion file's plan: {} node(s)", licensed.nodes());
    for line in licensed.report() {
        println!("{line}");
    }
    match merged_ports(&licensed) {
        Some((position, classes)) => println!("the merged node at {position} reads {classes:?}"),
        None => println!("no merged node"),
    }

    // The same plan with the class pack beside it: the classes land before the merge.
    lichen_compute::compute::clear_graph_pass();
    let classed = session
        .prepare_with_plan(PRE_STEP, &[class_pack.clone()], &plan)
        .expect("the class pack and the plan prepare");
    println!(
        "=== the class pack and the fusion file's plan: {} node(s)",
        classed.nodes()
    );
    for line in classed.report() {
        println!("{line}");
    }
    match merged_ports(&classed) {
        Some((position, classes)) => println!("the merged node at {position} reads {classes:?}"),
        None => println!("no merged node"),
    }

    // The unmerged step: the class pack alone.
    lichen_compute::compute::clear_graph_pass();
    let plain = session
        .prepare_with_packs(PRE_STEP, &[class_pack.clone()])
        .expect("the class pack alone prepares");
    println!("=== the class pack alone: {} node(s)", plain.nodes());
    for line in plain.report() {
        println!("{line}");
    }

    // The fuse pack that names the sibling region as the classes left it.
    let fuse_text = format!(
        r#"{{"schema":null,"name":"fuse-pre-step","requires":{{"class":"float"}},"fuse":{{"region":"{CLASSED_REGION}","as":"one_kernel"}},"plan":{{"staging":[]}}}}"#
    );
    let fuse_pack = read_placement_pack(&fuse_text).expect("the fuse pack");
    lichen_compute::compute::clear_graph_pass();
    let named = session
        .prepare_with_plan(PRE_STEP, &[class_pack.clone(), fuse_pack], &plan)
        .expect("the fuse pack prepares");
    println!(
        "=== the class pack and the fuse pack on the classed region: {} node(s)",
        named.nodes()
    );
    for line in named.report() {
        println!("{line}");
    }
    match merged_ports(&named) {
        Some((position, classes)) => println!("the merged node at {position} reads {classes:?}"),
        None => println!("no merged node"),
    }

    // The differential: the same host inputs, with the merge and without it.
    let stepped = classed.step(inputs()).expect("the merged step runs");
    let merged_answers = answers(&session, &stepped);
    let stepped = plain.step(inputs()).expect("the plain step runs");
    let plain_answers = answers(&session, &stepped);
    println!("=== the answers");
    for (index, (merged_answer, plain_answer)) in
        merged_answers.iter().zip(plain_answers.iter()).enumerate()
    {
        println!(
            "answer {index}: merged {} element(s), plain {} element(s), equal: {}",
            merged_answer.0,
            plain_answer.0,
            merged_answer.1 == plain_answer.1,
        );
    }

    // The counterfactual: the same step with no class pack reads the weights as they are.
    lichen_compute::compute::clear_graph_pass();
    let wide = session
        .prepare_with_packs(PRE_STEP, &[])
        .expect("the step prepares with no pack");
    let stepped = wide.step(wide_inputs()).expect("the wide step runs");
    let wide_answers = answers(&session, &stepped);
    println!("=== the counterfactual: no class pack, the weights as floats");
    for (index, (merged_answer, wide_answer)) in
        merged_answers.iter().zip(wide_answers.iter()).enumerate()
    {
        println!(
            "answer {index}: merged {} element(s), wide {} element(s), equal: {}",
            merged_answer.0,
            wide_answer.0,
            merged_answer.1 == wide_answer.1,
        );
    }
}

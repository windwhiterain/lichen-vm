//! The staged fan-in over one lane group: the merged node's answer, beside the unmerged one's.
//!
//! # Invariant
//! **The two preparations differ in one thing only**: the same program, the same inputs and the
//! same device — the first has the merge; the second asks for a one-node budget, so every
//! region is refused and the program runs unmerged.
//!
//! **Two producers and one consumer over one 64-lane group**, and the consumer reads at `i / 2`,
//! so it reads what another lane of the group wrote.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_compute::fusion_file::{Budget, FusionPlan, Strategy};
use lichen_kernel_ir::ScalarClass;
use lichen_serve::Session;

/// How many lanes the fan-in dispatches over: one lane group, so a barrier reaches every read.
const LANES: usize = 64;

/// The program: two reductions and one consumer that reads both.
///
/// # Invariant
/// **The two producers are independent and the consumer reads both**: a node that reads several
/// producers is a fan-in, and the region is two producers and one consumer.
const FAN_IN: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Par = struct{.n Int, .dim Int, .in In}
Arg = struct{.n Int, .dim Int, .I In}
square_raw = (k : Par) => {
  lane = compute.range k.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((lane * k.dim + (s(0) - 1))))) : Float))) ~ "combine" ~ "element"))
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to b_y, .at lane, .value (fold (k.dim, 0.0 ~ "identity")))))))
}
square = compute.parallel (square_raw ~ "claim:Reduction(combine,identity,element)") "gpu" Out
Both = struct{.a (compute.Buf ((compute.BufParam _)(.type Float, .length _))), .b (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
BothPar = struct{.n Int, .in Both}
BothArg = struct{.n Int, .I Both}
sum_raw = (k : BothPar) => {
  i = compute.range k.n
  head = i / 2
  a = (compute.read ((compute.Read _)(.from k.in.a, .at head)) : Float)
  b = (compute.read ((compute.Read _)(.from k.in.b, .at head)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value ((a + b) ~ "element"))))))
}
sum = compute.parallel (sum_raw ~ "claim:Elementwise(element)") "gpu" Out
GArg = struct{.in Both}
step = (s : GArg) => {
  a = (compute.plrun square ((Arg)(.n 64, .dim 1, .I In(.x s.in.a))) : Out)
  b = (compute.plrun square ((Arg)(.n 64, .dim 1, .I In(.x s.in.b))) : Out)
  c = (compute.plrun sum ((BothArg)(.n 64, .I Both(.a a.y, .b b.y))) : Out)
  c
}
built = compute.graph step
built
"#;

/// The plan that lets every region merge: the floor's own licence and no bound.
fn merge_plan() -> FusionPlan {
    FusionPlan {
        strategy: Strategy::Sibling,
        budget: Budget::unlimited(),
        overrides: Vec::new(),
        barriers: Vec::new(),
    }
}

/// The plan that bars every region: the budget of one node refuses them all.
fn one_node_plan() -> FusionPlan {
    FusionPlan {
        strategy: Strategy::Sibling,
        budget: Budget {
            max_region_nodes: 1,
            max_region_depth: 1,
            ..Budget::unlimited()
        },
        overrides: Vec::new(),
        barriers: Vec::new(),
    }
}

/// The host's numbers, as the two producers' inputs.
fn numbers() -> (Vec<f32>, Vec<f32>) {
    let left: Vec<f32> = (0..LANES).map(|at| (at % 11) as f32 * 0.25).collect();
    let right: Vec<f32> = (0..LANES).map(|at| (at % 7) as f32 * 0.5).collect();
    (left, right)
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

/// The host's numbers, as the program's inputs.
fn inputs() -> Vec<RunArgument> {
    let (left, right) = numbers();
    vec![floats(&left), floats(&right)]
}

/// The answers one run gave, as the floats it fetched.
fn answers(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<Vec<f32>> {
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
                lichen_kernel_ir::ScalarData::Float(values) => values,
                other => panic!("unexpected {other:?}"),
            }
        })
        .collect()
}

/// The answer the host computes: each producer's square, and the sum of the pair a lane reads.
///
/// # Invariant
/// **The consumer reads at `i / 2`**, so it reads what another lane of the same group wrote —
/// which is the read the barrier orders.
fn expected() -> Vec<f32> {
    let (left, right) = numbers();
    (0..LANES)
        .map(|at| {
            let head = at / 2;
            left[head] * left[head] + right[head] * right[head]
        })
        .collect()
}

fn main() {
    let session = Session::open().expect("a device");

    // The merge: the floor's own licence, the fan-in merged into one node.
    lichen_compute::compute::clear_graph_pass();
    let merged = session
        .prepare_with_plan(FAN_IN, &[], &merge_plan())
        .expect("the fan-in prepares");
    println!("=== the fan-in: {} node(s)", merged.nodes());
    for line in merged.report() {
        println!("{line}");
    }

    // The same program with every region refused: the unmerged reference.
    lichen_compute::compute::clear_graph_pass();
    let plain = session
        .prepare_with_plan(FAN_IN, &[], &one_node_plan())
        .expect("the barred plan prepares");
    println!(
        "=== the fan-in, every region refused: {} node(s)",
        plain.nodes()
    );
    for line in plain.report() {
        println!("{line}");
    }

    // The differential: the same host inputs, with the merge and without it.
    let stepped = merged.step(inputs()).expect("the merged step runs");
    let merged_answers = answers(&session, &stepped);
    let stepped = plain.step(inputs()).expect("the plain step runs");
    let plain_answers = answers(&session, &stepped);
    println!("=== the answers");
    for (index, (merged_answer, plain_answer)) in
        merged_answers.iter().zip(plain_answers.iter()).enumerate()
    {
        println!(
            "answer {index}: merged {} element(s), plain {} element(s), equal: {}",
            merged_answer.len(),
            plain_answer.len(),
            merged_answer == plain_answer,
        );
    }
    println!(
        "the host's own answer: equal: {}",
        merged_answers
            .first()
            .is_some_and(|answer| *answer == expected())
    );
}

//! A recorded step can hand back several values, and the order is the tuple's own.

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// Three doublings chained, handed back as the tuple `(third, second, first)`.
///
/// # Invariant
/// The tuple is written in reverse, so a run that answered in the graph's own order and one that
/// answered in the tuple's order are told apart by the numbers and not by their count.
const STEP: &str = r#"
--- compute = import "compute.lichen" ---
Inc = struct{.d (compute.Buf Float)}
Out = struct{.e (compute.Buf Float)}
Par = struct{.n Int, .in Inc, .out Out}
double_raw = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.e, .at i, .value ((compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float) * 2.0)))
}
double = compute.parallel double_raw "gpu" Out
GArg = struct{.n Int, .in Inc}
step = (s : GArg) => {
  a = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d s.in.d))) : Out)
  b = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d a.e))) : Out)
  c = (compute.plrun double ((compute.A Inc)(.n s.n, .I Inc(.d b.e))) : Out)
  (c.e, b.e, a.e)
}
built = compute.graph step
built
"#;

/// The resident a return is, or why this one is not one.
fn resident(result: &RunResult) -> ResidentBuffer {
    match result {
        RunResult::Resident(resident) => *resident,
        _ => panic!("a device wrote every value this step returns, so each stays a resident id"),
    }
}

/// The floats a resident holds.
fn read(session: &Session, resident: ResidentBuffer) -> Vec<f32> {
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the value is readable: {refusal}"))
    {
        ScalarData::Float(values) => values,
        other => panic!("the step declared Float buffers, and this is {other:?}"),
    }
}

/// A step answers with one value per element of the tuple it returned, in the tuple's order.
///
/// # Invariant
/// `1, 2, 3` doubled three times is `8, 16, 24`, twice is `4, 8, 12` and once is `2, 4, 6`, so each
/// returned value is recognisable on its own and a wrong order cannot pass.
#[test]
fn a_step_can_hand_back_three_values() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_step_can_hand_back_three_values: no device");
        return;
    };
    let prepared = session
        .prepare(STEP)
        .unwrap_or_else(|refusal| panic!("the step prepares: {refusal}"));
    assert_eq!(prepared.nodes(), 3, "the held step is three dispatches");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(3),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: [1.0f32, 2.0, 3.0]
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            },
        ])
        .unwrap_or_else(|refusal| panic!("the step runs: {refusal}"));
    assert_eq!(
        (stepped.cost.submissions, stepped.cost.fence_waits),
        (1, 1),
        "three dispatches are one submission"
    );
    assert_eq!(
        stepped.results.len(),
        3,
        "and one value comes back per element of the tuple"
    );
    let values: Vec<Vec<f32>> = stepped
        .results
        .iter()
        .map(|result| read(&session, resident(result)))
        .collect();
    assert_eq!(
        values,
        vec![
            vec![8.0, 16.0, 24.0],
            vec![4.0, 8.0, 12.0],
            vec![2.0, 4.0, 6.0],
        ],
        "the returns are the tuple's order: the third doubling first and the first one last"
    );
    for result in &stepped.results {
        session.release(resident(result));
    }
    println!("a step handed back three values in the tuple's order, at one submission");
}

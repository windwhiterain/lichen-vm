//! N copies in one step against N steps of one: what a dispatch inside a graph costs.

use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

fn program(nodes: usize, chained: bool) -> String {
    let mut calls = String::new();
    for at in 0..nodes {
        let input = if chained && at > 0 {
            format!("a{}.y", at - 1)
        } else {
            "s.in.x".to_string()
        };
        calls.push_str(&format!(
            "  a{at} = (compute.plrun copy ((Arg)(.n 256, .I In(.x {input}))) : Out)\n"
        ));
    }
    format!(
        r#"--- compute = import "compute.lichen" ---
In  = struct{{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}}
Out = struct{{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}}
Par = struct{{.n Int, .in In}}
Arg = struct{{.n Int, .I In}}
copy_raw = (k : Par) => {{
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value value)))))
}}
copy = compute.parallel copy_raw "gpu" Out
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
{calls}  a{nodes_minus} 
}}
built = compute.graph step
built
"#,
        calls = calls,
        nodes_minus = nodes - 1
    )
}

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    let values: Vec<f32> = (0..256).map(|at| (at % 7) as f32 * 0.001).collect();
    let args = || {
        vec![
            RunArgument::Count(256),
            RunArgument::Buffer {
                class: lichen_kernel_ir::ScalarClass::Float,
                data: values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            },
        ]
    };
    let rounds = 200;
    for nodes in [1usize, 4, 8] {
        let source = program(nodes, false);
        let prepared = session.prepare(&source).expect("the program prepares");
        let mut total = 0.0;
        for _ in 0..rounds {
            let started = std::time::Instant::now();
            let stepped = prepared.step(args()).expect("the step runs");
            total += started.elapsed().as_secs_f64();
            for result in &stepped.results {
                if let RunResult::Resident(resident) = result {
                    session.release(*resident);
                }
            }
        }
        println!(
            "one step, {nodes} independent dispatch(es): {:.3} ms a step ({:.3} ms a dispatch)",
            total * 1000.0 / rounds as f64,
            total * 1000.0 / (rounds * nodes) as f64
        );
    }
    for nodes in [2usize, 8] {
        let source = program(nodes, true);
        let prepared = session
            .prepare(&source)
            .expect("the chained program prepares");
        let mut total = 0.0;
        for _ in 0..rounds {
            let started = std::time::Instant::now();
            let stepped = prepared.step(args()).expect("the step runs");
            total += started.elapsed().as_secs_f64();
            for result in &stepped.results {
                if let RunResult::Resident(resident) = result {
                    session.release(*resident);
                }
            }
        }
        println!(
            "one step, {nodes} chained dispatch(es): {:.3} ms a step",
            total * 1000.0 / rounds as f64
        );
    }
    let single = session
        .prepare(&program(1, false))
        .expect("the one-node program prepares");
    let mut total = 0.0;
    for _ in 0..rounds * 8 {
        let started = std::time::Instant::now();
        let stepped = single.step(args()).expect("the step runs");
        total += started.elapsed().as_secs_f64();
        for result in &stepped.results {
            if let RunResult::Resident(resident) = result {
                session.release(*resident);
            }
        }
    }
    println!(
        "eight steps of one: {:.3} ms a step",
        total * 1000.0 / (rounds * 8) as f64
    );
}

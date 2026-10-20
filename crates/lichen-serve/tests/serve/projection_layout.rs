//! One projection, element by element, in both layouts the model could store its weights in.

use lichen_compute::compute::graph::RunArgument;
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::{MATVEC_BATCH_STEP, Session};

/// The projection's shape: four outputs over an input of three.
const ROWS: usize = 4;
const WIDTH: usize = 3;

/// `w` as the checkpoint stores it, one row an output.
fn by_row() -> Vec<f32> {
    vec![
        1.0, 2.0, 3.0, // output 0
        4.0, 5.0, 6.0, // output 1
        -1.0, 0.5, 0.0, // output 2
        2.0, -2.0, 1.0, // output 3
    ]
}

/// The same matrix read by column.
fn by_column(weights: &[f32]) -> Vec<f32> {
    let mut turned = vec![0.0_f32; weights.len()];
    for row in 0..ROWS {
        for column in 0..WIDTH {
            turned[column * ROWS + row] = weights[row * WIDTH + column];
        }
    }
    turned
}

/// The projection the kernel answers, which the host can work out.
fn expected(x: &[f32]) -> Vec<f32> {
    let weights = by_row();
    (0..ROWS)
        .map(|row| {
            (0..WIDTH)
                .map(|column| x[column] * weights[row * WIDTH + column])
                .sum()
        })
        .collect()
}

/// The projection with its weight stored by column, which is the layout the model would want.
///
/// # Invariant
/// The only difference from the shipped program is the weight's address: `(i * per) + weight_row` instead of
/// `(weight_row * dim) + i`. A disagreement here is the address and not the device.
const BY_COLUMN: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
Rows   = struct{.y (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
MatvecPar = struct{.n Int, .dim Int, .per Int, .in In, .out Rows}
MatvecArg = struct{.n Int, .dim Int, .per Int, .I In}
matvec_raw = (k : MatvecPar) => {
  row = compute.range k.n
  sequence = row / k.per
  weight_row = row - (sequence * k.per)
@loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + ((compute.read ((compute.Read _)(.from k.in.w, .at (((s(0) - 1) * k.per) + weight_row))) : Float) * (compute.read ((compute.Read _)(.from k.in.x, .at ((sequence * k.dim) + (s(0) - 1)))) : Float))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to k.out.y, .at row, .value (fold (k.dim, 0.0 ~ "identity"))))
}
matvec = compute.parallel (matvec_raw ~ "claim:Reduction(combine,identity)") "gpu" Rows
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.n Int, .dim Int, .per Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun matvec ((MatvecArg)(.n s.n, .dim s.dim, .per s.per, .I In(.x s.in.x, .w s.in.w))) : Rows)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#;

/// The kernel reads a weight stored by row as the checkpoint stores it.
///
/// # Invariant
/// This is the layout the model ran before the weights were considered by column, so it is the control: a
/// disagreement here is the harness and not the change.
#[test]
fn a_projection_reads_a_weight_stored_by_row() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED: no device");
        return;
    };
    let x = [1.0_f32, -2.0, 0.5];
    let prepared = session
        .prepare(MATVEC_BATCH_STEP)
        .expect("the projection prepares");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(ROWS as i64),
            RunArgument::Count(WIDTH as i64),
            RunArgument::Count(ROWS as i64),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: x.iter().flat_map(|value| value.to_le_bytes()).collect(),
            },
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: by_row()
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            },
        ])
        .expect("the projection runs");
    let [lichen_compute::compute::graph::RunResult::Resident(resident)] = &stepped.results[..]
    else {
        panic!("a projection answers with one resident");
    };
    let answered = session.fetch(*resident).expect("it answers");
    let ScalarData::Float(seen) = answered else {
        panic!("a float projection answers with floats");
    };
    println!("by row: {seen:?} wanted {:?}", expected(&x));
    for (index, value) in expected(&x).iter().enumerate() {
        assert!(
            (seen[index] - value).abs() < 1e-5,
            "output {index} is {seen:?}"
        );
    }
}

/// The projection answers the same numbers with its weight stored by column.
#[test]
fn a_projection_reads_a_weight_stored_by_column() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED: no device");
        return;
    };
    let x = [1.0_f32, -2.0, 0.5];
    let prepared = session.prepare(BY_COLUMN).expect("the projection prepares");
    let stepped = prepared
        .step(vec![
            RunArgument::Count(ROWS as i64),
            RunArgument::Count(WIDTH as i64),
            RunArgument::Count(ROWS as i64),
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: x.iter().flat_map(|value| value.to_le_bytes()).collect(),
            },
            RunArgument::Buffer {
                class: ScalarClass::Float,
                data: by_column(&by_row())
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect(),
            },
        ])
        .expect("the projection runs");
    let [lichen_compute::compute::graph::RunResult::Resident(resident)] = &stepped.results[..]
    else {
        panic!("a projection answers with one resident");
    };
    let answered = session.fetch(*resident).expect("it answers");
    let ScalarData::Float(seen) = answered else {
        panic!("a float projection answers with floats");
    };
    println!("by column: {seen:?} wanted {:?}", expected(&x));
    for (index, value) in expected(&x).iter().enumerate() {
        assert!(
            (seen[index] - value).abs() < 1e-5,
            "output {index} is {seen:?}"
        );
    }
}

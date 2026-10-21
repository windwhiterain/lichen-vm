//! The order the layer's own struct arrives in, read off rather than assumed.

use std::process::ExitCode;

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::ScalarClass;
use lichen_serve::Session;

/// The layer's twelve field names, in the order its own struct declares them.
const FIELDS: [&str; 12] = ["x", "i", "q", "k", "v", "qn", "kn", "o", "p", "g", "u", "d"];

/// A program that sums the twelve fields' first elements, each weighted by `2^(k-1)`.
///
/// # Invariant
/// Passing twelve buffers whose first elements are `1..12` in a known order makes the one answer
/// `sum of (argument that arrived at field k) · 2^(k-1)`: every argument number is below sixteen, so
/// the sum decodes one field at a time and one write states the whole permutation.
fn program() -> String {
    let mut source = String::from(
        r#"
--- compute = import "compute.lichen" ---
In     = struct{.x (compute.Buf Float), .i (compute.Buf Float), .q (compute.Buf Float), .k (compute.Buf Float), .v (compute.Buf Float), .qn (compute.Buf Float), .kn (compute.Buf Float), .o (compute.Buf Float), .p (compute.Buf Float), .g (compute.Buf Float), .u (compute.Buf Float), .d (compute.Buf Float)}
Out    = struct{.y (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
ReadPar = struct{.n Int, .in In, .out Out}
ReadArg = struct{.n Int, .I In}
read_raw = (k : ReadPar) => {
"#,
    );
    let mut total = String::from("0.0");
    for (index, field) in FIELDS.iter().enumerate() {
        let weight = 1u32 << index;
        source.push_str(&format!(
            "  first_{field} = (compute.read ((compute.Read _)(.from k.in.{field}, .at 0)) : Float)\n"
        ));
        total = format!("(({total}) + (first_{field} * {weight}.0))");
    }
    source.push_str(&format!(
        "  compute.write ((compute.Write _)(.to k.out.y, .at 0, .value {total}))\n"
    ));
    source.push_str(
        r#"}
read = compute.parallel read_raw "gpu" Out
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.position Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun read ((ReadArg)(.n 1, .I In(.x s.in.x, .i s.in.i, .q s.in.q, .k s.in.k, .v s.in.v, .qn s.in.qn, .kn s.in.kn, .o s.in.o, .p s.in.p, .g s.in.g, .u s.in.u, .d s.in.d))) : Out)
  compute.plrun copy ((CopArg)(.n 1, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#,
    );
    source
}

/// A `Float` buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> Result<ResidentBuffer, String> {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => Ok(*resident),
        _ => Err("the probe answers with one buffer, and this one answered otherwise".into()),
    }
}

/// Which argument reached which field, decoded from one number.
fn measure(session: &Session) -> Result<(), String> {
    let prepared = session
        .prepare(&program())
        .map_err(|refusal| format!("the probe did not prepare: {refusal}"))?;
    println!("the held step is {} dispatches", prepared.nodes());
    // Distinct lengths, so a collapsed-shape list would answer with fewer slots.
    let mut arguments: Vec<RunArgument> = (1..=12)
        .map(|value| {
            let mut buffer = vec![0.0_f32; value];
            buffer[0] = value as f32;
            floats(&buffer)
        })
        .collect();
    arguments.push(RunArgument::Count(0));
    let stepped = prepared
        .step(arguments)
        .map_err(|refusal| format!("the probe did not run: {refusal}"))?;
    let answered = match session.fetch(only(&stepped)?) {
        Ok(lichen_kernel_ir::ScalarData::Float(values)) => values,
        Ok(other) => return Err(format!("the answer declared a Float buffer: {other:?}")),
        Err(refusal) => return Err(format!("the answer is not readable: {refusal}")),
    };
    session.release(only(&stepped)?);
    let total = answered[0] as u64;
    println!("the answer is {total}");
    let mut arrived = [0u64; 12];
    for index in 0..12 {
        arrived[index] = (total >> (4 * index)) & 0xf;
    }
    for (index, field) in FIELDS.iter().enumerate() {
        println!(".{field} <- argument {}", arrived[index]);
    }
    Ok(())
}

fn main() -> ExitCode {
    match Session::open() {
        Ok(session) => match measure(&session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Err(refusal) => {
            eprintln!("this reading needs a device: {refusal}");
            ExitCode::FAILURE
        }
    }
}

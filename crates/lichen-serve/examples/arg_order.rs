//! What order the step's arguments arrive in, measured rather than assumed.

use std::process::ExitCode;

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// Three buffers read at one index, weighted by their position in a decimal number.
///
/// # Invariant
/// The step answers `a + 10 b + 100 c`, so the answer reads off which buffer reached which field: an
/// answer of `123` says the arguments arrived as `a`, `b`, `c`, and any other value says the order the
/// interface uses is not the order the struct declares.
const MIX_STEP: &str = r#"
--- compute = import "compute.lichen" ---
In     = struct{.a (compute.Buf Float), .b (compute.Buf Float), .c (compute.Buf Float)}
Out    = struct{.y (compute.Buf Float)}
CopIn  = struct{.d (compute.Buf Float)}
MixPar = struct{.n Int, .in In, .out Out}
MixArg = struct{.n Int, .I In}
mix_raw = (k : MixPar) => {
  first = (compute.read ((compute.Read _)(.from k.in.a, .at 0)) : Float)
  second = (compute.read ((compute.Read _)(.from k.in.b, .at 0)) : Float)
  third = (compute.read ((compute.Read _)(.from k.in.c, .at 0)) : Float)
  compute.write ((compute.Write _)(.to k.out.y, .at 0, .value (first + (second * 10.0) + (third * 100.0))))
}
mix = compute.parallel mix_raw "gpu" Out
CopPar = struct{.n Int, .in CopIn, .out CopIn}
CopArg = struct{.n Int, .I CopIn}
copy_raw = (k : CopPar) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun mix ((MixArg)(.n s.n, .I In(.a s.in.a, .b s.in.b, .c s.in.c))) : Out)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d a.y)))
}
built = compute.graph step
built
"#;

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
        _ => Err("the mix answers with one buffer, and this one answered otherwise".into()),
    }
}

/// The answer says which order the three buffers arrive in.
fn measure(session: &Session) -> Result<(), String> {
    let prepared = session
        .prepare(MIX_STEP)
        .map_err(|refusal| format!("the mix did not prepare: {refusal}"))?;
    println!("the held step is {} dispatches", prepared.nodes());
    let stepped = prepared
        .step(vec![
            RunArgument::Count(1),
            floats(&[1.0]),
            floats(&[2.0]),
            floats(&[3.0]),
        ])
        .map_err(|refusal| format!("the mix did not run: {refusal}"))?;
    let resident = only(&stepped)?;
    let answered = match session.fetch(resident) {
        Ok(ScalarData::Float(values)) => values,
        Ok(other) => return Err(format!("the answer declared a Float buffer: {other:?}")),
        Err(refusal) => return Err(format!("the answer is not readable: {refusal}")),
    };
    println!("a=1, b=2, c=3 answered {}", answered[0]);
    session.release(resident);
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

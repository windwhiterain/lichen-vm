//! What order the step's arguments arrive in, measured rather than assumed.

use super::common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
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

/// The answer says which order the three buffers arrive in.
#[test]
fn the_arguments_arrive_in_a_measurable_order() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED the_arguments_arrive_in_a_measurable_order: no device");
        return;
    };
    let prepared = session
        .prepare(MIX_STEP)
        .unwrap_or_else(|refusal| panic!("the mix prepares: {refusal}"));
    println!("the held step is {} dispatches", prepared.nodes());
    let stepped = prepared
        .step(vec![
            RunArgument::Count(1),
            floats(&[1.0]),
            floats(&[2.0]),
            floats(&[3.0]),
        ])
        .unwrap_or_else(|refusal| panic!("the mix runs: {refusal}"));
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("the mix answers with one buffer, and this one answered otherwise");
    };
    let answered = read(&session, *resident);
    println!("a=1, b=2, c=3 answered {}", answered[0]);
    session.release(*resident);
}

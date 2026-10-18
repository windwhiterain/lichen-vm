//! How many buffer fields one argument struct binds, measured rather than assumed.

mod common;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::Session;

/// A step whose `In` struct holds `count` buffers, answering `sum (i + 1) * 10^i`.
///
/// # Invariant
/// Each buffer `i` holds `i + 1` at element zero, so the answer is a decimal number whose digits are the
/// positions that arrived: `1234` says four buffers reached four fields in order, and a smaller number
/// says the ones above it were read as zero.
fn program(count: usize) -> String {
    let fields: Vec<String> = (0..count)
        .map(|index| format!(".b{index} (compute.Buf Float)"))
        .collect();
    let reads: String = (0..count)
        .map(|index| {
            format!("  value{index} = (compute.read ((compute.Read _)(.from k.in.b{index}, .at 0)) : Float)\n")
        })
        .collect();
    let mut sum = String::from("value0");
    for index in 1..count {
        let weight = 10_f64.powi(index as i32);
        sum.push_str(&format!(" + (value{index} * {weight:?})"));
    }
    let names: Vec<String> = (0..count)
        .map(|index| format!(".b{index} s.in.b{index}"))
        .collect();
    format!(
        r#"---
  compute = import "compute.lichen"
---
In     = struct{{{fields}}}
Out    = struct{{.y (compute.Buf Float)}}
MixPar = struct{{.n Int, .in In, .out Out}}
MixArg = struct{{.n Int, .I In}}
mix_raw = (k : MixPar) => {{
{reads}  compute.write ((compute.Write _)(.to k.out.y, .at 0, .value ({sum})))
}}
mix = compute.parallel mix_raw "gpu" Out
CopIn  = struct{{.d (compute.Buf Float)}}
CopPar = struct{{.n Int, .in CopIn, .out CopIn}}
CopArg = struct{{.n Int, .I CopIn}}
copy_raw = (k : CopPar) => {{
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.d, .at i)) : Float)
  compute.write ((compute.Write _)(.to k.out.d, .at i, .value value))
}}
copy = compute.parallel copy_raw "gpu" CopIn
GArg = struct{{.n Int, .in In}}
step = (s : GArg) => {{
  mixed = (compute.plrun mix ((MixArg)(.n s.n, .I In({names}))) : Out)
  compute.plrun copy ((CopArg)(.n s.n, .I CopIn(.d mixed.y)))
}}
built = compute.graph step
built
"#,
        fields = fields.join(", "),
        reads = reads,
        sum = sum,
        names = names.join(", "),
    )
}

/// A struct of up to twelve buffers binds all of them, in the order the source names them.
///
/// # Invariant
/// The expected answer is the decimal `1234...`, so a run that bound only the first `m` buffers would
/// answer a number with `m` digits: the reading is the count itself. Twelve is where this stops being
/// probed, not where it stops working.
#[test]
fn how_many_buffers_one_argument_struct_binds() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED how_many_buffers_one_argument_struct_binds: no device");
        return;
    };
    for count in 1..=12_usize {
        let prepared = match session.prepare(&program(count)) {
            Ok(prepared) => prepared,
            Err(refusal) => {
                println!("{count} buffers: the program does not prepare: {refusal}");
                continue;
            }
        };
        let mut arguments = vec![RunArgument::Count(1)];
        let mut expected = 0.0_f32;
        for index in 0..count {
            arguments.push(floats(&[(index + 1) as f32]));
            expected += (index + 1) as f32 * 10_f32.powi(index as i32);
        }
        let stepped = match prepared.step(arguments) {
            Ok(stepped) => stepped,
            Err(refusal) => {
                println!("{count} buffers: the step refuses: {refusal}");
                continue;
            }
        };
        let [RunResult::Resident(resident)] = &stepped.results[..] else {
            println!("{count} buffers: the step answered something other than one buffer");
            continue;
        };
        let answered = read(&session, *resident);
        session.release(*resident);
        assert!(
            (answered[0] - expected).abs() < 0.5,
            "{count} buffers answered {} where the digits say {expected}",
            answered[0]
        );
        println!("{count} buffers: {answered:?} bound in order");
    }
}

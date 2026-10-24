//! Whether the region merge fuses a plain producer-consumer chain.

use lichen_compute::placement_pack::read_placement_pack;
use lichen_serve::Session;

const CHAIN: &str = r#"
--- compute = import "compute.lichen" ---
In  = struct{.x (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Out = struct{.y (compute.Buf ((compute.BufParam _)(.type Float, .length _)))}
Par = struct{.n Int, .in In}
Arg = struct{.n Int, .I In}
copy_raw = (k : Par) => {
  i = compute.range k.n
  value = (compute.read ((compute.Read _)(.from k.in.x, .at i)) : Float)
  b_y = compute.buf ((compute.BufParam _)(.type Float, .length k.n))
  ((Out)(.y (compute.write ((compute.Write _)(.to b_y, .at i, .value value)))))
}
copy = compute.parallel copy_raw "gpu" Out
GArg = struct{.n Int, .in In}
step = (s : GArg) => {
  a = (compute.plrun copy ((Arg)(.n 256, .I In(.x s.in.x))) : Out)
  b = (compute.plrun copy ((Arg)(.n 256, .I In(.x a.y))) : Out)
  b
}
built = compute.graph step
built
"#;

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    let text = r#"{"name":"narrow","requires":{"class":"bfloat16","classes":null,"nodes":null},"plan":{"staging":[]}}"#;
    let pack = read_placement_pack(text).expect("the pack reads");
    let prepared = session
        .prepare_with_packs(CHAIN, &[pack])
        .expect("the chain prepares");
    for line in prepared.report() {
        println!("{line}");
    }
    println!("{} node(s)", prepared.nodes());
}

//! Whether an Elementwise claim reaches the registry at all.

use lichen_serve::Session;

const SOURCE: &str = r#"
--- compute = import "compute.lichen" ---
Out = struct{.z (compute.Buf ((compute.BufParam _)(.type Float, .length 256)))}
Fill = struct{.n Int}
fill_raw = (c : Fill) => {
  i = compute.range c.n
  y = compute.buf ((compute.BufParam _)(.type Float, .length 256))
  ((Out)(.z (compute.write ((compute.Write _)(.to y, .at i, .value ((int2float i + 1.0) ~ "element"))))))
}
filler = compute.parallel (fill_raw ~ "claim:Elementwise(element)") "cpu" Out
data = (compute.plrun filler ((Fill)(.n 256)) : Out)
data
"#;

fn main() {
    let session = match Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("no device: {refusal}");
            std::process::exit(1);
        }
    };
    match session.prepare(SOURCE) {
        Ok(prepared) => {
            println!("prepared, {} nodes", prepared.nodes());
            for line in prepared.report() {
                println!("{line}");
            }
        }
        Err(refusal) => println!("refused: {refusal}"),
    }
}

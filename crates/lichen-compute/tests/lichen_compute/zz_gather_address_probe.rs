//! Probe: an address that is itself a read — the indirection a page table needs.
//!
//! # Invariant
//!
//! Print-only. The body is `y[i] = x[tbl[i]]`, so one read's address is another read's
//! value. If this answers, a paged KV cache and a token-to-embedding lookup are both
//! expressible without recompiling per shape, because the mapping travels as data. The
//! second case sends one lane past the buffer's end, which is where a masked batch lives.

use lichen_compute::language::new_store;

/// The gather: one read's address is another read's value, over filled buffers.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  := {.x (compute.Buf (compute.BufParam{.type Int, .length 6})), .tbl (compute.Buf (compute.BufParam{.type Int, .length 6}))}
POut := {.y (compute.Buf (compute.BufParam{.type Int, .length 6}))}
PPar := {.n @any : Int, .in PIn}
PArg := {.n @any : Int, .I PIn}
gather_raw := ('k = PPar) -> {
  i := compute.range k.n
  j := (compute.read ((compute.Read _){.from k.in.tbl, .at i}) : Int)
  o := compute.buf (compute.BufParam{.type Int, .length 6})
  ((POut){.y (compute.write ((compute.Write _){.to o, .at i, .value ((compute.read ((compute.Read _){.from k.in.x, .at j}) : Int))}))})
}
gather := compute.parallel gather_raw "cpu" POut
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 6})
  b := compute.buf (compute.BufParam{.type Int, .length 6})
  ((PIn){.x (compute.write ((compute.Write _){.to a, .at i, .value (i * 10 + 10)})), .tbl (compute.write ((compute.Write _){.to b, .at i, .value ((i % 2 == 0) * (5 - i / 2) + (i % 2 == 1) * ((i - 1) / 2))}))})
}
filler := compute.parallel fill "cpu" PIn
data := (compute.plrun filler ((Fill){.n 6}) : Struct)
x := data.x
tbl := data.tbl
far_fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 6})
  b := compute.buf (compute.BufParam{.type Int, .length 6})
  ((PIn){.x (compute.write ((compute.Write _){.to a, .at i, .value (i * 10 + 10)})), .tbl (compute.write ((compute.Write _){.to b, .at i, .value ((i % 2 == 0) * (5 - i / 2) + (i % 2 == 1) * ((i - 1) / 2) + 9 * (i == 1))}))})
}
far_filler := compute.parallel far_fill "cpu" PIn
far_data := (compute.plrun far_filler ((Fill){.n 6}) : Struct)
far := far_data.tbl
"#;

/// The plain gather: `y[i] = x[tbl[i]]`, so `[60, 10, 50, 20, 40, 30]`.
const PLAIN: &str = r#"
gathered := (compute.plrun gather ((PArg){.n 6, .I PIn{.x x, .tbl tbl}}) : Struct)
compute.collect gathered.y : Array
"#;

/// The gather past the buffer's end: one lane names an index no element answers.
///
/// # Invariant
/// An out-of-buffer read answers `0`, which is the padding a masked batch relies on, so
/// `tbl = [5, 9, 4, 1, 3, 2]` answers `[60, 0, 50, 20, 40, 30]` rather than refusing.
const PAST_THE_END: &str = r#"
gathered := (compute.plrun gather ((PArg){.n 6, .I PIn{.x x, .tbl far}}) : Struct)
compute.collect gathered.y : Array
"#;

/// What the raw runner renders for `body`, or how it refused.
fn rendered(body: &str) -> String {
    let source = format!("{KERNEL}{body}");
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&source, None, &mut store) {
        Ok(text) => text,
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_the_gather_address() {
    println!("[plain] {}", rendered(PLAIN));
    println!("[past_the_end] {}", rendered(PAST_THE_END));
}

//! Probe: an address that is itself a read — the indirection a page table needs.
//!
//! # Invariant
//!
//! Print-only. The body is `y[i] = x[tbl[i]]`, so one read's address is another read's
//! value. If this answers, a paged KV cache and a token-to-embedding lookup are both
//! expressible without recompiling per shape, because the mapping travels as data. The
//! second case sends one lane past the buffer's end, which is where a masked batch lives.

use super::common;

use lichen_compute::language::new_store;

/// The gather: one read's address is another read's value.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  := _x => {.x (compute.Buf (compute.BufParam{.type Int, .length _})), .tbl (compute.Buf (compute.BufParam{.type Int, .length _}))}
POut := _x => {.y (compute.Buf (compute.BufParam{.type Int, .length _}))}
PPar := _x => {.n @any : Int, .in PIn _, .out POut}
PArg := _x => {.n @any : Int, .I PIn _}
gather_raw := (k <: PPar) => {
  i := compute.range k.n
  j := (compute.read (compute.Read{.from k.in.tbl, .at i}) : Int)
  compute.write (compute.Write{.to k.out.y, .at i, .value ((compute.read (compute.Read{.from k.in.x, .at j}) : Int))})
}
gather := compute.parallel gather_raw "gpu" (POut _)
x   := (compute.buffer ([@any : Int, 6][10, 20, 30, 40, 50, 60] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
tbl := (compute.buffer ([@any : Int, 6][5, 0, 4, 1, 3, 2] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
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
far := (compute.buffer ([@any : Int, 6][5, 9, 4, 1, 3, 2] : Array) : (compute.Buf (compute.BufParam{{.type Int, .length _}})))
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
    let (_stub, _guard) = common::stub();
    println!("[plain] {}", rendered(PLAIN));
    println!("[past_the_end] {}", rendered(PAST_THE_END));
}

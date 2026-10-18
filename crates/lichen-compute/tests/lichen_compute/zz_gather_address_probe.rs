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
PIn  = struct{.x (compute.Buf Int), .tbl (compute.Buf Int)}
POut = struct{.y (compute.Buf Int)}
PPar = struct{.n Int, .in PIn, .out POut}
PArg = struct{.n Int, .I PIn}
gather_raw = (k : PPar) => {
  i = compute.range k.n
  j = compute.read ((compute.Read _)(.from k.in.tbl, .at i))
  compute.write ((compute.Write _)(.to k.out.y, .at i, .value (compute.read ((compute.Read _)(.from k.in.x, .at j)))))
}
gather = compute.parallel gather_raw "gpu"
x   = (compute.buffer ([10, 20, 30, 40, 50, 60] : array{Int, 6}) : (compute.Buf Int))
tbl = (compute.buffer ([5, 0, 4, 1, 3, 2] : array{Int, 6}) : (compute.Buf Int))
"#;

/// The plain gather: `y[i] = x[tbl[i]]`, so `[60, 10, 50, 20, 40, 30]`.
const PLAIN: &str = r#"
gathered = (compute.plrun gather ((PArg)(.n 6, .I PIn(.x x, .tbl tbl))) : POut)
compute.collect gathered.y : array{Int, 6}
"#;

/// The gather past the buffer's end: one lane names an index no element answers.
///
/// # Invariant
/// An out-of-buffer read answers `0`, which is the padding a masked batch relies on, so
/// `tbl = [5, 9, 4, 1, 3, 2]` answers `[60, 0, 50, 20, 40, 30]` rather than refusing.
const PAST_THE_END: &str = r#"
far = (compute.buffer ([5, 9, 4, 1, 3, 2] : array{Int, 6}) : (compute.Buf Int))
gathered = (compute.plrun gather ((PArg)(.n 6, .I PIn(.x x, .tbl far))) : POut)
compute.collect gathered.y : array{Int, 6}
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
    let _guard = common::device("zz_gather_address_probe");
    println!("[plain] {}", rendered(PLAIN));
    println!("[past_the_end] {}", rendered(PAST_THE_END));
}

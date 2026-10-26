//! Probe: one dispatch writing into a sub-range of its output, at a launch-fixed offset.
//!
//! # Invariant
//!
//! Print-only. Three cases over one kernel and one input: `.off 0` collected at the extent is
//! the control, and `.off 4` collected at eight and at four ask whether the output slot's
//! length follows the launch extent or the array the collect declares.

use super::common;

use lichen_compute::language::new_store;

/// The kernel: `y[i + off] = x[i]`, with the offset a runtime scalar leaf beside the extent.
const KERNEL: &str = r#"
--- compute = import "compute.lichen" ---
PIn  := _x => {.x (compute.Buf (compute.BufParam{.type Int, .length _}))}
POut := _x => {.y (compute.Buf (compute.BufParam{.type Int, .length _}))}
PPar := _x => {.n @any : Int, .off @any : Int, .in PIn _, .out POut}
PArg := _x => {.n @any : Int, .off @any : Int, .I PIn _}
append_raw := (c <: PPar) => {
  i := compute.range c.n
  v := (compute.read (compute.Read{.from c.in.x, .at i}) : Int)
  compute.write (compute.Write{.to c.out.y, .at (i + c.off), .value v})
}
append := compute.parallel append_raw "gpu" (POut _)
x := (compute.buffer ([@any : Int, 4][1, 2, 3, 4] : Array) : (compute.Buf (compute.BufParam{.type Int, .length _})))
"#;

/// (a) The control: offset 0, collected at the extent. Hand answer `[1, 2, 3, 4]`.
const OFFSET_ZERO: &str = r#"
out := (compute.plrun append ((PArg){.n 4, .off 0, .I PIn{.x x}}) : Struct)
compute.collect out.y : Array
"#;

/// (b) The question: offset 4, collected at eight — the output's length against the extent's.
const OFFSET_FOUR_WIDE: &str = r#"
out := (compute.plrun append ((PArg){.n 4, .off 4, .I PIn{.x x}}) : Struct)
compute.collect out.y : Array
"#;

/// (c) The same launch collected at four: does the declared array or the extent win?
const OFFSET_FOUR_NARROW: &str = r#"
out := (compute.plrun append ((PArg){.n 4, .off 4, .I PIn{.x x}}) : Struct)
compute.collect out.y : Array
"#;

/// (d) A control for (b): an offset landing **half** inside the extent, answer `[0, 0, 1, 2]`.
const OFFSET_TWO: &str = r#"
out := (compute.plrun append ((PArg){.n 4, .off 2, .I PIn{.x x}}) : Struct)
compute.collect out.y : Array
"#;

/// (e) A control for the declared length: offset 0 collected at eight, the same four numbers.
const OFFSET_ZERO_WIDE: &str = r#"
out := (compute.plrun append ((PArg){.n 4, .off 0, .I PIn{.x x}}) : Struct)
compute.collect out.y : Array
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
fn probe_the_subrange_write() {
    let (_stub, _guard) = common::stub();
    println!("[a off0_at4] {}", rendered(OFFSET_ZERO));
    println!("[b off4_at8] {}", rendered(OFFSET_FOUR_WIDE));
    println!("[c off4_at4] {}", rendered(OFFSET_FOUR_NARROW));
    println!("[d off2_at4] {}", rendered(OFFSET_TWO));
    println!("[e off0_at8] {}", rendered(OFFSET_ZERO_WIDE));
}

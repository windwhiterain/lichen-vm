//! Probe: a dynamic-rank contraction over a plan buffer.
//!
//! # Invariant
//!
//! Print-only, asserts nothing. The plan is four (A stride, B stride, extent) entries:
//! `r0`/`c2` then `c0`/`c1`; the kernel decodes through `rank` entries and folds the rest.
//! Rank 2/2 answers `[2938, 7042, …]`, rank 3/1 `[330, 1554, …]` (`named-tensors.md` §11).
//!
//! # Invariant
//! The plan is host data: `Shape`'s first field is the lane count, the rest are the twelve
//! plan entries, and the plan kernel only materializes them into the buffer. A kernel's
//! launch extent is its parameter's first integer field.

use lichen_compute::language::new_store;

const PROGRAM: &str = r#"
--- compute = import "compute.lichen" ---
Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
In   := {.a (compute.Buf (compute.BufParam{.type Float, .length 24})), .b (compute.Buf (compute.BufParam{.type Float, .length 60})), .plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
Out  := {.c (compute.Buf (compute.BufParam{.type Float, .length 10}))}
Par  := {.meta Meta, .in In}
Arg  := {.meta Meta, .I In}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 24}))}
BBuf := {.b (compute.Buf (compute.BufParam{.type Float, .length 60}))}
PBuf := {.plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
Fill := {.n @any : Int}
fill_a := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 24})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (int2float (i + 1))}))})
}
fill_b := ('f = Fill) -> {
  i := compute.range f.n
  b := compute.buf (compute.BufParam{.type Float, .length 60})
  ((BBuf){.b (compute.write ((compute.Write _){.to b, .at i, .value (int2float (i + 1))}))})
}
Shape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
fill_plan := ('f = Shape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel fill_plan "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
data_p := (compute.plrun p_kernel ((Shape){@SHAPE@}) : Struct)
body := ('p = Par) -> {
  i := compute.range p.meta.n
  rank := p.meta.rank
  contract := p.meta.contract
  @loop decode_a := 's -> if s(0) == 0 then s(2) else {
    dim := rank - s(0)
    extent := (compute.read ((compute.Read _){.from p.in.plan, .at (dim * 3 + 2)}) : Int)
    coordinate := s(1) % extent
    decode_a (s(0) - 1, s(1) / extent, s(2) + coordinate * ((compute.read ((compute.Read _){.from p.in.plan, .at (dim * 3)}) : Int)))
  }
  @loop decode_b := 's -> if s(0) == 0 then s(2) else {
    dim := rank - s(0)
    extent := (compute.read ((compute.Read _){.from p.in.plan, .at (dim * 3 + 2)}) : Int)
    coordinate := s(1) % extent
    decode_b (s(0) - 1, s(1) / extent, s(2) + coordinate * ((compute.read ((compute.Read _){.from p.in.plan, .at (dim * 3 + 1)}) : Int)))
  }
  @loop address_a := 't -> if t(0) == 0 then t(2) else {
    dim := t(4) + (t(5) - t(0))
    extent := (compute.read ((compute.Read _){.from t(3), .at (dim * 3 + 2)}) : Int)
    coordinate := t(1) % extent
    address_a (t(0) - 1, t(1) / extent, t(2) + coordinate * ((compute.read ((compute.Read _){.from t(3), .at (dim * 3)}) : Int)), t(3), t(4), t(5))
  }
  @loop address_b := 't -> if t(0) == 0 then t(2) else {
    dim := t(4) + (t(5) - t(0))
    extent := (compute.read ((compute.Read _){.from t(3), .at (dim * 3 + 2)}) : Int)
    coordinate := t(1) % extent
    address_b (t(0) - 1, t(1) / extent, t(2) + coordinate * ((compute.read ((compute.Read _){.from t(3), .at (dim * 3 + 1)}) : Int)), t(3), t(4), t(5))
  }
  base_a := decode_a (rank, i, 0)
  base_b := decode_b (rank, i, 0)
  @loop dot := 'u -> if u(0) == 0 then u(1) else {
    j := u(0) - 1
    ca := address_a (contract, j, 0, p.in.plan, rank, contract)
    cb := address_b (contract, j, 0, p.in.plan, rank, contract)
    dot (u(0) - 1, (u(1) + ((compute.read ((compute.Read _){.from p.in.a, .at (base_a + ca)}) : Float) * (compute.read ((compute.Read _){.from p.in.b, .at (base_b + cb)}) : Float))))
  }
  value := dot (p.meta.total, 0.0)
  o := compute.buf (compute.BufParam{.type Float, .length p.meta.n})
  ((Out){.c (compute.write ((compute.Write _){.to o, .at i, .value value}))})
}
kernel := compute.parallel body "cpu" Out
out := (compute.plrun kernel ((Arg){.meta Meta{.n 10, .rank @RANK@, .contract @CONTRACT@, .total 12}, .I In{.a data_a.a, .b data_b.b, .plan data_p.plan}}) : Struct)
compute.collect out.c : Array
"#;

/// One contraction dim, as the host's shape table states it.
struct Dim {
    a_stride: u32,
    b_stride: u32,
    extent: u32,
}

/// The plan's integers, three per dim, in the table's order (test scaffolding).
fn plan_entries(dims: &[Dim]) -> Vec<i64> {
    let mut entries = Vec::with_capacity(dims.len() * 3);
    for dim in dims {
        entries.push(i64::from(dim.a_stride));
        entries.push(i64::from(dim.b_stride));
        entries.push(i64::from(dim.extent));
    }
    entries
}

/// The host's shape table: the demo's four dims, in plan order.
///
/// # Invariant
/// This is the host's data: the shape value's first field is the lane count, then the plan's
/// entries — the plan kernel only materializes it.
fn shape_table() -> [Dim; 4] {
    [
        Dim {
            a_stride: 12,
            b_stride: 0,
            extent: 2,
        },
        Dim {
            a_stride: 0,
            b_stride: 1,
            extent: 5,
        },
        Dim {
            a_stride: 4,
            b_stride: 20,
            extent: 3,
        },
        Dim {
            a_stride: 1,
            b_stride: 5,
            extent: 4,
        },
    ]
}

/// The `Shape` literal the host hands the program: its lane count, then the plan entries.
fn shape_value(table: &[Dim]) -> String {
    let entries = plan_entries(table);
    let mut fields = vec![format!(".n {}", entries.len())];
    for (index, value) in entries.iter().enumerate() {
        fields.push(format!(".s{index} {value}"));
    }
    fields.join(", ")
}

/// The program at one rank and contraction count, with the host's plan.
fn program(rank: u32, contract: u32) -> String {
    PROGRAM
        .replace("@RANK@", &rank.to_string())
        .replace("@CONTRACT@", &contract.to_string())
        .replace("@SHAPE@", &shape_value(&shape_table()))
}

#[test]
fn probe_a_dynamic_rank_contraction() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&program(2, 2), None, &mut store) {
        Ok(text) => println!("[dynamic] {text}"),
        Err(diagnostics) => println!(
            "[dynamic] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// Rank 3, one contraction; reference `[330, 1554, 360, 1728, 390, 1902, 420, 2076, 450, 2250]`.
#[test]
fn probe_a_three_dimensional_contraction() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(&program(3, 1), None, &mut store) {
        Ok(text) => println!("[three] {text}"),
        Err(diagnostics) => println!(
            "[three] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

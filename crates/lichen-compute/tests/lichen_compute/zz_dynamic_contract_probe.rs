//! Probe: a dynamic-rank contraction over a plan buffer.
//!
//! # Invariant
//!
//! Print-only, asserts nothing. The plan is four (A stride, B stride, extent) entries:
//! `r0`/`c2` then `c0`/`c1`; the kernel decodes through `rank` entries and folds the rest.
//! Rank 2/2 answers `[2938, 7042, …]`, rank 3/1 `[330, 1554, …]` (`named-tensors.md` §11).

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
fill_plan := ('f = Fill) -> {
  i := compute.range f.n
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  dim := i / 3
  slot := i - dim * 3
  a_stride := (dim == 0) * 12 + (dim == 2) * 4 + (dim == 3) * 1
  b_stride := (dim == 1) * 1 + (dim == 2) * 20 + (dim == 3) * 5
  extent := (dim == 0) * 2 + (dim == 1) * 5 + (dim == 2) * 3 + (dim == 3) * 4
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value ((slot == 0) * a_stride + (slot == 1) * b_stride + (slot == 2) * extent)}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel fill_plan "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
data_p := (compute.plrun p_kernel ((Fill){.n 12}) : Struct)
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

/// The program at one rank and contraction count, as the template states them.
fn program(rank: u32, contract: u32) -> String {
    PROGRAM
        .replace("@RANK@", &rank.to_string())
        .replace("@CONTRACT@", &contract.to_string())
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

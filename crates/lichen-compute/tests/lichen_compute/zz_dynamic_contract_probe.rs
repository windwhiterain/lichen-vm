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

/// The plan kernel run inside a plain function, returning its buffer.
///
/// # Invariant
/// The library entry's middle step: a function builds the plan value, runs the plan kernel,
/// and hands back the plan buffer.
const PLAN_IN_FUNCTION: &str = r#"
--- compute = import "compute.lichen" ---
PBuf := {.plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
p_kernel := compute.parallel plan_fill "cpu" PBuf
plan_of := 'n0 -> ('ashape : table{string, Int}) -> {
  e := ashape <== n0
  ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 e, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
  dp := (compute.plrun p_kernel ps : Struct)
  dp.plan
}
t := table(table{string, Int})("i" => 2, "j" => 3)
p := plan_of "i" t
compute.collect p : Array
"#;

/// A kernel run inside a plain function, and the result's field read.
///
/// # Invariant
/// The library's entry shape: a plain function runs the kernel and hands back the result's
/// buffer field; the reading answers the filled values.
const FUNCTION_RUN: &str = r#"
--- compute = import "compute.lichen" ---
Fill := {.n @any : Int}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 4}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (int2float (i + 1))}))})
}
k := compute.parallel fill "cpu" ABuf
run := ('f = Fill) -> {
  out := (compute.plrun k f : Struct)
  compute.collect out.a : Array
}
run ((Fill){.n 4})
"#;

/// The named Einstein contraction: the tensor carries its dim names, and the library derives
/// the plan from them.
///
/// # Invariant
/// The plan's order is the output list then the contraction list; a dim's extent is the
/// named shape's answer in whichever operand carries it, and the other operand's stride is
/// zero (its name is not in that operand's set, so the lookup is skipped, not defaulted).
const NAMED_CONTRACT: &str = r#"
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
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
ShapeTable := table{string, Int}
contract := 'abuf -> ('ashape : ShapeTable) -> ('astride : ShapeTable) -> 'bbuf -> ('bshape : ShapeTable) -> ('bstride : ShapeTable) -> {
  e0 := ashape <== "i"
  e1 := ashape <== "l"
  e2 := ashape <== "j"
  e3 := ashape <== "k"
  as0 := astride <== "i"
  bs0 := bstride <== "i"
  as1 := astride <== "l"
  bs1 := bstride <== "l"
  as2 := astride <== "j"
  bs2 := bstride <== "j"
  as3 := astride <== "k"
  bs3 := bstride <== "k"
  ps := PlanShape{.n 12, .s0 as0, .s1 bs0, .s2 e0, .s3 as1, .s4 bs1, .s5 e1, .s6 as2, .s7 bs2, .s8 e2, .s9 as3, .s10 bs3, .s11 e3}
  dp := (compute.plrun p_kernel ps : Struct)
  meta := Meta{.n e0 * e1, .rank 4, .contract 2, .total e2 * e3}
  (compute.plrun kernel ((Arg){.meta meta, .I In{.a abuf, .b bbuf, .plan dp.plan}}) : Struct)
}
ashape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
astride := table(table{string, Int})("i" => 12, "l" => 0, "j" => 4, "k" => 1)
bshape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
bstride := table(table{string, Int})("i" => 0, "l" => 1, "j" => 20, "k" => 5)
c := contract data_a.a ashape astride data_b.b bshape bstride
compute.collect c.c : Array
"#;

/// A tensor whose shape and stride are **named tables**, and a lookup by name.
///
/// # Invariant
/// The named form's primitives: a `table{Str, Int}` field on a tensor value, a lookup by a
/// string name, and an absent name's answer.
const NAMED_TABLE: &str = r#"
ShapeTable := table{string, Int}
t := table(table{string, Int})("i" => 2, "j" => 3)
read := ('s : ShapeTable) -> s <== "i"
read2 := 'b -> ('s : ShapeTable) -> s <== "j"
NameSet := set{string}
names := set(set{string})("i", "j")
has := ('s : NameSet) -> "i" @in s
get := ('n : string) -> ('s : ShapeTable) -> ('ns : NameSet) -> if n @in ns then s <== n else 0
(read t, read2 7 t, has names, get "i" t names, get "l" t names)
"#;

/// A plan read inside a struct literal, then returned.
///
/// # Invariant
/// Print-only. The plan kernel runs in a function, its field read feeds a struct literal,
/// and the caller collects the field — the named contract's inner flow, one step simpler.
const PLAN_IN_LITERAL: &str = r#"
--- compute = import "compute.lichen" ---
PBuf := {.plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
p_kernel := compute.parallel plan_fill "cpu" PBuf
run := ('ps = PlanShape) -> {
  dp := (compute.plrun p_kernel ps : Struct)
  ((PBuf){.plan dp.plan})
}
ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 2, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
r := run ps
compute.collect r.plan : Array
"#;

/// Two kernels in one function: the plan kernel's field and a second kernel's result.
///
/// # Invariant
/// Print-only. The second run takes the first's field inside a struct literal, and the
/// caller collects the second result — the named contract's shape with a square kernel.
const TWO_RUNS: &str = r#"
--- compute = import "compute.lichen" ---
PBuf := {.plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 4}))}
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (int2float (i + 1))}))})
}
a_kernel := compute.parallel fill "cpu" ABuf
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
p_kernel := compute.parallel plan_fill "cpu" PBuf
run := ('ps = PlanShape) -> {
  dp := (compute.plrun p_kernel ps : Struct)
  d := (compute.plrun a_kernel ((Fill){.n 4}) : Struct)
  ((PBuf){.plan dp.plan})
}
ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 2, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
r := run ps
compute.collect r.plan : Array
"#;

/// An unannotated buffer parameter, placed in a struct literal, read back.
///
/// # Invariant
/// Print-only. The named contract's buffers are unannotated parameters; this states whether
/// such a parameter can fill a struct field and be read.
const BUFFER_PARAMETER: &str = r#"
--- compute = import "compute.lichen" ---
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 4}))}
Fill := {.n @any : Int}
Holder := {.x (compute.Buf (compute.BufParam{.type Float, .length 4}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (int2float (i + 1))}))})
}
a_kernel := compute.parallel fill "cpu" ABuf
data := (compute.plrun a_kernel ((Fill){.n 4}) : Struct)
run := 'b -> {
  h := Holder{.x b}
  h
}
r := run data.a
compute.collect r.x : Array
"#;

/// The contract kernel inside a function, its plan handed in from the top level.
///
/// # Invariant
/// Print-only. The kernel runs with a plan buffer parameter; the caller collects the result.
const CONTRACT_KERNEL_IN_FUNCTION: &str = r#"
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
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
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 2, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
dp := (compute.plrun p_kernel ps : Struct)
kernel := compute.parallel body "cpu" Out
run := 'abuf -> 'bbuf -> 'plan_b -> {
  (compute.plrun kernel ((Arg){.meta Meta{.n 10, .rank 4, .contract 2, .total 12}, .I In{.a abuf, .b bbuf, .plan plan_b}}) : Struct)
}
c := run data_a.a data_b.b dp.plan
compute.collect c.c : Array
"#;

#[test]
fn probe_a_contract_kernel_in_function() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(CONTRACT_KERNEL_IN_FUNCTION, None, &mut store) {
        Ok(text) => println!("[contract-in-function] {text}"),
        Err(diagnostics) => println!(
            "[contract-in-function] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The minimal field-read case: a local struct value built from a parameter.
///
/// # Invariant
/// Print-only. The same read at the top level answers; inside a function it is the
/// question this probe answers.
const LOCAL_STRUCT_READ: &str = r#"
T := {.a @any : Int}
f := 'x -> { p := T{.a x}; p.a }
f 7
"#;

#[test]
fn probe_a_local_struct_read() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(LOCAL_STRUCT_READ, None, &mut store) {
        Ok(text) => println!("[local-struct-read] {text}"),
        Err(diagnostics) => println!(
            "[local-struct-read] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The read/write chain in one function, with the consumer's argument a **user struct**.
///
/// # Invariant
/// Print-only. With `(compute.A _)` as the argument it answers `[20, 22, 24]`; with the
/// user struct `In2A` it answers `raw none`, so the declared field's type value is the
/// trigger, not the function boundary.
const FUNCTION_USER_STRUCT_ARGUMENT: &str = r#"
--- compute = import "compute.lichen" ---
In1  := {.a @any : Int}
Out1 := {.z (compute.Buf (compute.BufParam{.type Int, .length 3}))}
Par1 := {.n @any : Int, .in In1}
f1 := ('k = Par1) -> {
  i := compute.range k.n
  z := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out1){.z (compute.write ((compute.Write _){.to z, .at i, .value i + 10}))})
}
k1 := compute.parallel f1 "cpu" Out1
In2  := {.b (compute.Buf (compute.BufParam{.type Int, .length 3}))}
Out2 := {.w (compute.Buf (compute.BufParam{.type Int, .length 3}))}
Par2 := {.n @any : Int, .in In2}
f2 := ('k = Par2) -> {
  i := compute.range k.n
  a := (compute.read ((compute.Read _){.from k.in.b, .at i}) : Int)
  w := compute.buf (compute.BufParam{.type Int, .length k.n})
  ((Out2){.w (compute.write ((compute.Write _){.to w, .at i, .value a + a}))})
}
k2 := compute.parallel f2 "cpu" Out2
In2A := {.n @any : Int, .in In2}
run := 'n -> {
  inbuf := (compute.plrun k1 ((compute.A _){.n n, .I In1{.a 0}}) : Struct)
  out := (compute.plrun k2 ((In2A){.n n, .in In2{.b inbuf.z}}) : Struct)
  compute.collect out.w : Array
}
run 3
"#;

/// The chained probe with `.meta` removed and one input field, read from the argument.
///
/// # Invariant
/// Print-only. Answers raw none, so neither the meta nesting nor the field count is it.
const CHAIN_NO_META: &str = r#"
--- compute = import "compute.lichen" ---
ABuf := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Buffers := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Sum := {.n @any : Int, .in Buffers}
SumOut := {.c (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
}
sum := ('p = Sum) -> {
  i := compute.range p.n
  o := compute.buf (compute.BufParam{.type Int, .length 4})
  ((SumOut){.c (compute.write ((compute.Write _){.to o, .at i, .value (compute.read ((compute.Read _){.from p.in.a, .at i}) : Int)}))})
}
a_kernel := compute.parallel fill "cpu" ABuf
sum_kernel := compute.parallel sum "cpu" SumOut
run := 'n -> {
  d1 := (compute.plrun a_kernel ((Fill){.n n}) : Struct)
  (compute.plrun sum_kernel ((Sum){.n 4, .in Buffers{.a d1.a}}) : Struct)
}
r := run 4
compute.collect r.c : Array
"#;

#[test]
fn probe_a_chain_no_meta() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(CHAIN_NO_META, None, &mut store) {
        Ok(text) => println!("[chain-no-meta] {text}"),
        Err(diagnostics) => println!(
            "[chain-no-meta] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_function_user_struct_argument() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_USER_STRUCT_ARGUMENT, None, &mut store) {
        Ok(text) => println!("[function-user-struct-argument] {text}"),
        Err(diagnostics) => println!(
            "[function-user-struct-argument] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The consumer call replaced by a direct collect of the producer's field inside the
/// same function.
///
/// # Invariant
/// Print-only. Answers the buffer's four values when the producer's result itself is
/// sound and the loss is in the argument literal.
const FUNCTION_DIRECT_COLLECT: &str = r#"
--- compute = import "compute.lichen" ---
ABuf := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
}
k := compute.parallel fill "cpu" ABuf
run := 'n -> {
  d := (compute.plrun k ((Fill){.n n}) : Struct)
  compute.collect d.a : Array
}
run 4
"#;

#[test]
fn probe_a_function_direct_collect() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_DIRECT_COLLECT, None, &mut store) {
        Ok(text) => println!("[function-direct-collect] {text}"),
        Err(diagnostics) => println!(
            "[function-direct-collect] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// Nested typed field filled from a call result.
///
/// # Invariant
/// Print-only; answers 7: Int.
const NESTED_TYPED_FIELD: &str = r#"
In2 := {.a @any : Int}
T := {.b In2}
F := 'x -> T{.b In2{.a x}}
g := ('p = T) -> p.b.a
h := 'x -> {
  d := (F x)
  g (T{.b In2{.a d.b.a}})
}
h 7
"#;

#[test]
fn probe_a_nested_typed_field() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NESTED_TYPED_FIELD, None, &mut store) {
        Ok(text) => println!("[nested-typed-field] {text}"),
        Err(diagnostics) => println!(
            "[nested-typed-field] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The kernel launch's shape without compute.
///
/// # Invariant
/// Print-only; answers 7: Int, so the language apply is not the failing part.
const DECIDED_FIELD_VALUE: &str = r#"
Inner := {.x @any : Int}
mk := 'u -> Inner{.x u}
Outer := {.i (mk 1)}
g := ('p = Outer) -> p.i.x
h := 'n -> {
  d := (mk n)
  g (Outer{.i Inner{.x d.x}})
}
h 7
"#;

#[test]
fn probe_a_decided_field_value() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(DECIDED_FIELD_VALUE, None, &mut store) {
        Ok(text) => println!("[decided-field-value] {text}"),
        Err(diagnostics) => println!("[decided-field-value] refused: {diagnostics:?}"),
    }
}
/// The failing chain with the argument type declared inside the function.
///
/// # Invariant
/// Print-only; answers the buffer values when a function-scoped type repairs it.
const FUNCTION_LOCAL_TYPE: &str = r#"
--- compute = import "compute.lichen" ---
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 4})
  ({.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
}
k := compute.parallel fill "cpu" {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
run := 'n -> {
  ABuf := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
  G := ('p = ABuf) -> p.a
  d := (compute.plrun k ((Fill){.n n}) : Struct)
  G (ABuf{.a d.a})
}
r := run 4
compute.collect r : Array
"#;

#[test]
fn probe_a_function_local_type() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_LOCAL_TYPE, None, &mut store) {
        Ok(text) => println!("[function-local-type] {text}"),
        Err(diagnostics) => println!(
            "[function-local-type] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The chained launch with every type declared inside the function.
///
/// # Invariant
/// Print-only; answers raw none.when a function-scoped type carries them.
const NAMED_CONTRACT_WHOLE_ARG: &str = r#"
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
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
ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 2, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
dp := (compute.plrun p_kernel ps : Struct)
meta := Meta{.n 10, .rank 2, .contract 2, .total 12}
contract := ('arg = Arg) -> ((compute.plrun kernel arg) : Struct)
out := contract ((Arg){.meta meta, .I In{.a data_a.a, .b data_b.b, .plan dp.plan}})
compute.collect out.c : Array
"#;

const NAMED_CONTRACT_HOST_ARG: &str = r#"
--- compute = import "compute.lichen" ---
Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
In   := {.a (compute.Buf (compute.BufParam{.type Float, .length 24})), .b (compute.Buf (compute.BufParam{.type Float, .length 60})), .plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
Out  := {.c (compute.Buf (compute.BufParam{.type Float, .length 10}))}
Par  := {.meta Meta, .in In}
Arg  := 'I -> {.meta Meta, .I I}
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
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
ShapeTable := table{string, Int}
contract := 'abuf -> ('ashape : ShapeTable) -> ('astride : ShapeTable) -> 'bbuf -> ('bshape : ShapeTable) -> ('bstride : ShapeTable) -> {
  e0 := ashape <== "i"
  e1 := ashape <== "l"
  e2 := ashape <== "j"
  e3 := ashape <== "k"
  as0 := astride <== "i"
  bs0 := bstride <== "i"
  as1 := astride <== "l"
  bs1 := bstride <== "l"
  as2 := astride <== "j"
  bs2 := bstride <== "j"
  as3 := astride <== "k"
  bs3 := bstride <== "k"
  ps := PlanShape{.n 12, .s0 as0, .s1 bs0, .s2 e0, .s3 as1, .s4 bs1, .s5 e1, .s6 as2, .s7 bs2, .s8 e2, .s9 as3, .s10 bs3, .s11 e3}
  dp := (compute.plrun p_kernel ps : Struct)
  meta := Meta{.n e0 * e1, .rank 4, .contract 2, .total e2 * e3}
  (compute.plrun kernel ((Arg _){.meta meta, .I In{.a abuf, .b bbuf, .plan dp.plan}}) : Struct)
}
ashape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
astride := table(table{string, Int})("i" => 12, "l" => 0, "j" => 4, "k" => 1)
bshape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
bstride := table(table{string, Int})("i" => 0, "l" => 1, "j" => 20, "k" => 5)
c := contract data_a.a ashape astride data_b.b bshape bstride
compute.collect c.c : Array
"#;

const FUNCTION_LOCAL_CONTRACT: &str = r#"
--- compute = import "compute.lichen" ---
Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
ShapeTable := table{string, Int}
contract := 'abuf -> ('ashape : ShapeTable) -> ('astride : ShapeTable) -> 'bbuf -> ('bshape : ShapeTable) -> ('bstride : ShapeTable) -> {
  In   := {.a (compute.Buf (compute.BufParam{.type Float, .length 24})), .b (compute.Buf (compute.BufParam{.type Float, .length 60})), .plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
  Out  := {.c (compute.Buf (compute.BufParam{.type Float, .length 10}))}
  Par  := {.meta Meta, .in In}
  Arg  := {.meta Meta, .I In}
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
  e0 := ashape <== "i"
  e1 := ashape <== "l"
  e2 := ashape <== "j"
  e3 := ashape <== "k"
  as0 := astride <== "i"
  bs0 := bstride <== "i"
  as1 := astride <== "l"
  bs1 := bstride <== "l"
  as2 := astride <== "j"
  bs2 := bstride <== "j"
  as3 := astride <== "k"
  bs3 := bstride <== "k"
  ps := PlanShape{.n 12, .s0 as0, .s1 bs0, .s2 e0, .s3 as1, .s4 bs1, .s5 e1, .s6 as2, .s7 bs2, .s8 e2, .s9 as3, .s10 bs3, .s11 e3}
  dp := (compute.plrun p_kernel ps : Struct)
  meta := Meta{.n e0 * e1, .rank 4, .contract 2, .total e2 * e3}
  (compute.plrun kernel ((Arg){.meta meta, .I In{.a abuf, .b bbuf, .plan dp.plan}}) : Struct)
}
ashape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
astride := table(table{string, Int})("i" => 12, "l" => 0, "j" => 4, "k" => 1)
bshape := table(table{string, Int})("i" => 2, "l" => 5, "j" => 3, "k" => 4)
bstride := table(table{string, Int})("i" => 0, "l" => 1, "j" => 20, "k" => 5)
c := contract data_a.a ashape astride data_b.b bshape bstride
compute.collect c.c : Array
"#;

const FUNCTION_LOCAL_CHAIN: &str = r#"
--- compute = import "compute.lichen" ---
run := 'n -> {
  ABuf := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
  Buffers := {.a (compute.Buf (compute.BufParam{.type Int, .length 4})), .b (compute.Buf (compute.BufParam{.type Int, .length 4})), .plan (compute.Buf (compute.BufParam{.type Int, .length 4}))}
  Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
  Sum := {.meta Meta, .in Buffers}
  SumOut := {.c (compute.Buf (compute.BufParam{.type Int, .length 4}))}
  Fill := {.n @any : Int}
  fill := ('f = Fill) -> {
    i := compute.range f.n
    a := compute.buf (compute.BufParam{.type Int, .length 4})
    ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
  }
  sum := ('p = Sum) -> {
    i := compute.range p.meta.n
    o := compute.buf (compute.BufParam{.type Int, .length 4})
    ((SumOut){.c (compute.write ((compute.Write _){.to o, .at i, .value (compute.read ((compute.Read _){.from p.in.plan, .at i}) : Int)}))})
  }
  a_kernel := compute.parallel fill "cpu" ABuf
  sum_kernel := compute.parallel sum "cpu" SumOut
  d1 := (compute.plrun a_kernel ((Fill){.n n}) : Struct)
  (compute.plrun sum_kernel ((Sum){.meta Meta{.n 4, .rank 4, .contract 1, .total 2}, .in Buffers{.a d1.a, .b d1.a, .plan d1.a}}) : Struct)
}
r := run 4
compute.collect r.c : Array
"#;

#[test]
fn probe_a_named_contraction_whole_arg() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NAMED_CONTRACT_WHOLE_ARG, None, &mut store) {
        Ok(text) => println!("[named-contraction-whole-arg] {text}"),
        Err(diagnostics) => println!(
            "[named-contraction-whole-arg] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_named_contraction_host_arg() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let program = NAMED_CONTRACT_HOST_ARG
        .replace("@RANK@", "10")
        .replace("@CONTRACT@", "2");
    match lichen_language::run::evaluate_raw(&program, None, &mut store) {
        Ok(text) => println!("[named-contraction-host-arg] {text}"),
        Err(diagnostics) => println!(
            "[named-contraction-host-arg] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_function_local_contract() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_LOCAL_CONTRACT, None, &mut store) {
        Ok(text) => println!("[function-local-contract] {text}"),
        Err(diagnostics) => println!(
            "[function-local-contract] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_function_local_chain() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_LOCAL_CHAIN, None, &mut store) {
        Ok(text) => println!("[function-local-chain] {text}"),
        Err(diagnostics) => println!(
            "[function-local-chain] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The smallest chained launch: the consumer's argument reads the producer's field.
///
/// # Invariant
/// Print-only; answers raw none.
const FUNCTION_CHAIN_MINIMAL: &str = r#"
--- compute = import "compute.lichen" ---
B1 := {.a (compute.Buf (compute.BufParam{.type Int, .length 2}))}
Fill := {.n @any : Int}
Par := {.n @any : Int, .in B1}
Out := {.c (compute.Buf (compute.BufParam{.type Int, .length 2}))}
mk := ('p = Fill) -> {
  i := compute.range p.n
  a := compute.buf (compute.BufParam{.type Int, .length 2})
  ((B1){.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
}
sum := ('p = Par) -> {
  i := compute.range p.n
  o := compute.buf (compute.BufParam{.type Int, .length 2})
  ((Out){.c (compute.write ((compute.Write _){.to o, .at i, .value (compute.read ((compute.Read _){.from p.in.a, .at i}) : Int)}))})
}
k1 := compute.parallel mk "cpu" B1
k2 := compute.parallel sum "cpu" Out
run := 'n -> {
  d := (compute.plrun k1 ((Fill){.n n}) : Struct)
  (compute.plrun k2 ((Par){.n 2, .in B1{.a d.a}}) : Struct)
}
r := run 2
compute.collect r.c : Array
"#;

#[test]
fn probe_a_function_chain_minimal() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_CHAIN_MINIMAL, None, &mut store) {
        Ok(text) => println!("[function-chain-minimal] {text}"),
        Err(diagnostics) => println!(
            "[function-chain-minimal] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_buffer_parameter() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(BUFFER_PARAMETER, None, &mut store) {
        Ok(text) => println!("[buffer-parameter] {text}"),
        Err(diagnostics) => println!(
            "[buffer-parameter] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_two_runs() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(TWO_RUNS, None, &mut store) {
        Ok(text) => println!("[two-runs] {text}"),
        Err(diagnostics) => println!(
            "[two-runs] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// A kernel reading two buffers that arrive as function parameters.
///
/// # Invariant
/// Print-only. The kernel's input buffers are a function's parameters here, which is the
/// contract kernel's shape; a zero answer means a parameter buffer does not reach a kernel.
const PARAMETER_BUFFERS: &str = r#"
--- compute = import "compute.lichen" ---
ABuf := {.a (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Buffers := {.a (compute.Buf (compute.BufParam{.type Int, .length 4})), .b (compute.Buf (compute.BufParam{.type Int, .length 4})), .plan (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
Sum := {.meta Meta, .in Buffers}
SumOut := {.c (compute.Buf (compute.BufParam{.type Int, .length 4}))}
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Int, .length 4})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value (i + 1)}))})
}
sum := ('p = Sum) -> {
  i := compute.range p.meta.n
  o := compute.buf (compute.BufParam{.type Int, .length 4})
  ((SumOut){.c (compute.write ((compute.Write _){.to o, .at i, .value (compute.read ((compute.Read _){.from p.in.plan, .at i}) : Int)}))})
}
a_kernel := compute.parallel fill "cpu" ABuf
sum_kernel := compute.parallel sum "cpu" SumOut
run := 'n -> {
  d1 := (compute.plrun a_kernel ((Fill){.n n}) : Struct)
  (compute.plrun sum_kernel ((Sum){.meta Meta{.n 4, .rank 4, .contract 1, .total 2}, .in Buffers{.a d1.a, .b d1.a, .plan d1.a}}) : Struct)
}
r := run 4
compute.collect r.c : Array
"#;

#[test]
fn probe_a_parameter_buffers() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PARAMETER_BUFFERS, None, &mut store) {
        Ok(text) => println!("[parameter-buffers] {text}"),
        Err(diagnostics) => println!(
            "[parameter-buffers] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_plan_in_literal() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PLAN_IN_LITERAL, None, &mut store) {
        Ok(text) => println!("[plan-in-literal] {text}"),
        Err(diagnostics) => println!(
            "[plan-in-literal] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_plan_in_function() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(PLAN_IN_FUNCTION, None, &mut store) {
        Ok(text) => println!("[plan-in-function] {text}"),
        Err(diagnostics) => println!(
            "[plan-in-function] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_function_run() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(FUNCTION_RUN, None, &mut store) {
        Ok(text) => println!("[function-run] {text}"),
        Err(diagnostics) => println!(
            "[function-run] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_named_contraction() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NAMED_CONTRACT, None, &mut store) {
        Ok(text) => println!("[named-contract] {text}"),
        Err(diagnostics) => println!(
            "[named-contract] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (
                    diagnostic.message.clone(),
                    diagnostic.span,
                    diagnostic
                        .check
                        .as_ref()
                        .map(|check| format!("{:?}", check.kind))
                ))
                .collect::<Vec<_>>()
        ),
    }
}

/// A curried call's result type: the outer apply names the inner function.
///
/// # Invariant
/// Print-only. The field read on the call's result needs the return type the outer apply
/// states, which is the probe of the curried-chain resolution.
const CURRIED_CALL: &str = r#"
f := ('a : Int) -> ('b : Int) -> {.x (a + b)}
r := f 1 2
r.x
"#;

#[test]
fn probe_a_curried_call() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(CURRIED_CALL, None, &mut store) {
        Ok(text) => println!("[curried] {text}"),
        Err(diagnostics) => println!(
            "[curried] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The spec's directed unify on a struct value against its kind.
///
/// # Invariant
/// Print-only. `a = b` is `a <: b`; a struct value's type is `Struct`, so `S = Struct`
/// is the SPEC's type-of rule.
const STRUCT_KIND_UNIFY: &str = r#"
S := {.x 1}
r := (S = Struct)
r
"#;

#[test]
fn probe_a_struct_kind_unify() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(STRUCT_KIND_UNIFY, None, &mut store) {
        Ok(text) => println!("[kind-unify] {text}"),
        Err(diagnostics) => println!(
            "[kind-unify] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

/// The named contract with its body inlined at the top level: the function boundary removed.
///
/// # Invariant
/// Print-only. If this answers the values, the remaining `raw none` is the function return,
/// not the layout.
const NAMED_INLINE: &str = r#"
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
PlanShape := {.n @any : Int, .s0 @any : Int, .s1 @any : Int, .s2 @any : Int, .s3 @any : Int, .s4 @any : Int, .s5 @any : Int, .s6 @any : Int, .s7 @any : Int, .s8 @any : Int, .s9 @any : Int, .s10 @any : Int, .s11 @any : Int}
plan_fill := ('f = PlanShape) -> {
  i := compute.range f.n
  v := (i == 0) * f.s0 + (i == 1) * f.s1 + (i == 2) * f.s2 + (i == 3) * f.s3 + (i == 4) * f.s4 + (i == 5) * f.s5 + (i == 6) * f.s6 + (i == 7) * f.s7 + (i == 8) * f.s8 + (i == 9) * f.s9 + (i == 10) * f.s10 + (i == 11) * f.s11
  plan := compute.buf (compute.BufParam{.type Int, .length 12})
  ((PBuf){.plan (compute.write ((compute.Write _){.to plan, .at i, .value v}))})
}
a_kernel := compute.parallel fill_a "cpu" ABuf
b_kernel := compute.parallel fill_b "cpu" BBuf
p_kernel := compute.parallel plan_fill "cpu" PBuf
data_a := (compute.plrun a_kernel ((Fill){.n 24}) : Struct)
data_b := (compute.plrun b_kernel ((Fill){.n 60}) : Struct)
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
ps := PlanShape{.n 12, .s0 12, .s1 0, .s2 2, .s3 0, .s4 1, .s5 5, .s6 4, .s7 20, .s8 3, .s9 1, .s10 5, .s11 4}
dp := (compute.plrun p_kernel ps : Struct)
meta := Meta{.n 10, .rank 2, .contract 2, .total 12}
out := (compute.plrun kernel ((Arg){.meta meta, .I In{.a data_a.a, .b data_b.b, .plan dp.plan}}) : Struct)
compute.collect out.c : Array
"#;

#[test]
fn probe_a_named_inline() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NAMED_INLINE, None, &mut store) {
        Ok(text) => println!("[named-inline] {text}"),
        Err(diagnostics) => println!(
            "[named-inline] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn probe_a_named_shape_table() {
    let mut store = new_store(None).expect("the embedded compute package registers");
    match lichen_language::run::evaluate_raw(NAMED_TABLE, None, &mut store) {
        Ok(text) => println!("[named] {text}"),
        Err(diagnostics) => println!(
            "[named] refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                .collect::<Vec<_>>()
        ),
    }
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

/// The smallest pure-lichen shapes the struct rewrite's two unifies touch.
///
/// # Invariant
/// Print-only, asserts nothing: each program prints what the spec-conforming
/// implementation answers, so the smallest failing shape is named by a reading.
#[test]
fn probe_a_spec_struct_and_clone() {
    let cases: [(&str, &str); 6] = [
        (
            "decided-field-given-other",
            r#"
Inner := {.x @any : Int}
T := {.a (Inner{.x 1})}
f := 'n -> {
  d := (T{.a (Inner{.x n})})
  d.a.x
}
f 7
"#,
        ),
        (
            "decided-field-given-same",
            r#"
Inner := {.x @any : Int}
T := {.a (Inner{.x 7})}
f := 'n -> {
  d := (T{.a (Inner{.x n})})
  d.a.x
}
f 7
"#,
        ),
        (
            "local-read",
            r#"
T := {.x @any : Int}
mk := 'u -> T{.x u}
f := 'n -> {
  d := (mk n)
  d.x
}
f 7
"#,
        ),
        (
            "literal-from-local",
            r#"
T := {.x @any : Int}
mk := 'u -> T{.x u}
f := 'n -> {
  d := (mk n)
  (T{.x d.x}).x
}
f 7
"#,
        ),
        (
            "nested-local",
            r#"
Inner := {.x @any : Int}
Outer := {.i Inner}
mk := 'n -> Outer{.i Inner{.x n}}
f := 'n -> {
  d := (mk n)
  d.i.x
}
f 7
"#,
        ),
        (
            "two-step",
            r#"
T := {.a @any : Int}
mk := 'u -> T{.a u}
f := 'n -> {
  d := (mk n)
  e := (mk d.a)
  e.a
}
f 7
"#,
        ),
    ];
    for (name, source) in cases {
        let mut store = new_store(None).expect("the embedded compute package registers");
        match lichen_language::run::evaluate_raw(source, None, &mut store) {
            Ok(text) => println!("[spec-struct-clone] {name}: {text}"),
            Err(diagnostics) => println!(
                "[spec-struct-clone] {name}: refused {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    }
}

/// The same mismatch, shrunk: a field whose declared value is a literal.
///
/// # Invariant
/// Print-only: `T{.a 2}` against a declared `.a` of `1` is the shortest shape the
/// struct rewrite's `v[k] = e1` unify can refuse.
#[test]
fn probe_a_spec_struct_minimal_error() {
    let cases: [(&str, &str); 3] = [
        (
            "literal-declared",
            r#"
T := {.a 1}
f := 'n -> (T{.a n}).a
f 2
"#,
        ),
        (
            "struct-declared",
            r#"
Inner := {.x @any : Int}
T := {.a (Inner{.x 1})}
f := 'n -> (T{.a (Inner{.x n})}).a.x
f 7
"#,
        ),
        (
            "struct-declared-same",
            r#"
Inner := {.x @any : Int}
T := {.a (Inner{.x 7})}
f := 'n -> (T{.a (Inner{.x n})}).a.x
f 7
"#,
        ),
    ];
    for (name, source) in cases {
        let mut store = new_store(None).expect("the embedded compute package registers");
        match lichen_language::run::evaluate_raw(source, None, &mut store) {
            Ok(text) => println!("[spec-minimal] {name}: {text}"),
            Err(diagnostics) => println!(
                "[spec-minimal] {name}: refused {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    }
}

/// The Any-typed field's own reading, beside the decided field's refusal.
///
/// # Invariant
/// Print-only: an `@any` field takes the supplied value (`@any` is undecided and always
/// unifies), so only a decided declared value can refuse.
#[test]
fn probe_a_spec_any_field() {
    let cases: [(&str, &str); 3] = [
        (
            "any-field",
            r#"
T := {.a @any : Int}
f := 'n -> (T{.a n}).a
f 2
"#,
        ),
        (
            "any-nested-field",
            r#"
Inner := {.x @any : Int}
T := {.a Inner}
f := 'n -> (T{.a Inner{.x n}}).a.x
f 7
"#,
        ),
        (
            "decided-field",
            r#"
T := {.a 1}
f := 'n -> (T{.a n}).a
f 2
"#,
        ),
    ];
    for (name, source) in cases {
        let mut store = new_store(None).expect("the embedded compute package registers");
        match lichen_language::run::evaluate_raw(source, None, &mut store) {
            Ok(text) => println!("[spec-any] {name}: {text}"),
            Err(diagnostics) => println!(
                "[spec-any] {name}: refused {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    }
}

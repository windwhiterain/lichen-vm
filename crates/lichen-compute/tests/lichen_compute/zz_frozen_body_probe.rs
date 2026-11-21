//! Probe: what a frozen package may hand a kernel — a value's field names, a body
//! function, or a declared kernel.
//!
//! # Invariant
//!
//! Print-only, asserts nothing. A package compiles and runs its own `@loop` kernel while
//! live (`declared`), the importer reads that compiled kernel back (`kernel`), and a
//! program runs it on an argument built through the imported type (`run`; the buffer's
//! perspective comes from `compute.write`). A frozen *function* handed to
//! `compute.parallel` stays refused by name (`program`).

use std::path::PathBuf;

use lichen_compute::language::new_store;

/// A package whose body folds its input and returns a locally allocated output.
const BODY: &str = r#"
---
  compute = import "compute.lichen"
---
In  := {.unused (compute.Buf (compute.BufParam{.type Float, .length 8}))}
Out := {.y (compute.Buf (compute.BufParam{.type Float, .length 2}))}
Par := {.n @any : Int, .taps @any : Int, .in In}
Arg := {.n @any : Int, .taps @any : Int, .I In}
pair := {.row 1, .column 2}
raw := ('c = Par) -> {
  i := compute.range c.n
  o := compute.buf (compute.BufParam{.type Float, .length 2})
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _){.from c.in.unused, .at s(0) - 1}) : Float)))
  ((Out){.y (compute.write ((compute.Write _){.to o, .at i, .value fold (c.taps, 0.0)}))})
}
"#;

/// The same package with the kernel declared beside its body.
const KERNEL: &str = r#"
---
  compute = import "compute.lichen"
---
In  := {.unused (compute.Buf (compute.BufParam{.type Float, .length 8}))}
Out := {.y (compute.Buf (compute.BufParam{.type Float, .length 2}))}
Par := {.n @any : Int, .taps @any : Int, .in In}
Arg := {.n @any : Int, .taps @any : Int, .I In}
Fill := {.n @any : Int}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 8}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 8})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value 1.0}))})
}
f_kernel := compute.parallel fill "cpu" ABuf
data := (compute.plrun f_kernel ((Fill){.n 8}) : Struct)
raw := ('c = Par) -> {
  i := compute.range c.n
  o := compute.buf (compute.BufParam{.type Float, .length 2})
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _){.from c.in.unused, .at s(0) - 1}) : Float)))
  ((Out){.y (compute.write ((compute.Write _){.to o, .at i, .value fold (c.taps, 0.0)}))})
}
k := compute.parallel raw "cpu" Out
out := (compute.plrun k ((Arg){.n 2, .taps 3, .I In{.unused data.a}}) : Struct)
collected := compute.collect out.y : Array
"#;

const PROGRAM: &str = r#"
---
  compute = import "compute.lichen"
  lib = import "frozen_body_lib.lichen"
---
Fill := {.n @any : Int}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 8}))}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 8})
  ((ABuf){.a (compute.write ((compute.Write _){.to a, .at i, .value 1.0}))})
}
f_kernel := compute.parallel fill "cpu" ABuf
data := (compute.plrun f_kernel ((Fill){.n 8}) : Struct)
k := compute.parallel lib.raw "cpu" lib.Out
out := (compute.plrun k ((lib.Arg){.n 2, .taps 3, .I lib.In{.unused data.a}}) : Struct)
compute.collect out.y : Array
"#;

/// The dynamic-rank contraction as a library: types, fills, body and kernel at package level.
const CONTRACT: &str = r#"
---
  compute = import "compute.lichen"
---
Meta := {.n @any : Int, .rank @any : Int, .contract @any : Int, .total @any : Int}
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
"#;

/// The program at `rank` and `contract`, filling the plan buffers first.
fn contract_program(rank: u32, contract: u32) -> String {
    CONTRACT_PROGRAM
        .replace("@RANK@", &rank.to_string())
        .replace("@CONTRACT@", &contract.to_string())
}

/// [`contract_program`]'s source.
const CONTRACT_PROGRAM: &str = r#"
---
  compute = import "compute.lichen"
  lib = import "contract_lib.lichen"
---
Fill := {.n @any : Int}
ABuf := {.a (compute.Buf (compute.BufParam{.type Float, .length 24}))}
BBuf := {.b (compute.Buf (compute.BufParam{.type Float, .length 60}))}
PBuf := {.plan (compute.Buf (compute.BufParam{.type Int, .length 12}))}
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
contract := ('arg = lib.Arg) -> ((compute.plrun lib.kernel arg) : Struct)
out := contract ((lib.Arg){.meta lib.Meta{.n 10, .rank @RANK@, .contract @CONTRACT@, .total 12}, .I lib.In{.a data_a.a, .b data_b.b, .plan data_p.plan}})
compute.collect out.c : Array
"#;

macro_rules! reading {
    ($value:expr) => {
        match $value {
            Ok(text) => format!("answered {text}"),
            Err(diagnostics) => format!(
                "refused {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| (diagnostic.message.clone(), diagnostic.span))
                    .collect::<Vec<_>>()
            ),
        }
    };
}

#[test]
#[ignore = "probe: prints and asserts nothing; run with --ignored"]
fn probe_a_kernel_body_from_a_frozen_package() {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("frozen_body_probe");
    std::fs::create_dir_all(&directory).expect("the probe's directory is writable");
    for (name, source) in [
        ("frozen_body_lib.lichen", BODY),
        ("kernel_lib.lichen", KERNEL),
        ("contract_lib.lichen", CONTRACT),
    ] {
        std::fs::write(directory.join(name), source).expect("the probe's library is writable");
    }
    let mut store = new_store(None).expect("the embedded compute package registers");
    let library = directory.join("frozen_body_lib.lichen");
    match store.load_package(&library) {
        Ok(handle) => println!(
            "[frozen] body library loaded: {} export(s), key {:?}",
            handle.direct.len(),
            handle.key
        ),
        Err(diagnostics) => {
            println!(
                "[frozen] body library refused: {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect::<Vec<_>>()
            );
            return;
        }
    }
    for (label, source) in [
        (
            "field",
            "---\n  lib = import \"frozen_body_lib.lichen\"\n---\nlib.pair.row",
        ),
        (
            "value",
            "---\n  lib = import \"frozen_body_lib.lichen\"\n---\nlib.pair",
        ),
        (
            "body",
            "---\n  lib = import \"frozen_body_lib.lichen\"\n---\nlib.raw",
        ),
        ("program", PROGRAM),
        ("declared", KERNEL),
        (
            "kernel",
            "---\n  compute = import \"compute.lichen\"\n  lib = import \"kernel_lib.lichen\"\n---\nlib.k",
        ),
        (
            "kernel-type",
            "---
  compute = import \"compute.lichen\"
  lib = import \"kernel_lib.lichen\"
---
lib.Out",
        ),
        (
            "kernel-function",
            "---
  compute = import \"compute.lichen\"
  lib = import \"kernel_lib.lichen\"
---
lib.fill",
        ),
        (
            "answer",
            "---\n  compute = import \"compute.lichen\"\n  lib = import \"kernel_lib.lichen\"\n---\nlib.out",
        ),
        (
            "run",
            r#"---
  compute = import "compute.lichen"
  lib = import "kernel_lib.lichen"
---
Fill := {.n @any : Int}
fill := ('f = Fill) -> {
  i := compute.range f.n
  a := compute.buf (compute.BufParam{.type Float, .length 8})
  ((lib.In){.unused (compute.write ((compute.Write _){.to a, .at i, .value 2.0}))})
}
f_kernel := compute.parallel fill "cpu" lib.In
data := (compute.plrun f_kernel ((Fill){.n 8}) : Struct)
out := (compute.plrun lib.k ((lib.Arg){.n 2, .taps 3, .I data}) : Struct)
compute.collect out.y : Array
"#,
        ),
    ] {
        println!(
            "[frozen] {label}: {}",
            reading!(lichen_language::run::evaluate_raw(
                source,
                Some(&directory),
                &mut store
            ))
        );
    }
    for (label, source) in [
        ("contract", contract_program(2, 2)),
        ("contract3", contract_program(3, 1)),
    ] {
        println!(
            "[frozen] {label}: {}",
            reading!(lichen_language::run::evaluate_raw(
                &source,
                Some(&directory),
                &mut store
            ))
        );
    }
}

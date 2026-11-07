//! The direct contraction: two host buffers of different lengths through one prepared step.
//!
//! # Invariant
//!
//! `A` is 4x2 and `B` is 2x3 (lengths 8 and 6, which the language's own parameter cannot
//! state); the contraction over `c0`/`r1` answers `A·B` as `C{r0,c1}`:
//! `[5, 8, 11, 11, 18, 25, 17, 28, 39, 23, 38, 53]`. The step's free values are the
//! `GArg`'s `.in`'s `.a` and `.b` in declaration order; the shape-table leaves are computed at
//! check time.

use std::process::ExitCode;

use lichen_compute::compute::ResidentBuffer;
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

const SOURCE: &str = r#"
--- compute = import "compute.lichen" ---
In  := {.a (compute.Buf (compute.BufParam{.type Float, .length 8})), .b (compute.Buf (compute.BufParam{.type Float, .length 6}))}
Out := {.c (compute.Buf (compute.BufParam{.type Float, .length 12}))}
Meta := {.n @any : Int, .k @any : Int, .c1 @any : Int, .ar @any : Int, .ac @any : Int, .br @any : Int, .bc @any : Int}
Par  := {.meta Meta, .in In}
Arg  := {.meta Meta, .I In}
GArg := {.in In}
body := ('p = Par) -> {
  i := compute.range p.meta.n
  row := i / p.meta.c1
  col := i - row * p.meta.c1
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + ((compute.read ((compute.Read _){.from p.in.a, .at (row * p.meta.ar + (s(0) - 1) * p.meta.ac)}) : Float) * (compute.read ((compute.Read _){.from p.in.b, .at ((s(0) - 1) * p.meta.br + col * p.meta.bc)}) : Float))))
  o := compute.buf (compute.BufParam{.type Float, .length p.meta.n})
  ((Out){.c (compute.write ((compute.Write _){.to o, .at i, .value (fold (p.meta.k, 0.0))}))})
}
kernel := compute.parallel body "gpu" Out
a_shape := table(table{string, Int})("r0" => 4, "c0" => 2)
b_shape := table(table{string, Int})("r1" => 2, "c1" => 3)
c_shape := a_shape @table_merge b_shape @table_without "c0" @table_without "r1"
step := 's = GArg -> {
  meta := Meta{.n ((c_shape <== "r0") * (c_shape <== "c1")), .k (a_shape <== "c0"), .c1 (c_shape <== "c1"), .ar (a_shape <== "c0"), .ac 1, .br (b_shape <== "c1"), .bc 1}
  out := (compute.plrun kernel ((Arg){.meta meta, .I In{.a s.in.a, .b s.in.b}}) : Struct)
  out.c
}
built := compute.graph step
built
"#;

/// A `Float` buffer argument of `values`.
fn floats(values: &[f32]) -> RunArgument {
    RunArgument::Buffer {
        class: ScalarClass::Float,
        data: values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect(),
    }
}

/// The one resident a single-return step answered with.
fn only(stepped: &lichen_serve::Stepped) -> Result<ResidentBuffer, String> {
    match &stepped.results[..] {
        [RunResult::Resident(resident)] => Ok(*resident),
        _ => {
            Err("the contraction answers with one buffer, and this step answered otherwise".into())
        }
    }
}

/// Hand `A` and `B` to the prepared step and read `C` back.
fn measure(session: &Session) -> Result<(), String> {
    let prepared = session
        .prepare(SOURCE)
        .map_err(|refusal| format!("the contraction did not prepare: {refusal}"))?;
    println!("the held step is {} dispatches", prepared.nodes());
    let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
    let b = [1.0, 2.0, 3.0, 2.0, 3.0, 4.0];
    let stepped = prepared
        .step(vec![floats(&a), floats(&b)])
        .map_err(|refusal| format!("the contraction did not run: {refusal}"))?;
    let resident = only(&stepped)?;
    let answered = match session.fetch(resident) {
        Ok(ScalarData::Float(values)) => values,
        Ok(other) => return Err(format!("the answer declared a Float buffer: {other:?}")),
        Err(refusal) => return Err(format!("the answer is not readable: {refusal}")),
    };
    println!("C = {answered:?}");
    session.release(resident);
    Ok(())
}

fn main() -> ExitCode {
    match Session::open() {
        Ok(session) => match measure(&session) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Err(refusal) => {
            eprintln!("this reading needs a device: {refusal}");
            ExitCode::FAILURE
        }
    }
}

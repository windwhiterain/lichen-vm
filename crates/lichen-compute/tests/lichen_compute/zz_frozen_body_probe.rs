//! Probe: what a frozen package may hand a kernel — a value's field names, a body
//! function, or a declared kernel.
//!
//! # Invariant
//!
//! Print-only, asserts nothing. The library is a package, so its functions are frozen
//! before the program names them; the readings are where a package's kernel meets the
//! check (`docs/notes/general-contract-handoff.md`).

use std::path::PathBuf;

use lichen_compute::language::new_store;

/// A package with a named field value and a body function carrying a `@loop`.
const BODY: &str = r#"
---
  compute = import "compute.lichen"
---
In  := {.unused (compute.Buf (compute.BufParam{.type Float, .length _}))}
Out := {.y (compute.Buf (compute.BufParam{.type Float, .length _}))}
Par := {.n @any : Int, .taps @any : Int, .in In, .out Out}
Arg := {.n @any : Int, .taps @any : Int, .I In}
pair := {.row 1, .column 2}
raw := ('c = Par) -> {
  i := compute.range c.n
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _){.from c.in.unused, .at s(0) - 1}) : Float)))
  compute.write ((compute.Write _){.to c.out.y, .at i, .value fold (c.taps, 0.0)})
}
"#;

/// The same package with the kernel declared beside its body.
const KERNEL: &str = r#"
---
  compute = import "compute.lichen"
---
In  := {.unused (compute.Buf (compute.BufParam{.type Float, .length _}))}
Out := {.y (compute.Buf (compute.BufParam{.type Float, .length _}))}
Par := {.n @any : Int, .taps @any : Int, .in In, .out Out}
Arg := {.n @any : Int, .taps @any : Int, .I In}
raw := ('c = Par) -> {
  i := compute.range c.n
  @loop fold := 's -> if s(0) == 0 then s(1) else fold (s(0) - 1, (s(1) + (compute.read ((compute.Read _){.from c.in.unused, .at s(0) - 1}) : Float)))
  compute.write ((compute.Write _){.to c.out.y, .at i, .value fold (c.taps, 0.0)})
}
k := compute.parallel raw "cpu" Out
"#;

const PROGRAM: &str = r#"
---
  compute = import "compute.lichen"
  lib = import "frozen_body_lib.lichen"
---
x := (compute.buffer ([@any : Float, 2][1.0, 1.0] : Array) : (compute.Buf (compute.BufParam{.type Float, .length _})))
k := compute.parallel lib.raw "cpu" lib.Out
out := (compute.plrun k ((lib.Arg){.n 2, .taps 3, .I lib.In{.unused x}}) : Struct)
compute.collect out.y : Array
"#;

macro_rules! reading {
    ($value:expr) => {
        match $value {
            Ok(text) => format!("answered {text}"),
            Err(diagnostics) => format!(
                "refused {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.clone())
                    .collect::<Vec<_>>()
            ),
        }
    };
}
#[test]
fn probe_a_kernel_body_from_a_frozen_package() {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("frozen_body_probe");
    std::fs::create_dir_all(&directory).expect("the probe's directory is writable");
    for (name, source) in [
        ("frozen_body_lib.lichen", BODY),
        ("kernel_lib.lichen", KERNEL),
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
    // A package that declares the kernel itself.
    match lichen_language::run::evaluate_raw(
        "---\n  compute = import \"compute.lichen\"\n  lib = import \"kernel_lib.lichen\"\n---\nlib.k",
        Some(&directory),
        &mut store,
    ) {
        Ok(text) => println!("[frozen] kernel value: answered {text}"),
        Err(diagnostics) => println!(
            "[frozen] kernel value: refused {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    }
}

//! Probe: how many store registrations and module drops a process survives, and where it
//! stops.
//!
//! # Invariant
//!
//! The markers are written to a file rather than to stderr, because a stack overflow aborts
//! the process and loses the tail of its output. Nothing here asserts: the run's own limits
//! are the reading.

use lichen_compute::language::new_store;

use super::common;

/// Append one line to the probe's log; a fresh handle per line flushes it.
fn mark(what: &str) {
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("C:/resource/lichen-vm/.scratch-core/phases.log")
    {
        let _ = writeln!(file, "{what}");
    }
}

#[test]
fn a_store_registration_and_a_module_drop_repeat_where_the_peel_breaks_them() {
    let source = r#"
---
  compute = import "compute.lichen" ---
k = compute.jit (x : Int => int2float (x + 1))
compute.launch k 5
"#;
    let parallel = r#"
--- compute = import "compute.lichen" ---
In  = struct{.a Int}
Out = struct{.z (compute.Buf _)}
Par = compute.P (compute.KT _)(.I In, .O Out)
f = (k : Par) => {
  i = compute.range k.n
  compute.write ((compute.Write _)(.to k.out.z, .at i, .value float2int (int2float i + 0.5)))
}
k = compute.parallel f "BACKEND" Out
out = (compute.plrun k ((compute.A In)(.n 69, .I In(.a 0))) : Out)
((compute.read ((compute.Read _)(.from out.z, .at 0)) : Float), (compute.read ((compute.Read _)(.from out.z, .at 1)) : Float), (compute.read ((compute.Read _)(.from out.z, .at 68)) : Float), compute.collect out.z)
"#;
    mark("first program start");
    let (first_module, first_value, _first_ty) = common::run(source);
    mark("first program done");
    drop(first_module);
    drop(first_value);
    mark("first program dropped");
    let _installed = common::device_slot();
    mark("cpu start");
    let (cpu_module, cpu_value, _) = common::run(&parallel.replace("BACKEND", "cpu"));
    mark("cpu done");
    match lichen_compute_gpu::install_default() {
        Ok(()) => {
            mark("gpu installed");
            let (gpu_module, gpu_value, _) = common::run(&parallel.replace("BACKEND", "gpu"));
            mark("gpu values kept");
            mark("gpu done");
            lichen_compute_gpu::uninstall();
            mark("gpu uninstalled");
            drop(gpu_module);
            mark("gpu module dropped");
        }
        Err(reason) => mark(&format!("no device: {reason}")),
    }
    mark("cpu values kept");
    drop(cpu_module);
    mark("cpu module dropped");
    for round in 0..6 {
        mark(&format!("round {round} store"));
        let store = new_store(None);
        mark(&format!("round {round} store done {}", store.is_ok()));
        drop(store);
        mark(&format!("round {round} store dropped"));
        let (module, value, _) = common::run(source);
        mark(&format!(
            "round {round} run done {:?}",
            common::float_of(&value)
        ));
        drop(module);
        mark(&format!("round {round} module dropped"));
    }
}

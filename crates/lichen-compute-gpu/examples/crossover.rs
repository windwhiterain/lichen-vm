//! Where the GPU stops being a pessimisation.
//!
//! `plrun` stays on the CPU thread pool below a count threshold, for the same
//! reason it does: a fan-out has to pay for itself.  That reason is *stronger*
//! for a GPU, not weaker — a dispatch costs a pipeline bind, a descriptor set, a
//! submit and a fence wait, against a thread spawn — so the threshold here is a
//! measured number rather than a guess.
//!
//! The CPU side of the comparison is a plain sequential scalar loop standing in
//! for the thread pool's single-worker path, which makes this a *conservative*
//! crossover: the real CPU path fans out over threads above 4096 indices, so
//! the true crossover is at least this high.
//!
//! ```text
//! cargo run --release -p lichen-compute-gpu --example crossover
//! ```

use std::time::Instant;

use lichen_compute_gpu::{GpuContext, LOCAL_SIZE_X};
use lichen_kernel_ir::{IntWidth, KernelBin, KernelFragment, KernelInstr, KernelShape};

/// `out[i] = in[i] + in[i] + 1` — enough arithmetic that the run is not purely
/// launch overhead, and the same shape the acceptance tests use.
fn fragment() -> KernelFragment {
    KernelFragment {
        param_shape: KernelShape::Tuple(vec![KernelShape::Scalar, KernelShape::Scalar]),
        body: vec![
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall,
            KernelInstr::Const(0),
            KernelInstr::LocalGet(1),
            KernelInstr::BufferReadCall,
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::Const(1),
            KernelInstr::Bin(KernelBin::Add),
            KernelInstr::BufferWriteCall,
            KernelInstr::Const(0),
        ],
        outputs: 1,
        results: 1,
        int_width: IntWidth::I64,
    }
}

/// The same computation, scalar, on this thread.
fn sequential(input: &[i64], output: &mut [i64]) {
    for (index, value) in input.iter().enumerate() {
        output[index] = value + value + 1;
    }
}

fn main() {
    let Ok(context) = GpuContext::new() else {
        eprintln!("no usable Vulkan device; nothing to compare against");
        std::process::exit(1);
    };
    println!("device: {}", context.device_name());
    println!("workgroup: {LOCAL_SIZE_X} invocations\n");
    println!(
        "{:>10}  {:>12}  {:>12}  {:>8}",
        "count", "gpu (ms)", "sequential (ms)", "ratio"
    );

    for count in [
        64usize, 256, 1_024, 4_096, 16_384, 65_536, 262_144, 1_048_576,
    ] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let mut expected = vec![0i64; count];
        sequential(&input, &mut expected);

        // A warm-up run, so the first timed run is not paying for pipeline
        // creation — the pipeline cache makes the second run of a fragment free.
        context
            .run(&fragment(), &[input.clone()], count)
            .expect("the warm-up run completes");

        let started = Instant::now();
        let from_gpu = context
            .run(&fragment(), &[input.clone()], count)
            .expect("the timed run completes");
        let gpu = started.elapsed();

        let mut produced = vec![0i64; count];
        let started = Instant::now();
        sequential(&input, &mut produced);
        let cpu = started.elapsed();

        assert_eq!(from_gpu[0], expected, "the GPU result must still be right");

        println!(
            "{count:>10}  {:>12.3}  {:>12.3}  {:>7.2}x",
            gpu.as_secs_f64() * 1e3,
            cpu.as_secs_f64() * 1e3,
            cpu.as_secs_f64() / gpu.as_secs_f64().max(f64::MIN_POSITIVE),
        );
    }
}

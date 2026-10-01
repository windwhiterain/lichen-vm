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
use lichen_kernel_ir::BufferSlot;
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

/// The fixed cost of a dispatch, with the answer stated as a distribution.
///
/// A single sample of this is worth very little: on a machine that is not
/// otherwise idle the same binary measures an empty dispatch anywhere from
/// 0.3 ms to 0.9 ms, which is a 3x spread and wide enough to hide the effect of
/// removing object churn. The **minimum** is the estimator that answers "what
/// does this cost when nothing else is interfering", and the median is printed
/// beside it so the spread stays visible rather than being quietly optimised away
/// by the choice of statistic.
fn fixed_cost(context: &GpuContext) -> (f64, f64) {
    const REPEATS: usize = 200;
    let count = 64usize;
    let input: Vec<i64> = (0..count as i64).collect();
    let warm = context
        .run(&fragment(), &[BufferSlot::Host(&input)], count)
        .expect("the warm-up run completes");
    for id in &warm {
        context.release(*id);
    }

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let resident = context
            .run(&fragment(), &[BufferSlot::Host(&input)], count)
            .expect("a probe run completes");
        let elapsed = started.elapsed().as_secs_f64() * 1e3;
        for id in &resident {
            context.release(*id);
        }
        samples.push(elapsed);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("the samples are all finite"));
    (samples[0], samples[REPEATS / 2])
}

/// The fixed cost of a dispatch that consumes a **resident** buffer, which is
/// what every link of a chain after the first one is.
///
/// This is a different number from [`fixed_cost`] and the difference is the
/// point. A host input is `memcpy`'d into staging and copied by the device
/// inside the same submission; a chain link pays neither. Dividing a chain's
/// marginal link by the host-input floor therefore credits the link with an
/// upload it never did, and at small counts that credit comes to more than the
/// link costs — which is how the first run of this table reported a link that
/// was 108% overhead, which is not a number any fraction can be.
fn resident_cost(context: &GpuContext) -> (f64, f64) {
    const REPEATS: usize = 200;
    let count = 64usize;
    let input: Vec<i64> = (0..count as i64).collect();
    let seed = context
        .run(&fragment(), &[BufferSlot::Host(&input)], count)
        .expect("the seeding run completes");

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let resident = context
            .run(&fragment(), &[BufferSlot::Resident(seed[0])], count)
            .expect("a probe run completes");
        let elapsed = started.elapsed().as_secs_f64() * 1e3;
        for id in &resident {
            context.release(*id);
        }
        samples.push(elapsed);
    }
    context.release(seed[0]);
    samples.sort_by(|a, b| a.partial_cmp(b).expect("the samples are all finite"));
    (samples[0], samples[REPEATS / 2])
}

/// One chain of `links` dispatches over a single uploaded buffer, in
/// milliseconds, with the trailing readback counted.
///
/// The readback is counted deliberately: a chain a program actually runs ends in
/// a value somebody wanted, so leaving it out would flatter every number
/// derived from this.
fn time_chain(context: &GpuContext, input: &[i64], count: usize, links: usize) -> f64 {
    let started = Instant::now();
    let mut current = context
        .run(&fragment(), &[BufferSlot::Host(input)], count)
        .expect("the chain's first link uploads");
    for _ in 1..links {
        // Each link is handed the previous one's id and never sees its data.
        let next = context
            .run(&fragment(), &[BufferSlot::Resident(current[0])], count)
            .expect("a link consumes the previous link's id");
        context.release(current[0]);
        current = next;
    }
    let _answer = context
        .fetch(current[0], count)
        .expect("the last link comes home");
    let elapsed = started.elapsed().as_secs_f64() * 1e3;
    context.release(current[0]);
    elapsed
}

fn main() {
    let Ok(context) = GpuContext::new() else {
        eprintln!("no usable Vulkan device; nothing to compare against");
        std::process::exit(1);
    };
    println!("device: {}", context.device_name());
    println!("workgroup: {LOCAL_SIZE_X} invocations");
    let (best, median) = fixed_cost(&context);
    println!("empty dispatch over 200 runs: best {best:.3} ms, median {median:.3} ms\n");
    let (link_floor, link_median) = resident_cost(&context);
    println!(
        "resident-input dispatch over 200 runs: best {link_floor:.3} ms, median {link_median:.3} ms\n"
    );
    println!(
        "{:>10}  {:>12}  {:>12}  {:>12}  {:>8}",
        "count", "gpu (ms)", "dispatch", "fetch", "ratio"
    );

    for count in [
        64usize, 256, 1_024, 4_096, 16_384, 65_536, 262_144, 1_048_576,
    ] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let mut expected = vec![0i64; count];
        sequential(&input, &mut expected);

        // A warm-up run, so the first timed run is not paying for pipeline
        // creation — the pipeline cache makes the second run of a fragment free.
        // Its result is released rather than kept: the ids are what a program
        // releases, and this example is also where that path gets exercised.
        let warm = context
            .run(&fragment(), &[BufferSlot::Host(&input)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        // Timed as two steps, because they are two costs: the dispatch uploads and
        // runs, and the fetch brings the answer home.  A program that chains
        // kernels pays the first and not the second.
        let started = Instant::now();
        let resident = context
            .run(&fragment(), &[BufferSlot::Host(&input)], count)
            .expect("the timed run completes");
        let dispatch = started.elapsed();

        let started = Instant::now();
        let from_gpu = context
            .fetch(resident[0], count)
            .expect("the result comes back off the device");
        let fetch = started.elapsed();
        for id in &resident {
            context.release(*id);
        }
        let gpu = dispatch + fetch;

        let mut produced = vec![0i64; count];
        let started = Instant::now();
        sequential(&input, &mut produced);
        let cpu = started.elapsed();

        assert_eq!(from_gpu, expected, "the GPU result must still be right");

        println!(
            "{count:>10}  {:>12.3}  {:>12.3}  {:>10.3}  {:>7.2}x",
            gpu.as_secs_f64() * 1e3,
            dispatch.as_secs_f64() * 1e3,
            fetch.as_secs_f64() * 1e3,
            cpu.as_secs_f64() / gpu.as_secs_f64().max(f64::MIN_POSITIVE),
        );
    }

    // The number that decides whether this backend is worth anything: a *chain* of
    // dispatches over one resident buffer, against the same chain run as scalar
    // passes. The first dispatch uploads and the last fetch downloads; everything
    // between them is resident, so a chain of N costs two transfers and N runs
    // rather than 2N.
    println!("\nresident chain — one upload, N dispatches, one download");
    println!(
        "{:>10}  {:>8}  {:>12}  {:>12}  {:>8}",
        "count", "chain", "gpu (ms)", "cpu (ms)", "ratio"
    );

    for count in [65_536usize, 262_144, 1_048_576] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let warm = context
            .run(&fragment(), &[BufferSlot::Host(&input)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        for links in [1usize, 2, 4, 8, 16] {
            let started = Instant::now();
            let mut current = context
                .run(&fragment(), &[BufferSlot::Host(&input)], count)
                .expect("the chain's first link uploads");
            for _ in 1..links {
                // Each link is handed the previous one's id and never sees its data.
                let next = context
                    .run(&fragment(), &[BufferSlot::Resident(current[0])], count)
                    .expect("a link consumes the previous link's id");
                context.release(current[0]);
                current = next;
            }
            let _last = context
                .fetch(current[0], count)
                .expect("the last link comes home");
            let gpu = started.elapsed();
            context.release(current[0]);

            // Two buffers swapped per link, so the CPU side is not paying for a
            // fresh multi-megabyte allocation per link — that would be measuring
            // the allocator rather than the loop, and it shows up as an outlier
            // big enough to invent a crossover that is not there.
            let mut front = input.clone();
            let mut back = vec![0i64; count];
            let started = Instant::now();
            for _ in 0..links {
                sequential(&front, &mut back);
                std::mem::swap(&mut front, &mut back);
            }
            let cpu = started.elapsed();

            println!(
                "{count:>10}  {links:>8}  {:>12.3}  {:>12.3}  {:>7.2}x",
                gpu.as_secs_f64() * 1e3,
                cpu.as_secs_f64() * 1e3,
                cpu.as_secs_f64() / gpu.as_secs_f64().max(f64::MIN_POSITIVE),
            );
        }
    }

    // The question the table above cannot answer: how much of a chain is the
    // per-dispatch overhead rather than the kernel. It decides whether a graph
    // is worth building at all, so it is worth a table of its own.
    //
    // It has to be a **difference of two chain lengths**, not one chain minus a
    // constant. The upload and the download cost the same at both lengths, so
    // subtracting cancels them and what survives is the marginal cost of a
    // link. Subtracting the floor from a single chain would leave the transfers
    // in the answer, and a graph cannot remove a transfer.
    //
    // Best of several runs for the reason `fixed_cost` is a best: a single
    // sample of a chain on a machine that is not otherwise idle lands over a
    // millisecond apart, which is wider than the effect being measured.
    const REPEATS: usize = 20;
    const LINKS: usize = 16;

    println!("\nper-dispatch overhead against kernel size — {LINKS} links, best of {REPEATS}");
    println!(
        "{:>10}  {:>10}  {:>10}  {:>12}  {:>14}  {:>11}",
        "count", "1 link", "16 links", "per link", "fusion share", "overhead"
    );

    for count in [1_024usize, 4_096, 16_384, 65_536, 262_144, 1_048_576] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let warm = context
            .run(&fragment(), &[BufferSlot::Host(&input)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        let mut single = f64::MAX;
        let mut chained = f64::MAX;
        for _ in 0..REPEATS {
            single = single.min(time_chain(&context, &input, count, 1));
            chained = chained.min(time_chain(&context, &input, count, LINKS));
        }

        // A per-link cost at or below zero would mean the long chain beat the
        // short one, which is noise rather than a result, and dividing by it
        // would invert every column to its right. Clamped, so that case reads
        // as "no signal here" instead of as a spectacular win.
        let per_link = ((chained - single) / (LINKS - 1) as f64).max(f64::MIN_POSITIVE);
        let saved = (LINKS - 1) as f64 * link_floor;
        println!(
            "{count:>10}  {single:>10.3}  {chained:>10.3}  {per_link:>12.3}  {:>13.1}%  {:>10.1}%",
            saved / chained * 100.0,
            link_floor / per_link * 100.0,
        );
    }

    // The last two columns answer different questions and both are needed.
    //
    // **fusion share** is `15 x link floor / 16 links`: the speed-up a program
    // running a 16-link chain would get from having those fifteen submissions
    // removed. It is a *ceiling* — it assumes every submit and every wait but
    // the last disappears, which is the Batch schedule. Async keeps the submits
    // and removes only the waits, so it collects part of this and not all of it;
    // how much is the submit/wait split below, and that is not measured yet.
    //
    // **overhead** is `link floor / per link`: what fraction of one marginal
    // link is spent not doing the work. This is the diagnostic, and it is the
    // column that moves with count. The two differ because the first divides by
    // a total that includes the upload and the download, and the second does
    // not.
    //
    // Both divide by `per link`, which is a difference of two chain lengths. A
    // negative one would mean the long chain beat the short one, which is noise
    // rather than a result, so it is clamped above and that case reads as no
    // signal rather than as a spectacular win.
    println!(
        "\nthe floor used above is {link_floor:.3} ms — a dispatch that reads a resident\n\
         buffer, not the {best:.3} ms host-input one, because a chain link after the first\n\
         uploads nothing. It is still an upper bound: an empty dispatch's fence wait has no\n\
         device work to hide behind, so a link that does work waits for less."
    );
}

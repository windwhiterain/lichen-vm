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
use lichen_kernel_ir::{
    FlatOp, IntWidth, KernelBin, KernelBody, KernelFragment, KernelInstr, KernelRoles, KernelShape,
    LaunchSet, Pending, ScalarClass, ScalarData,
};

/// A one-fragment launch set: this example measures one kernel at a time.
fn only(fragment: &KernelFragment) -> LaunchSet<'_> {
    LaunchSet::single(fragment)
}

/// The **packed** bytes of `words`, one `i64` each — the host payload an integer
/// fragment reads.  Packed rather than reinterpreted, because the ABI's element
/// width is the class's ([`ScalarClass::byte_width`]) and not `i64`'s.
fn pack(words: &[i64]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// A fetched payload as the `i64` elements these integer fragments produce.
fn words(data: ScalarData) -> Vec<i64> {
    match data {
        ScalarData::Int(elements) => elements,
        ScalarData::Float(elements) => {
            panic!(
                "an integer run's result came back as {} float element(s)",
                elements.len()
            )
        }
    }
}

/// `out[i] = in[i] + in[i] + 1` — enough arithmetic that the run is not purely
/// launch overhead, and the same shape the acceptance tests use.
fn fragment() -> KernelFragment {
    KernelFragment {
        roles: KernelRoles::default(),
        param_shape: KernelShape::Tuple(vec![
            KernelShape::Scalar(ScalarClass::Int),
            KernelShape::Scalar(ScalarClass::Int),
        ]),
        body: KernelBody::from_flat(
            2,
            &[
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
                FlatOp::Read(1),
                FlatOp::Instr(KernelInstr::BufferReadCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 1)),
                FlatOp::Instr(KernelInstr::Bin(ScalarClass::Int, KernelBin::Add)),
                FlatOp::Instr(KernelInstr::BufferWriteCall(ScalarClass::Int)),
                FlatOp::Instr(KernelInstr::Const(ScalarClass::Int, 0)),
            ],
        ),
        inputs: 1,
        outputs: 1,
        input_classes: vec![ScalarClass::Int],
        output_classes: vec![ScalarClass::Int],
        result_classes: vec![ScalarClass::Int],
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
    let packed = pack(&input);
    let warm = context
        .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
        .expect("the warm-up run completes");
    for id in &warm {
        context.release(*id);
    }

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let resident = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
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
    let packed = pack(&input);
    let seed = context
        .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
        .expect("the seeding run completes");

    let mut samples = Vec::with_capacity(REPEATS);
    for _ in 0..REPEATS {
        let started = Instant::now();
        let resident = context
            .run(&only(&fragment()), &[BufferSlot::Resident(seed[0])], count)
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

/// One chain of `links` dispatches, each its own submission, in milliseconds
/// with the trailing readback counted.
fn time_chain(context: &GpuContext, input: &[u8], count: usize, links: usize) -> f64 {
    let started = Instant::now();
    let mut current = context
        .run(&only(&fragment()), &[BufferSlot::Host(input)], count)
        .expect("the chain's first link uploads");
    for _ in 1..links {
        // Each link is handed the previous one's id and never sees its data.
        let next = context
            .run(
                &only(&fragment()),
                &[BufferSlot::Resident(current[0])],
                count,
            )
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

/// One fused chain of `links` dispatches in a single submission, in
/// milliseconds, with the trailing readback counted so the two are comparable.
fn time_fused_chain(context: &GpuContext, input: &[u8], count: usize, links: usize) -> f64 {
    let started = Instant::now();
    let id = context
        .run_chain(&only(&fragment()), input, count, links)
        .expect("the fused chain records");
    let _answer = context
        .fetch(id, count)
        .expect("the fused chain comes home");
    let elapsed = started.elapsed().as_secs_f64() * 1e3;
    context.release(id);
    elapsed
}

/// A fused chain's answer, checked against the kernel's own closed form.
///
/// The kernel is `out[i] = 2*in[i] + 1`, so `n` links give `2^n * in[i] +
/// (2^n - 1)`. That is **derived, not run** — which is the whole point. Checking
/// a fused chain against a second CPU copy of the same loop would agree with a
/// mis-ordered chain just as cheerfully, because both would apply the links in
/// the same wrong order. The closed form has no order to get wrong.
///
/// This is also the first execution of the two rules that until now existed only
/// as comments: one descriptor set per dispatch with the pool reset outside the
/// submission, and a trailing barrier naming a shader as well as a transfer. A
/// chain with either of them wrong reads undefined data, so this is the check
/// that says whether they are right.
fn check_fused(context: &GpuContext, count: usize, links: usize) {
    let input: Vec<i64> = (0..count as i64).collect();
    let id = context
        .run_chain(&only(&fragment()), &pack(&input), count, links)
        .expect("the chain under test records");
    let answer = words(
        context
            .fetch(id, count)
            .expect("the chain under test comes home"),
    );
    context.release(id);

    let factor = 1i64 << links;
    for (index, value) in answer.iter().enumerate() {
        let expected = factor * index as i64 + (factor - 1);
        assert_eq!(
            *value, expected,
            "a fused chain of {links} over {count} element(s) is wrong at index {index}"
        );
    }
}

/// Which side of the wait the host work goes on.
///
/// The two schedules run **the same submission and the same host work**; the
/// only difference is whether the host is busy while the device is. So the
/// difference in `wait` between them is everything that overlapping is worth,
/// and nothing else is in the answer — which is the only way to make the number
/// mean something. Any of it that came from the work itself, or from the
/// submission being different, would show up in both columns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlap {
    /// Before the wait, which is the schedule Async exists for.
    BeforeWait,
    /// After the wait, which is what the same work costs with nothing to hide
    /// behind.
    AfterWait,
}

/// One submission timed in three pieces, in milliseconds.
#[derive(Clone, Copy)]
struct Split {
    /// Getting it onto the queue: staging, recording, `queue_submit`.
    submit: f64,
    /// Blocking on the fence.
    wait: f64,
    /// The host work, wherever it was put.
    host: f64,
}

/// A CPU pass over `data`, repeated `passes` times.
///
/// This stands in for the one thing in a graph run the device cannot do: host
/// code touching every element, which is exactly what a node written in lichen
/// rather than compiled to a kernel is. The `black_box` is load-bearing — a
/// pass whose stores are provably dead would be deleted, and the measurement
/// would be of nothing at all while still printing a number.
fn host_passes(data: &mut [i64], passes: usize) {
    for _ in 0..passes {
        for value in data.iter_mut() {
            *value = value.wrapping_mul(2).wrapping_add(1);
        }
    }
    std::hint::black_box(data);
}

/// One submission, with `passes` host passes on one side of its wait or the
/// other.
fn time_split(
    context: &GpuContext,
    input: &[u8],
    count: usize,
    overlap: Overlap,
    host: &mut [i64],
    passes: usize,
) -> Split {
    let millis = |elapsed: std::time::Duration| elapsed.as_secs_f64() * 1e3;
    let started = Instant::now();
    let pending = context
        .submit(&only(&fragment()), &[BufferSlot::Host(input)], count)
        .expect("the run is recorded and handed to the queue");
    let submitted = Instant::now();

    // The passes run on both sides even at zero, so the two schedules do
    // exactly the same work and differ only in where the wait falls.
    if overlap == Overlap::BeforeWait {
        host_passes(host, passes);
    }
    let worked = Instant::now();
    let ids = Box::new(pending).wait().expect("the submission finishes");
    let waited = Instant::now();
    if overlap == Overlap::AfterWait {
        host_passes(host, passes);
    }
    let after = Instant::now();

    let split = match overlap {
        Overlap::BeforeWait => Split {
            submit: millis(submitted - started),
            host: millis(worked - submitted),
            wait: millis(waited - worked),
        },
        Overlap::AfterWait => Split {
            submit: millis(submitted - started),
            wait: millis(waited - submitted),
            host: millis(after - waited),
        },
    };
    context.release(ids[0]);
    split
}

/// The single fastest run of [`time_split`], on each side of the wait.
///
/// The three pieces are reported from **one** run, not as three independent
/// minima, and the reason is not tidiness. They are a decomposition of one
/// elapsed time, so the best wait and the best host work generally come from
/// different runs — and pairing the shortest wait with the shortest host work
/// measures an overlap that never happened. Reported that way the hidden column
/// came out *larger* than the host work it is supposed to be hiding, which is
/// the tell: a hidden amount cannot exceed the work that hid it.
fn best_split(
    context: &GpuContext,
    input: &[u8],
    count: usize,
    host: &mut [i64],
    passes: usize,
    repeats: usize,
) -> (Split, Split) {
    let fastest = |current: &Option<Split>, sample: Split| match current {
        Some(best) if total(best) <= total(&sample) => *best,
        _ => sample,
    };
    let mut before = None;
    let mut after = None;
    for _ in 0..repeats {
        before = Some(fastest(
            &before,
            time_split(context, input, count, Overlap::BeforeWait, host, passes),
        ));
        after = Some(fastest(
            &after,
            time_split(context, input, count, Overlap::AfterWait, host, passes),
        ));
    }
    (
        before.unwrap_or_else(|| unreachable!("repeats is positive")),
        after.unwrap_or_else(|| unreachable!("repeats is positive")),
    )
}

/// The elapsed time one [`Split`] decomposes.
fn total(split: &Split) -> f64 {
    split.submit + split.host + split.wait
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
        "resident-input dispatch over 200 runs: best {link_floor:.3} ms, median {link_median:.3} ms"
    );
    println!("\nchecking a fused chain against the kernel's closed form...");
    for (count, links) in [
        (1_024usize, 16usize),
        (16_384, 16),
        (65_536, 2),
        (1_048_576, 1),
    ] {
        check_fused(&context, count, links);
        println!("  {count} element(s), {links} link(s): correct");
    }
    println!();
    println!(
        "{:>10}  {:>12}  {:>12}  {:>12}  {:>8}",
        "count", "gpu (ms)", "dispatch", "fetch", "ratio"
    );

    for count in [
        64usize, 256, 1_024, 4_096, 16_384, 65_536, 262_144, 1_048_576,
    ] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let packed = pack(&input);
        let mut expected = vec![0i64; count];
        sequential(&input, &mut expected);

        // A warm-up run, so the first timed run is not paying for pipeline
        // creation — the pipeline cache makes the second run of a fragment free.
        // Its result is released rather than kept: the ids are what a program
        // releases, and this example is also where that path gets exercised.
        let warm = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        // Timed as two steps, because they are two costs: the dispatch uploads and
        // runs, and the fetch brings the answer home.  A program that chains
        // kernels pays the first and not the second.
        let started = Instant::now();
        let resident = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
            .expect("the timed run completes");
        let dispatch = started.elapsed();

        let started = Instant::now();
        let from_gpu = words(
            context
                .fetch(resident[0], count)
                .expect("the result comes back off the device"),
        );
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
        let packed = pack(&input);
        let warm = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        for links in [1usize, 2, 4, 8, 16] {
            let started = Instant::now();
            let mut current = context
                .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
                .expect("the chain's first link uploads");
            for _ in 1..links {
                // Each link is handed the previous one's id and never sees its data.
                let next = context
                    .run(
                        &only(&fragment()),
                        &[BufferSlot::Resident(current[0])],
                        count,
                    )
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

    println!(
        "\nper-dispatch overhead, and what fusing a chain actually buys — {LINKS} links, best of {REPEATS}"
    );
    println!(
        "{:>10}  {:>9}  {:>9}  {:>9}  {:>10}  {:>9}  {:>11}  {:>8}",
        "count", "1 link", "serial", "fused", "per link", "overhead", "predicted", "actual"
    );

    for count in [1_024usize, 4_096, 16_384, 65_536, 262_144, 1_048_576] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let packed = pack(&input);
        let warm = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        let mut single = f64::MAX;
        let mut serial = f64::MAX;
        let mut fused = f64::MAX;
        for _ in 0..REPEATS {
            single = single.min(time_chain(&context, &packed, count, 1));
            serial = serial.min(time_chain(&context, &packed, count, LINKS));
            fused = fused.min(time_fused_chain(&context, &packed, count, LINKS));
        }

        // A per-link cost at or below zero would mean the long chain beat the
        // short one, which is noise rather than a result, and dividing by it
        // would invert every column to its right. Clamped, so that case reads
        // as "no signal here" instead of as a spectacular win.
        let per_link = ((serial - single) / (LINKS - 1) as f64).max(f64::MIN_POSITIVE);
        let removable = (LINKS - 1) as f64 * link_floor;
        println!(
            "{count:>10}  {single:>9.3}  {serial:>9.3}  {fused:>9.3}  {per_link:>10.3}  {:>8.1}%  {:>10.1}%  {:>7.1}%",
            link_floor / per_link * 100.0,
            removable / serial * 100.0,
            (serial - fused) / serial * 100.0,
        );
    }

    // **predicted** is `15 x link floor / serial`: the share of the serial chain
    // that the fifteen submissions and waits it does not need should be worth.
    // **actual** is `(serial - fused) / serial`: the share of it that fusing
    // really removed. Putting them side by side is the point of the table --
    // predicted is arithmetic done before the feature existed, actual is the
    // feature, and the gap between them is what the fused path costs or saves
    // beyond removing submissions: the extra barriers, the descriptor set per
    // link, and the pool's larger footprint.
    //
    // Two cautions on reading it. *predicted* is a ceiling — it assumes every
    // submit and every wait but the last disappears, which is exactly what
    // `run_chain` does, so the two should land close; a schedule that kept the
    // submits (Async) would collect less and this table would not describe it.
    // And *actual* cannot exceed 100% but can go negative, which would mean
    // fusing made it slower, not that it went fast enough to be free.
    println!(
        "\nthe floor used above is {link_floor:.3} ms — a dispatch that reads a resident\n\
         buffer, not the {best:.3} ms host-input one, because a chain link after the first\n\
         uploads nothing. It is still an upper bound: an empty dispatch's fence wait has no\n\
         device work to hide behind, so a link that does work waits for less."
    );

    // **What the Async schedule is actually worth.** The tables above are Batch:
    // they say what disappears when fifteen submits *and* fifteen waits go. Async
    // keeps the submits and removes only the waits, and only where something
    // needs the data, so it collects a different and smaller number — and the
    // only way to that number is to split a submission in half and measure each
    // half against a controlled amount of host work in the gap.
    //
    // Each row is the same submission and the same host work twice, once with
    // the work before the wait and once after it. Nothing else differs, so the
    // difference in `wait` is the whole of what overlapping bought. The
    // prediction is `min(host, wait after)`: the wait cannot be cut below zero,
    // and it cannot be cut by more than the device was busy for.
    const SPLIT_REPEATS: usize = 20;
    println!("\nsubmit and wait, split, with host work in the gap — best of {SPLIT_REPEATS}");
    println!(
        "{:>10}  {:>7}  {:>9}  {:>9}  {:>11}  {:>11}  {:>9}  {:>10}",
        "count", "passes", "host", "submit", "wait after", "wait before", "hidden", "predicted"
    );

    for count in [262_144usize, 1_048_576] {
        let input: Vec<i64> = (0..count).map(|value| value as i64).collect();
        // The host work is over the same number of elements the kernel is, so
        // one pass is the same order of magnitude as the dispatch it would sit
        // beside. That is not a tuning choice: it is what a graph looks like when
        // one node is compiled and the next is written in lichen.
        let mut host: Vec<i64> = (0..count).map(|value| value as i64).collect();
        let packed = pack(&input);
        let warm = context
            .run(&only(&fragment()), &[BufferSlot::Host(&packed)], count)
            .expect("the warm-up run completes");
        for id in &warm {
            context.release(*id);
        }

        for passes in [0usize, 1, 2, 4, 8] {
            let (before, after) =
                best_split(&context, &packed, count, &mut host, passes, SPLIT_REPEATS);
            // Reported from the *after* schedule: with nothing to hide behind,
            // that wait is the device's time, and it is the ceiling on how much
            // the other one can be cut by.  The host figure is the *before*
            // schedule's, because that is the work doing the hiding.
            let hidden = (after.wait - before.wait).max(0.0);
            println!(
                "{count:>10}  {passes:>7}  {:>9.3}  {:>9.3}  {:>11.3}  {:>11.3}  {:>9.3}  {:>10.3}",
                before.host,
                after.submit,
                after.wait,
                before.wait,
                hidden,
                before.host.min(after.wait),
            );
        }
    }

    println!(
        "\n*hidden* is `wait after - wait before` and *predicted* is `min(host, wait after)`:\n\
         a wait cannot be cut below zero and cannot be cut by more than the device was\n\
         busy for. The host column is the host time **of the run that was selected for\n\
         its total**, so it jitters -- selecting on the total picks the run that hid the\n\
         most, and that run's host work was not necessarily a typical one. It does not\n\
         matter for the check, because both columns come from the same run.\n\
         \n\
         The submit column is worth a second look and is **not** a constant: flat at a\n\
         quarter of a million elements, and rather more than doubled at a million, where\n\
         a pass over the data is big enough to evict the input the next submit has to\n\
         copy into staging. It is non-monotonic row to row as well, so read it as a cost\n\
         the schedule pays rather than as a number to plan against -- and note that Async\n\
         keeps every submit, so whatever it grows into comes straight off what the\n\
         overlap wins."
    );
}

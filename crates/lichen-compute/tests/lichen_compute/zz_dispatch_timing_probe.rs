//! Probe: what one dispatch costs, body-only against a nine-tap fold.
//!
//! # Invariant
//!
//! Print-only. One annotated loop dispatches the same kernel `steps` times with the state threaded as
//! the kernel's buffer, so `steps = 1` against `steps = 101` leaves a hundred dispatches with the
//! compile paid by both. Two kernels, each lifted verbatim from the example that
//! compiles it: the three-read stencil and the nine-tap convolution, so the difference is a fold.

use super::common;

use lichen_compute::language::new_store;

/// Repetitions per case, so a reading comes with a spread rather than a single run.
const REPEATS: usize = 3;

/// The three-read stencil, from `examples/stencil.lichen`.
const STENCIL: &str = r#"
XIn  = struct{.x (compute.Buf Float)}
XOut = struct{.y (compute.Buf Float)}
XPar = struct{.n Int, .alpha Float, .in XIn, .out XOut}
XArg = struct{.n Int, .alpha Float, .I XIn}
stencil_raw = (c : XPar) => {
  i = compute.range c.n
  centre = compute.read ((compute.Read _)(.from c.in.x, .at i))
  left = compute.read ((compute.Read _)(.from c.in.x, .at (i - 1)))
  right = compute.read ((compute.Read _)(.from c.in.x, .at (i + 1)))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))))
}
one = compute.parallel stencil_raw "@BACKEND@"
"#;

/// The nine-tap convolution, from `examples/filter.lichen`.
const FILTER: &str = r#"
FIn  = struct{.x (compute.Buf Float), .w (compute.Buf Float)}
FOut = struct{.y (compute.Buf Float)}
FPar = struct{.n Int, .taps Int, .in FIn, .out FOut}
FArg = struct{.n Int, .taps Int, .I FIn}
filter_raw = (c : FPar) => {
  i = compute.range c.n
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((s(1) + (compute.read ((compute.Read _)(.from c.in.w, .at (s(0) - 1))) * compute.read ((compute.Read _)(.from c.in.x, .at (i - (s(0) - 1)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, 0.0 ~ "identity"))))
}
one = compute.parallel (filter_raw ~ "claim:Reduction(combine,identity,element)") "@BACKEND@"
"#;

/// The nine-tap window max, from `examples/pooling.lichen`.
const POOL: &str = r#"
PIn  = struct{.x (compute.Buf Float)}
POut = struct{.y (compute.Buf Float)}
PPar = struct{.n Int, .cols Int, .stride Int, .taps Int, .least Float, .in PIn, .out POut}
PArg = struct{.n Int, .cols Int, .stride Int, .taps Int, .least Float, .I PIn}
pool_raw = (c : PPar) => {
  i = compute.range c.n
  row = compute.row c
  col = compute.col c
  @loop fold = s => if s(0) == 0 then s(1) else fold (s(0) - 1, ((max s(1) (compute.read ((compute.Read _)(.from c.in.x, .at ((row + (s(0) - 1) / 3) * c.stride + (col + (s(0) - 1) % 3)))))) ~ "combine" ~ "element"))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (fold (c.taps, c.least ~ "identity"))))
}
one = compute.parallel (pool_raw ~ "claim:Reduction(combine,identity,element)") "@BACKEND@"
"#;

/// One program: `steps` dispatches of `kernel` over `lanes` elements on `backend`.
///
/// # Invariant
/// The first dispatch seeds the loop and every further one comes from the loop's body, so the trace is
/// `1 + steps` dispatches and the seed is counted in both measurements.
fn program(backend: &str, lanes: usize, steps: usize, kernel: &str) -> String {
    // The pooling window reads three rows of a padded grid, so its buffer is three strides long.
    let buffer_length = match kernel {
        "pool" => 3 * (lanes + 2),
        _ => lanes,
    };
    let data = (0..buffer_length)
        .map(|index| if index % 17 == 0 { "4.0" } else { "0.0" })
        .collect::<Vec<_>>()
        .join(", ");
    let (declarations, weights) = match kernel {
        "filter" => (
            FILTER,
            "w = (compute.buffer ([1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0] : array{Float, 9}) : (compute.Buf Float))",
        ),
        "pool" => (POOL, ""),
        _ => (STENCIL, ""),
    };
    let (arg_name, out_name) = match kernel {
        "filter" => ("FArg", "FOut"),
        "pool" => ("PArg", "POut"),
        _ => ("XArg", "XOut"),
    };
    let stride = lanes + 2;
    let seeded = match kernel {
        "filter" => format!(".n {lanes}, .taps 9, .I FIn(.x x, .w w)"),
        "pool" => format!(
            ".n {lanes}, .cols {lanes}, .stride {stride}, .taps 9, .least (0.0 - 100.0), .I PIn(.x x)"
        ),
        _ => format!(".n {lanes}, .alpha 0.25, .I XIn(.x x)"),
    };
    let threaded = match kernel {
        "filter" => format!(".n {lanes}, .taps 9, .I FIn(.x s(1), .w w)"),
        "pool" => format!(
            ".n {lanes}, .cols {lanes}, .stride {stride}, .taps 9, .least (0.0 - 100.0), .I PIn(.x s(1))"
        ),
        _ => format!(".n {lanes}, .alpha 0.25, .I XIn(.x s(1))"),
    };
    format!(
        "--- compute = import \"compute.lichen\" ---\n{declarations}\n{weights}\nx = (compute.buffer ([{data}] : array{{Float, {buffer_length}}}) : (compute.Buf Float))\nseed = (compute.plrun one (({arg_name})({seeded})) : {out_name}).y\n@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun one (({arg_name})({threaded})) : {out_name}).y)) : tuple{{Int, compute.Buf Float}} -> compute.Buf Float)\ncompute.collect (march ({steps}, seed)) : array{{Float, {lanes}}}\n"
    )
    .replace("@BACKEND@", backend)
}

/// One run's wall clock in milliseconds, and what it answered.
fn timed(source: &str) -> (f64, String) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let started = std::time::Instant::now();
    let answer = lichen_language::run::evaluate_raw(source, None, &mut store);
    let elapsed = started.elapsed().as_secs_f64() * 1.0e3;
    let rendered = match answer {
        Ok(text) => text,
        Err(diagnostics) => format!(
            "refused: {:?}",
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone())
                .collect::<Vec<_>>()
        ),
    };
    (elapsed, rendered)
}

#[test]
fn probe_the_dispatch_cost() {
    let Some((_context, _guard)) = common::device("zz_dispatch_timing_probe") else {
        return;
    };
    // One warm-up run, so the first measurement is not the driver's first.
    let _ = timed(&program("gpu", 6, 1, "stencil"));
    for backend in ["cpu", "gpu"] {
        for lanes in [6usize, 1024] {
            for kernel in ["stencil", "filter", "pool"] {
                let mut samples = Vec::new();
                let mut answer = String::new();
                for _ in 0..REPEATS {
                    let (once, _) = timed(&program(backend, lanes, 1, kernel));
                    let (hundred, rendered) = timed(&program(backend, lanes, 101, kernel));
                    samples.push((hundred - once) / 100.0);
                    answer = rendered;
                }
                samples.sort_by(|left, right| {
                    left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal)
                });
                println!(
                    "[{backend}/lanes={lanes}/kernel={kernel}] {REPEATS} runs: per extra dispatch \
                     median {:.4} ms, min {:.4}, max {:.4}, answer {}",
                    samples[samples.len() / 2],
                    samples[0],
                    samples[samples.len() - 1],
                    answer
                        .rsplit_once(": ")
                        .map_or(answer.as_str(), |(value, _)| value)
                );
            }
        }
    }
}

//! Probe: what one stencil step costs per dispatch, and what the loop's state costs.
//!
//! # Invariant
//!
//! Print-only. One annotated loop dispatches the same kernel `steps` times, so `steps = 1` against
//! `steps = 101` leaves a hundred dispatches with the compile paid by both. Two states are timed: the
//! kernel's own buffer, which needs no `collect` inside the step, and an `array{Float, N}` rebuilt
//! with `compute.buffer`, which collects every iteration. Their difference is the readback.

use super::common;

use std::time::Instant;

use lichen_compute::language::new_store;

/// One program: `steps` dispatches of a stencil over `lanes` elements on `backend`, threading `shape`.
///
/// # Invariant
/// The first dispatch seeds the loop and every further one comes from the loop's body, so the trace is
/// `1 + steps` dispatches in both shapes and the seed is counted in both measurements.
fn program(backend: &str, lanes: usize, steps: usize, shape: &str) -> String {
    let data = (0..lanes)
        .map(|index| if index % 17 == 0 { "4.0" } else { "0.0" })
        .collect::<Vec<_>>()
        .join(", ");
    let (seed, looping, answer) = match shape {
        "buffer" => (
            format!(
                "seed = (compute.plrun one ((XArg)(.n {lanes}, .alpha 0.25, .I XIn(.x x))) : XOut).y"
            ),
            format!(
                "@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.plrun one ((XArg)(.n {lanes}, .alpha 0.25, .I XIn(.x s(1)))) : XOut).y)) : tuple{{Int, compute.Buf Float}} -> compute.Buf Float)"
            ),
            format!("compute.collect (march ({steps}, seed)) : array{{Float, {lanes}}}"),
        ),
        _ => (
            format!(
                "seed = (compute.collect (compute.plrun one ((XArg)(.n {lanes}, .alpha 0.25, .I XIn(.x x))) : XOut).y : array{{Float, {lanes}}})"
            ),
            format!(
                "@loop march = ((s => if s(0) == 0 then s(1) else march (s(0) - 1, (compute.collect (compute.plrun one ((XArg)(.n {lanes}, .alpha 0.25, .I XIn(.x (compute.buffer (s(1)) : (compute.Buf Float))))) : XOut).y : array{{Float, {lanes}}}))) : tuple{{Int, array{{Float, {lanes}}}}} -> array{{Float, {lanes}}})"
            ),
            format!("march ({steps}, seed)"),
        ),
    };
    let template = r#"
--- compute = import "compute.lichen" ---
XIn  = struct{.x (compute.Buf Float)}
XOut = struct{.y (compute.Buf Float)}
XPar = struct{.n Int, .alpha Float, .in XIn, .out XOut}
XArg = struct{.n Int, .alpha Float, .I XIn}
step_raw = (c : XPar) => {
  i = compute.range c.n
  centre = compute.read ((compute.Read _)(.from c.in.x, .at i))
  left = compute.read ((compute.Read _)(.from c.in.x, .at (i - 1)))
  right = compute.read ((compute.Read _)(.from c.in.x, .at (i + 1)))
  compute.write ((compute.Write _)(.to c.out.y, .at i, .value (centre + c.alpha * ((left + right) - 2.0 * centre))))
}
one = compute.parallel step_raw "@BACKEND@"
x = (compute.buffer ([@DATA@] : array{Float, @LANES@}) : (compute.Buf Float))
@SEED@
@LOOP@
@ANSWER@
"#;
    template
        .replace("@BACKEND@", backend)
        .replace("@DATA@", &data)
        .replace("@LANES@", &lanes.to_string())
        .replace("@SEED@", &seed)
        .replace("@LOOP@", &looping)
        .replace("@ANSWER@", &answer)
}

/// One run's wall clock in milliseconds, and what it answered.
fn timed(source: &str) -> (f64, String) {
    let mut store = new_store(None).expect("the embedded compute package registers");
    let started = Instant::now();
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
    let _ = timed(&program("gpu", 6, 1, "buffer"));
    for backend in ["cpu", "gpu"] {
        for lanes in [6usize, 1024] {
            for shape in ["buffer", "array"] {
                let (once, _) = timed(&program(backend, lanes, 1, shape));
                let (hundred, answer) = timed(&program(backend, lanes, 101, shape));
                println!(
                    "[{backend}/lanes={lanes}/state={shape}] steps=1 {once:.3} ms, steps=101 \
                     {hundred:.3} ms, per extra dispatch {:.4} ms, answer {}",
                    (hundred - once) / 100.0,
                    answer
                        .rsplit_once(": ")
                        .map_or(answer.as_str(), |(value, _)| value)
                );
            }
        }
    }
}

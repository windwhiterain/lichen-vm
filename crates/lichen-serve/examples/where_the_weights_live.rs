//! What a step costs with a layer's weights in host memory and with them on the device.

use std::process::ExitCode;
use std::time::Instant;

use lichen_serve::{Batched, Qwen, Sampling, Session, Width};

/// The wall clock of one step of one request.
fn step_once(model: &Qwen<'_>) -> Result<std::time::Duration, lichen_serve::QwenRefusal> {
    let mut batched = Batched::new(model);
    let id = batched.admit(&[9707], 4, Sampling::greedy(), 0)?;
    let started = Instant::now();
    batched.step()?;
    let elapsed = started.elapsed();
    let _ = batched.answer(id);
    Ok(elapsed)
}

/// One step's wall clock, and what the device reported for it.
///
/// # Invariant
/// The two runs measure the same program over the same shapes, so the only difference between them is where
/// the layer bytes live: a resident weight is read by the device, a host-visible one crosses the bus.
fn measure(session: &Session) -> Result<(), String> {
    let mut model = Qwen::open_with(session, Width::Narrowed)
        .map_err(|refusal| format!("the narrowed model did not open: {refusal}"))?;
    model
        .hold_in_memory()
        .map_err(|refusal| format!("the layers are not held: {refusal}"))?;
    let held =
        step_once(&model).map_err(|refusal| format!("the held step did not run: {refusal}"))?;
    println!("held in host memory: {:.3} s", held.as_secs_f64());
    model
        .reside(28)
        .map_err(|refusal| format!("the layers do not fit: {refusal}"))?;
    let resident =
        step_once(&model).map_err(|refusal| format!("the resident step did not run: {refusal}"))?;
    println!("resident on the device: {:.3} s", resident.as_secs_f64());
    let before = model.stage_nanos();
    let wall = step_once(&model).map_err(|refusal| format!("the step did not run: {refusal}"))?;
    let stages = std::time::Duration::from_nanos(model.stage_nanos() - before);
    println!(
        "a step is {:.3} s, its stages {:.3} s, so {:.3} s is around them",
        wall.as_secs_f64(),
        stages.as_secs_f64(),
        wall.as_secs_f64() - stages.as_secs_f64()
    );
    println!(
        "the bus costs {:.2}x",
        held.as_secs_f64() / resident.as_secs_f64()
    );
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

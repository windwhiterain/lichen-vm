//! What a step costs with a layer's weights in host memory and with them on the device.

use std::time::Instant;

use lichen_serve::{Batched, Qwen, Sampling, Session, Width};

/// One step's wall clock, and what the device reported for it.
///
/// # Invariant
/// The two runs measure the same program over the same shapes, so the only difference between them is where
/// the layer bytes live: a resident weight is read by the device, a host-visible one crosses the bus.
#[test]
fn a_step_costs_what_its_weights_are_near() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_step_costs_what_its_weights_are_near: no device");
        return;
    };
    let mut model = match Qwen::open_with(&session, Width::Narrowed) {
        Ok(model) => model,
        Err(refusal) => {
            eprintln!("SKIPPED: {refusal}");
            return;
        }
    };
    if let Err(refusal) = model.hold_in_memory() {
        eprintln!("SKIPPED: the layers are not held: {refusal}");
        return;
    }
    let held = step_once(&model).unwrap_or_else(|refusal| panic!("the held step runs: {refusal}"));
    println!("held in host memory: {:.3} s", held.as_secs_f64());
    if let Err(refusal) = model.reside(28) {
        eprintln!("SKIPPED: the layers do not fit: {refusal}");
        return;
    }
    let resident =
        step_once(&model).unwrap_or_else(|refusal| panic!("the resident step runs: {refusal}"));
    println!("resident on the device: {:.3} s", resident.as_secs_f64());
    println!(
        "the bus costs {:.2}x",
        held.as_secs_f64() / resident.as_secs_f64()
    );
}

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

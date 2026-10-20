//! The layer's live path, one verified step at a time, so a wrong stage names itself.

mod common;

use std::path::PathBuf;

use common::{floats, read};
use lichen_compute::compute::graph::{RunArgument, RunResult};
use lichen_serve::{HIDDEN, MATVEC_STEP, RMS_NORM_STEP, Session};

/// The token's row and the shape of the head blocks.
const ROW: &str = "row9707";
const HEAD: usize = 128;
const KEY_HEADS: usize = 8;

/// One of the reference's f32 tensors, or `None` when the model is not on this machine.
fn tensor(name: &str) -> Option<Vec<f32>> {
    let root = match std::env::var_os("LICHEN_MODEL_DIR") {
        Some(found) => PathBuf::from(found),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.model"),
    };
    let bytes = std::fs::read(root.join(format!("layer0_{name}.bin"))).ok()?;
    Some(
        bytes
            .chunks_exact(4)
            .map(|word| f32::from_le_bytes(word.try_into().unwrap_or_default()))
            .collect(),
    )
}

/// The one resident a step answered with, read and released.
fn answered(session: &Session, stepped: &lichen_serve::Stepped) -> Vec<f32> {
    let [RunResult::Resident(resident)] = &stepped.results[..] else {
        panic!("a step answers with one buffer, and this one answered otherwise");
    };
    let values = read(session, *resident);
    session.release(*resident);
    values
}

/// Each stage of the live path is compared with the host's own number for that stage.
///
/// # Invariant
/// The stages are the ones the layer's answer depends on at one position — the first norm, the value
/// projection, the grouped spread, the output projection and the residual — so a stage that answers
/// the host's number leaves only the wiring between stages able to explain a wrong layer.
#[test]
fn each_stage_of_the_live_path_answers_the_host() {
    let Some(row) = tensor(ROW) else {
        eprintln!("SKIPPED each_stage_of_the_live_path_answers_the_host: no model files");
        return;
    };
    let (Some(gain), Some(value_weight), Some(out_weight)) =
        (tensor("wi"), tensor("wv"), tensor("wo"))
    else {
        eprintln!("SKIPPED each_stage_of_the_live_path_answers_the_host: a tensor is missing");
        return;
    };
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED each_stage_of_the_live_path_answers_the_host: no device");
        return;
    };

    let norm = session
        .prepare(RMS_NORM_STEP)
        .unwrap_or_else(|refusal| panic!("the norm prepares: {refusal}"));
    let stepped = norm
        .step(vec![
            RunArgument::Count(HIDDEN as i64),
            RunArgument::Count(1),
            RunArgument::Count(HIDDEN as i64),
            floats(&row),
            floats(&gain),
        ])
        .unwrap_or_else(|refusal| panic!("the norm runs: {refusal}"));
    let normed = answered(&session, &stepped);
    let wanted = [-0.023084412_f32, 0.026700413, -0.07286645];
    for (index, expected) in wanted.iter().enumerate() {
        println!("n1[{index}] = {} (host {expected})", normed[index]);
        assert!(
            (normed[index] - expected).abs() < 1.0e-5,
            "the first norm differs at {index}"
        );
    }

    let projection = session
        .prepare(MATVEC_STEP)
        .unwrap_or_else(|refusal| panic!("the projection prepares: {refusal}"));
    let stepped = projection
        .step(vec![
            RunArgument::Count((KEY_HEADS * HEAD) as i64),
            RunArgument::Count(HIDDEN as i64),
            floats(&normed),
            floats(&value_weight),
        ])
        .unwrap_or_else(|refusal| panic!("the value projection runs: {refusal}"));
    let value = answered(&session, &stepped);
    let wanted = [-0.170916010_f32, 0.149055877, 0.339529740];
    for (index, expected) in wanted.iter().enumerate() {
        println!("v[{index}] = {} (host {expected})", value[index]);
        assert!(
            (value[index] - expected).abs() < 1.0e-5,
            "the value projection differs at {index}"
        );
    }

    let attended: Vec<f32> = (0..HIDDEN)
        .map(|i| value[(i / HEAD / 2) * HEAD + i % HEAD])
        .collect();
    println!("attended[0] = {} (host -0.170916010)", attended[0]);

    let stepped = projection
        .step(vec![
            RunArgument::Count(HIDDEN as i64),
            RunArgument::Count(HIDDEN as i64),
            floats(&attended),
            floats(&out_weight),
        ])
        .unwrap_or_else(|refusal| panic!("the output projection runs: {refusal}"));
    let out = answered(&session, &stepped);
    let wanted = [-0.251894822_f32, -0.223910645, 0.224302041];
    for (index, expected) in wanted.iter().enumerate() {
        println!("o[{index}] = {} (host {expected})", out[index]);
        assert!(
            (out[index] - expected).abs() < 1.0e-5,
            "the output projection differs at {index}"
        );
    }

    let hidden: Vec<f32> = row
        .iter()
        .zip(&out)
        .map(|(left, right)| left + right)
        .collect();
    let wanted = [-0.263125291_f32, -0.214328125, 0.194761025];
    for (index, expected) in wanted.iter().enumerate() {
        println!("h[{index}] = {} (host {expected})", hidden[index]);
        assert!(
            (hidden[index] - expected).abs() < 1.0e-5,
            "the residual differs at {index}"
        );
    }
    println!("every live stage answers the host's number");
}

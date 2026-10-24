//! Which of the serving programs prepare, and why the others do not.
//!
//! The model's steps are prepared once at load, so a program the surface refuses stops the
//! service before its first token. This prints one line a step: the node count when it prepares,
//! the refusal when it does not.

use lichen_serve::{
    ATTEND_CACHE_STEP, ATTEND_STEP, BLOCK, DECODE_STEP, LAYER_STEP, MATVEC_BATCH_STEP,
    MATVEC_ROW_STEP, MATVEC_STEP, PREFILL_STEP, RMS_NORM_STEP, ROPE_STEP, SAMPLE_STEP, SWIGLU_STEP,
};

fn main() {
    let session = match lichen_serve::Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let steps: [(&str, &str); 13] = [
        ("RMS_NORM_STEP", RMS_NORM_STEP),
        ("MATVEC_STEP", MATVEC_STEP),
        ("MATVEC_BATCH_STEP", MATVEC_BATCH_STEP),
        ("MATVEC_ROW_STEP", MATVEC_ROW_STEP),
        ("ROPE_STEP", ROPE_STEP),
        ("ATTEND_STEP", ATTEND_STEP),
        ("ATTEND_CACHE_STEP", ATTEND_CACHE_STEP),
        ("SWIGLU_STEP", SWIGLU_STEP),
        ("DECODE_STEP", DECODE_STEP),
        ("PREFILL_STEP", PREFILL_STEP),
        ("SAMPLE_STEP", SAMPLE_STEP),
        ("LAYER_STEP", LAYER_STEP),
        ("BLOCK", BLOCK),
    ];
    for (name, source) in steps {
        match session.prepare(source) {
            Ok(prepared) => println!("{name}: {} node(s)", prepared.nodes()),
            Err(refusal) => println!("{name}: {refusal}"),
        }
    }
}

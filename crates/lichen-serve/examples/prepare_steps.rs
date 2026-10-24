//! One line a serving step: its node count, or the refusal that stopped it.

use lichen_serve::{
    ATTEND_CACHE_STEP, ATTEND_ROWS_STEP, ATTEND_STEP, BLOCK, DECODE_STEP, LAYER_STEP,
    MATVEC_BATCH_STEP, MATVEC_ROW_STEP, MATVEC_STEP, PRE_STEP, PREFILL_STEP, RMS_NORM_STEP,
    ROPE_ROWS_STEP, ROPE_STEP, SAMPLE_STEP, SWIGLU_STEP,
};

fn main() {
    let session = match lichen_serve::Session::open() {
        Ok(session) => session,
        Err(refusal) => {
            eprintln!("{refusal}");
            std::process::exit(1);
        }
    };
    let steps: [(&str, &str); 16] = [
        ("RMS_NORM_STEP", RMS_NORM_STEP),
        ("MATVEC_STEP", MATVEC_STEP),
        ("MATVEC_BATCH_STEP", MATVEC_BATCH_STEP),
        ("MATVEC_ROW_STEP", MATVEC_ROW_STEP),
        ("ROPE_STEP", ROPE_STEP),
        ("ROPE_ROWS_STEP", ROPE_ROWS_STEP),
        ("ATTEND_STEP", ATTEND_STEP),
        ("ATTEND_CACHE_STEP", ATTEND_CACHE_STEP),
        ("ATTEND_ROWS_STEP", ATTEND_ROWS_STEP),
        ("SWIGLU_STEP", SWIGLU_STEP),
        ("DECODE_STEP", DECODE_STEP),
        ("PREFILL_STEP", PREFILL_STEP),
        ("PRE_STEP", PRE_STEP),
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

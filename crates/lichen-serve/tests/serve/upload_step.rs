//! A weight load makes a resident out of host data, which a step's output is otherwise the only way.

use lichen_kernel_ir::ScalarData;
use lichen_serve::Session;

/// Host words come back as the device holds them, and the id survives a step that reads it.
///
/// # Invariant
/// The upload's own value is its answer, so the assertion is the round trip: the resident holds the
/// words it was given. The second half is the point of it — a resident a load produced is a valid
/// `RunArgument`, which is what keeps a model's weights off the host for every later step.
#[test]
fn a_load_makes_a_resident_out_of_host_words() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED a_load_makes_a_resident_out_of_host_words: no device");
        return;
    };
    let weights = [2.0f32, 2.0, -3.0, 3.0, 1.0, 4.0, 2.0, -5.0];
    let resident = session
        .upload(&weights)
        .unwrap_or_else(|refusal| panic!("the weights load: {refusal}"));
    assert_eq!(
        resident.count,
        weights.len(),
        "the resident holds one element per word uploaded"
    );
    match session
        .fetch(resident)
        .unwrap_or_else(|refusal| panic!("the resident is readable: {refusal}"))
    {
        ScalarData::Float(values) => assert_eq!(
            values.as_slice(),
            weights.as_slice(),
            "the words came back as they went in"
        ),
        other => panic!("a weight load declares Float words, and this is {other:?}"),
    }
    session.release(resident);
    println!("a weight load made a resident out of eight host words");
}

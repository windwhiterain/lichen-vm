//! What `place` puts on the device, read back.

use lichen_kernel_ir::{ScalarClass, ScalarData};
use lichen_serve::Session;

/// The half of each `f32`, little-endian, which is one `bf16` element.
fn halves(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| ((value.to_bits() >> 16) as u16).to_le_bytes())
        .collect()
}

/// Bytes placed at a class read back as that class's numbers.
///
/// # Invariant
/// The four numbers are not exact in four bytes and are exact in two, so a payload read at the wrong width
/// answers a different number rather than the same one: the copy moves bytes and the restatement is what
/// says how many of them are elements.
#[test]
fn placed_bytes_read_back_at_the_class_they_were_placed() {
    let Ok(session) = Session::open() else {
        eprintln!("SKIPPED placed_bytes_read_back_at_the_class_they_were_placed: no device");
        return;
    };
    let values = [1.5_f32, 2.0, 3.0, 4.0];
    let resident = session
        .place(&halves(&values), ScalarClass::BFloat16)
        .unwrap_or_else(|refusal| panic!("the bytes are placed: {refusal}"));
    let answered = session.fetch(resident).expect("the placed bytes come back");
    session.release(resident);
    match answered {
        ScalarData::Float(answered) => {
            assert_eq!(
                answered,
                values.to_vec(),
                "the numbers come back as they went"
            );
        }
        other => panic!("a brain-float buffer answers as numbers, and this answered {other:?}"),
    }
}

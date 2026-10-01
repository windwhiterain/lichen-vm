//! The leaf-name discriminator's length is a one-byte field, so the writer
//! must refuse a name it cannot encode rather than truncate the length and
//! desynchronise every field after it.

use lichen_registry::codec::{Reader, Writer};

#[test]
fn a_leaf_name_the_length_field_cannot_hold_is_refused() {
    // One past the largest length a one-byte field can encode.
    let name = "n".repeat(256);
    let mut writer = Writer::new();
    let refused = writer.leaf(&name);
    assert!(
        refused.is_err(),
        "a leaf name of {} bytes cannot be encoded and must be refused",
        name.len()
    );
    assert!(
        writer.finish().is_err(),
        "a writer that refused a leaf name must not hand out a buffer"
    );
}

#[test]
fn a_leaf_name_the_length_field_holds_round_trips() {
    let name = "n".repeat(255);
    let mut writer = Writer::new();
    writer.leaf(&name).expect("255 bytes fits the field");
    let bytes = writer.finish().expect("a complete leaf name");
    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.leaf_name().expect("the leaf name"), name.as_bytes());
    assert!(reader.done(), "the reader must consume the whole leaf name");
}

//! The compiler-cache slot key: the single derivation the package manager
//! (which builds a composed compiler into the slot) and the compiler (which
//! locates its own artifact-cache root in it) both call.

use lichen_utils::cache::{DEFAULT_CORE_REPO, compiler_slot_key};

#[test]
fn the_core_repository_is_part_of_the_key() {
    // Two repositories build differently-linked core crates, so a compiler
    // from one must never be reused for the other.
    assert_ne!(
        compiler_slot_key(DEFAULT_CORE_REPO, &[]),
        compiler_slot_key("file:///elsewhere/lichen-vm", &[])
    );
}

#[test]
fn the_plugin_set_is_part_of_the_key_in_any_order() {
    let ordered = compiler_slot_key(DEFAULT_CORE_REPO, &["a@1".into(), "b@2".into()]);
    let reordered = compiler_slot_key(DEFAULT_CORE_REPO, &["b@2".into(), "a@1".into()]);
    assert_eq!(ordered, reordered, "the key covers the set, not its order");
    assert_ne!(
        ordered,
        compiler_slot_key(DEFAULT_CORE_REPO, &["a@1".into()]),
        "a changed plugin set keys a different slot"
    );
}

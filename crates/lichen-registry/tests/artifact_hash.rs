//! The artifact identity fold, pinned directly.
//!
//! The fold is what makes the cache transitive, and the difference it turns on
//! is exactly one thing: a dependency contributes its **identity**, not its key.
//! A recompile reuses the key (the key names the cache slot, not the content
//! behind it), so a fold over keys alone leaves an importer's identity unchanged
//! when a dependency's content changed — and its frozen artifact, full of
//! cross-module node references written as `(dependency key, index)`, is then
//! served against the dependency's new node layout.

use lichen_registry::{ModuleKey, artifact_hash, sha256};

fn identity_of(content: &str) -> [u8; 32] {
    sha256(content.as_bytes())
}

#[test]
fn a_dependency_identity_moves_the_importer() {
    let key = ModuleKey::from_raw(7);
    let before = artifact_hash(
        identity_of("import before"),
        &[(key, identity_of("succ = x => x + 1\n"))],
    );
    let after = artifact_hash(
        identity_of("import before"),
        &[(key, identity_of("succ = x => x + 1\nextra = 7\n"))],
    );
    assert_ne!(
        before, after,
        "the dependency's content changed but held its key, so only the \
         identity can tell the importer apart"
    );
}

#[test]
fn the_own_source_still_moves_the_identity() {
    let key = ModuleKey::from_raw(7);
    let dep = identity_of("succ = x => x + 1\n");
    assert_ne!(
        artifact_hash(identity_of("one"), &[(key, dep)]),
        artifact_hash(identity_of("two"), &[(key, dep)]),
        "a package's own source must still be part of its identity"
    );
}

#[test]
fn the_dependency_order_is_part_of_the_identity() {
    let a = (ModuleKey::from_raw(1), identity_of("a"));
    let b = (ModuleKey::from_raw(2), identity_of("b"));
    assert_ne!(
        artifact_hash(identity_of("s"), &[a, b]),
        artifact_hash(identity_of("s"), &[b, a]),
        "the fold is over source order, so a reordering is a different artifact"
    );
}

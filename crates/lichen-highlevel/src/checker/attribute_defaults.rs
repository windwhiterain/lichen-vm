//! The rule a shared default attribute node is judged by. See
//! `docs/notes/array-attribute-view.md`.

use super::sole_default;

/// One matching position names that attribute.
#[test]
fn one_matching_default_names_its_position() {
    assert_eq!(sole_default(&[false, true, false]), Some(1));
    assert_eq!(sole_default(&[true]), Some(0));
}

/// A shared default names **none**: every position holds one node, so a class that
/// owns it matches them all.
///
/// # Invariant
/// Taking the first match would read one attribute's order as another's — the shape
/// this rule replaced.
#[test]
fn a_shared_default_names_no_position() {
    assert_eq!(sole_default(&[true, true, false]), None);
    assert_eq!(sole_default(&[true, true, true]), None);
}

/// A class no default owns names none.
#[test]
fn an_unshared_default_names_no_position() {
    assert_eq!(sole_default(&[false, false, false]), None);
    assert_eq!(sole_default(&[]), None);
}

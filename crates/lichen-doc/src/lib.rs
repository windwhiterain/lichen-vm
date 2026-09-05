//! The `lichen-doc` plugin: the doc attribute (`? expr`) — a **label** that
//! attaches a metadata value (a struct instance the user builds — by
//! convention a `Doc` struct with `.name` and `.description` fields) to any
//! expression.
//!
//! # A compiler plugin, not a native plugin
//!
//! Like `lichen-perspective`, this crate is a **compiler plugin** (see
//! `docs/notes/plugin-taxonomy.md`): the language layer must **codesign** with
//! it —
//!
//! - the grammar production and AST node for `? expr` (the `AnnPiece::Doc`
//!   annotation),
//! - the `Schema { tail: [.., Perspective, Doc, ..] }` IR form the checker
//!   reads, and
//! - the lowering of a `?` annotation into the label slot (`check_ann`).
//!
//! Those live in the host language layer (e.g. `lichen-language`), because they
//! are tied to one language's surface and on-disk contract and so cannot be
//! shared by any host.  What *can* be shared — and is now in this crate — is
//! the **meaning**: the [`Doc`] attribute marker and its [`AttrExt`] lowering.
//! A host composes the marker into its own attribute vocabulary and calls
//! [`doc_attr_ext::<P>()`] to register the attribute's extension with the
//! checker.
//!
//! # A label, not a constraint
//!
//! Unlike `Perspective` (whose slot value unifies under the divisibility
//! lattice and is checked at every apply), a `Doc` carries no constraint:
//!
//! - [`AttrExt::combine`] returns *no doc* — a compound's doc is its own
//!   annotation, never a meet of its children's docs.
//! - [`AttrExt::unify_slots`] **propagates** the doc onto the other side and
//!   never reports a failure (the existing doc is kept — the override case).
//! - [`AttrExt::is_label`] is `true`, so `Doc` contributes no constraint slot;
//!   the label's runtime slot is the annotation value's `[value, type]` term
//!   pair (so the renderer can walk the value's type chain).
//!
//! The doc value is just any first-class value (a struct instance), so the
//! type system validates it like any other.  The checker's slot for the label
//! is the `?` expression's `[value, type]` term pair — the uniform slot shape
//! shared with a constraint — so the renderer can walk the value's whole type
//! chain: a doc's *field names* come from the struct type, not a hardcoded
//! shape.  That render reuses the program-generic printer in `lichen-render`.
//!
//! [`AttrExt`]: lichen_highlevel::attr::AttrExt

pub mod doc;

pub use doc::{Doc, doc_attr_ext};

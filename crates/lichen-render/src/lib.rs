//! The program-generic pretty printer: how lowlevel/highlevel values, types,
//! and attribute slots read in a host's own syntax.
//!
//! This crate is the "pretty view" of the generic core.  [`lichen-highlevel`]
//! stays raw (it knows only the recursive-pair encoding and the checker's
//! facts); this crate reads the top of that encoding — a value against its
//! type chain, a type expression, a struct instance's named fields — and
//! spells it in a concrete language's syntax.  Everything here is generic over
//! [`HighProgram`], so any host that composes the core's value/type
//! vocabularies can render with the same machinery; a plugin that needs to
//! spell its own attribute slot (e.g. `lichen-doc`'s `? name = "…"`) uses the
//! same printer instead of carrying its own.
//!
//! The host surface lives in the host crate: `lichen-language`'s `render`
//! module re-exports these and layers the host-specific shells (the caret
//! diagnostic and the checker-message wording) on top.
//!
//! [`HighProgram`]: lichen_highlevel::program::HighProgram
//! [`lichen-highlevel`]: lichen_highlevel

pub mod render;

pub use render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named,
};

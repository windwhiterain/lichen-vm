//! The program-generic pretty printer: how values, types, and attribute slots
//! read in a host's own syntax.
//!
//! # Invariant
//!
//! Everything here is generic over [`HighProgram`], so any host composing the
//! core's value/type vocabularies renders with the same machinery.  The
//! host-specific wording lives in the host crate: `lichen-language`'s `render`
//! module re-exports these and layers its caret diagnostic on top.  See
//! docs/notes/raw-rendering-mark.md.
//!
//! [`HighProgram`]: lichen_highlevel::program::HighProgram

pub mod render;

pub use render::{
    TypePrinter, ValuePrinter, print_type, print_value, render_attributes,
    render_struct_fields_named, struct_type_named_fields, value_label,
};

//! The `lichen-doc` plugin: the doc attribute `? expr`, a label carrying
//! metadata.  See docs/notes/attributes.md.

pub mod doc;

pub use doc::{Doc, doc_attr_ext};

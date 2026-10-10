//! The lichen tooling crate: the shared editor view of the frontend.
//! Layering: `docs/notes/language-toolchain.md`.

pub mod analysis;
pub mod home;
pub mod lsp;
#[cfg(feature = "server")]
pub mod server;

pub use analysis::{Definition, Doc, DocIndex, StatementValue};
pub use lsp_types;

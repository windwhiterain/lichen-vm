//! The lichen editor-tooling library: the shared *editor view* of the frontend
//! artifacts.
//!
//! `lichen-language` owns the grammar — `lex`, `ast`, `parse`, `compile`, `diag`
//! — and this crate builds the *lookup* on top of it that an editor needs and
//! the raw frontend does not provide:
//!
//! - [`lsp`] — a minimal LSP type set and the source-position ↔ LSP-position
//!   conversion (typename `Span = (line, col)` in 1-based bytes ↔ LSP `line /
//!   character` in 0-based UTF-16 code units).
//! - [`analysis`] — [`Doc`]: parse a source once and hold the tokens (byte
//!   ranges), the AST, the pipeline diagnostics and a name-resolution index, so
//!   the LSP server and the Zed extension answer hover / go-to-definition from
//!   the *same* shared artifacts.
//!
//! Both the `lichen-language-server` binary (below) and the
//! `lichen-language-zed` extension consume this library, so the span math and
//! the definition logic are byte-for-byte identical across editors. See
//! `docs/notes/language-toolchain.md` for the crate-boundary rationale: one
//! frontend, one tooling crate, many thin entry points — *not* one crate per
//! tool.

pub mod analysis;
pub mod lsp;

pub use analysis::{Definition, Doc};
pub use lsp::{Diagnostic, Position, Range};

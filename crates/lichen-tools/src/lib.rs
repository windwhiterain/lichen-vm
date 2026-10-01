//! Repository tooling that is not part of the compiler pipeline.
//!
//! The README example sync lives here rather than in `lichen-language` because
//! it is a repository maintenance tool, not a compiler surface: it walks the
//! checkout through the compile-time `CARGO_MANIFEST_DIR`, and shipping it in
//! the library made every embedder link a generator it never calls (`P2-6`).
//!
//! See [readme-sync](../../docs/notes/readme-sync.md) for the workflow, and
//! [`readme`]'s own rustdoc for the rendering rules.

pub mod readme;

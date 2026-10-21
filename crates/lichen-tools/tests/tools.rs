//! The README tooling's own suite: one binary, one module per area.
//!
//! # Invariant
//! Cargo links a test binary per `tests/*.rs` and per `tests/<dir>/main.rs`, so the areas are
//! modules under `tests/tools/` and this root names that directory with `#[path]`. A root's
//! submodules resolve beside it, not under its stem, which is why the directory's `mod.rs` is
//! named here.

#[path = "tools/mod.rs"]
mod tools;

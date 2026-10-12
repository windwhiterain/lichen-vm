//! This crate's whole integration suite: one binary, one module per area.
//!
//! # Invariant
//! Cargo links a test binary per `tests/*.rs` and per `tests/<dir>/main.rs`, each statically
//! linking the whole stack, so an area per file meant a relink per area. The areas are modules
//! under `tests/compute_gpu/` instead, none named `main.rs`, so the directory is not a binary.
//! A root's submodules resolve beside it, not under its stem, so `mod.rs` is named here.

#[path = "compute_gpu/mod.rs"]
mod compute_gpu;

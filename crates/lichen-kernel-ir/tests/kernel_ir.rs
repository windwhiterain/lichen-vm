//! This crate's whole integration suite: one binary, one module per area.
//!
//! # Invariant
//! Cargo links a test binary per `tests/*.rs` and per `tests/<dir>/main.rs`, each statically
//! linking the whole stack, so an area per file meant a relink per area. The areas are modules
//! under `tests/kernel_ir/` instead, none named `main.rs`, so the directory is not a binary.
//! A root's submodules resolve beside it, not under its stem, so `mod.rs` is named here.

#[path = "kernel_ir/mod.rs"]
mod kernel_ir;

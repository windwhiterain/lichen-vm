//! The `lichen-compiler` command-line surface, in a crate of its own.
//!
//! The compiler itself is the [`lichen_language`] library; this crate owns the
//! argument parser and the compiler binary, so a program that embeds the
//! library (the language server, for one) does not link `clap`.  A
//! plugin-built compiler's generated `main` calls
//! [`cli::main_with_native_packages`] over its composed program, which is why
//! the crate has a library half as well as the binary.

pub mod cli;

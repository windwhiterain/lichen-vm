//! The language compiler binary, `lichen-compiler`: install it with `cargo
//! install --path crates/lichen-compiler`.

use std::process::ExitCode;

fn main() -> ExitCode {
    lichen_compiler::cli::main::<lichen_language::program::LangProgram>()
}

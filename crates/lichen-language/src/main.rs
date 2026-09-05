//! The language compiler binary: `lichen-compiler`.
//!
//! `lichen-compiler <program.lichen>` compiles and runs one program, printing
//! its output; a directory path runs every `.lichen` file in it, printing
//! `file: output` per program.  `run`, `build`, and `cache gc` subcommands are
//! also accepted.
//!
//! The CLI is depend-aware: a file's `depend "url"` directives resolve against
//! the lichen-home source cache (populated by the package manager's
//! `lichen fetch`), so a compiler built with a native plugin can be driven
//! directly by the package manager.
//!
//! Install it with `cargo install --path crates/lichen-language` (from a
//! checkout of this repo) or `cargo install --git <repo-url> lichen-language`.
//! The binary is named `lichen-compiler`; the package manager
//! (`crates/lichen-package`, binary `lichen`) is the tool that fetches and
//! drives it.

use std::process::ExitCode;

fn main() -> ExitCode {
    lichen_language::cli::main::<
        lichen_language::program::LangValue,
        lichen_language::program::LangOperator,
        lichen_language::persist::HighProgramCodec,
    >()
}

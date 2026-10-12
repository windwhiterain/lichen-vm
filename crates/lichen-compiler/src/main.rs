//! The language compiler binary, `lichen-compiler`: install it with `cargo
//! install --path crates/lichen-compiler`.

use std::process::ExitCode;

fn main() -> ExitCode {
    // The shipping compiler is the compute sub-language.
    // The store carries what this binary registers.
    lichen_compiler::cli::main_with_wiring::<lichen_compute::language::LangProgram>(
        &lichen_language::persist::shipping_cache_root(),
        &lichen_compute::language::native_packages(),
        lichen_compute::language::wiring(),
    )
}

//! Integration tests for the VM's basic runtime layer.  The `lowlevel` core
//! is one submodule; `highlevel` (the checker built over it) will be
//! another.  Each layer is a same-name file + directory pair inside this
//! crate, so plain `mod` declarations resolve them — no `#[path]` needed.

mod lowlevel;

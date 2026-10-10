//! The preprocessor surface, re-exported from `lichen_preprocess`.
//! See docs/notes/preprocessor-isolation.md.

pub use lichen_preprocess::{
    Depend, Directive, ImportResolver, PreprocessDiag, Preprocessed, ResolvedImport,
    ResolvedPackage, block_depends, block_directives, block_metadata, depend_of, split_block,
};

//! The lichen language parser: `Token`s to AST, depending only on the lex crate
//! (for `Span` and `Token`).

pub mod ast;
pub mod path;

#[path = "parse.rs"]
mod parser;

pub use parser::{
    ParseDiag, Parsed, collect_error_blocks, parse, parse_statement_region,
    parse_statement_region_traced,
};

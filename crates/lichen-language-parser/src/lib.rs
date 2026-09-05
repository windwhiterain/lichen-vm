//! The lichen language parser: `Token`s → AST.  Depends only on the lex crate
//! (for [`span`](lichen_language_lex::Span) and [`Token`]), so it is a pure,
//! checker-free front-end.  The language crate re-exports this crate as `parse`
//! (and `ast`), so `lichen_language::parse::parse` / `::ast::Expr` resolve.

pub mod ast;

#[path = "parse.rs"]
mod parser;

pub use parser::{
    ParseDiag, Parsed, collect_error_blocks, parse, parse_statement_region,
    parse_statement_region_traced,
};

//! `adoctra-core`: parser and document model for the adoctra AsciiDoc toolchain.
//!
//! Editor-oriented: [`parse`] never fails on malformed syntax — it returns a
//! [`ParseResult`] (partial document + diagnostics), nodes carry source
//! [`Span`]s, and [`tokens`] produces a highlight stream. Backends still use
//! the [`Backend`] trait and the [`Error`]/[`Result`] types for real failures.

pub mod ast;
pub mod attributes;
pub mod backend;
mod comments;
mod conditional_expression;
mod context;
pub mod diagnostic;
pub mod error;
pub mod extension;
mod header;
mod inline;
pub mod json;
mod parser;
mod reader;
mod replacements;
mod resolve;
mod scanner;
pub mod source;
mod tokenizer;
pub mod visit;

pub use ast::{Author, Block, BlockKind, Document, DocumentHeader, Inline, Revision, Span};
pub use attributes::Attributes;
pub use backend::Backend;
pub use context::ParseOptions;
pub use diagnostic::{Diagnostic, ParseResult, Severity};
pub use error::{Error, Result};
pub use extension::{Registry, TreeProcessor};
pub use source::{SourceFile, SourceId, SourceMap, SourcePosition};
pub use tokenizer::{Token, TokenKind};

/// Parse AsciiDoc source into a [`ParseResult`] (never fails on syntax).
pub fn parse(source: &str) -> ParseResult {
    parse_with_options(source, &ParseOptions::default())
}

/// Parse one source with its identity and hard attribute overrides. The caller
/// retains the corresponding [`SourceMap`]; parsing performs no file I/O.
pub fn parse_with_options(source: &str, options: &ParseOptions) -> ParseResult {
    parser::parse_with_options(source, options)
}

/// Tokenize AsciiDoc source for syntax highlighting.
pub fn tokens(source: &str) -> Vec<Token> {
    tokens_with_options(source, &ParseOptions::default())
}

/// Highlight one source using the same source identity and attribute options.
pub fn tokens_with_options(source: &str, options: &ParseOptions) -> Vec<Token> {
    tokenizer::tokens_with_options(source, options)
}

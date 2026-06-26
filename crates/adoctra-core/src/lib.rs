//! `adoctra-core`: parser and document model for the adoctra AsciiDoc toolchain.
//!
//! Editor-oriented: [`parse`] never fails on malformed syntax — it returns a
//! [`ParseResult`] (partial document + diagnostics), nodes carry source
//! [`Span`]s, and [`tokens`] produces a highlight stream. Backends still use
//! the [`Backend`] trait and the [`Error`]/[`Result`] types for real failures.

pub mod ast;
pub mod attributes;
pub mod backend;
pub mod diagnostic;
pub mod error;
pub mod extension;
mod inline;
pub mod json;
mod parser;
mod replacements;
mod resolve;
mod scanner;
mod tokenizer;

pub use ast::{Block, BlockKind, Document, Inline, Span};
pub use attributes::Attributes;
pub use backend::Backend;
pub use diagnostic::{Diagnostic, ParseResult, Severity};
pub use error::{Error, Result};
pub use extension::{Registry, TreeProcessor};
pub use tokenizer::{Token, TokenKind};

/// Parse AsciiDoc source into a [`ParseResult`] (never fails on syntax).
pub fn parse(source: &str) -> ParseResult {
    parser::parse(source)
}

/// Tokenize AsciiDoc source for syntax highlighting.
pub fn tokens(source: &str) -> Vec<Token> {
    tokenizer::tokens(source)
}

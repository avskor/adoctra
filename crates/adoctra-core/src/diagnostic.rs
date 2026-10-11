//! Parse diagnostics.
//!
//! For an editor, syntax problems must be *data*, not a hard failure: the
//! parser always returns a (possibly partial) document plus a list of
//! diagnostics. Real failures (I/O in backends) still use [`crate::Error`].

use crate::ast::{Document, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub span: Span,
    pub severity: Severity,
    pub message: String,
}

/// The result of parsing for editor use: always succeeds, carries diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult {
    pub document: Document,
    pub diagnostics: Vec<Diagnostic>,
    /// Source trivia omitted from the semantic tree, in source order.
    pub comments: Vec<Span>,
    /// Recognized conditional syntax (single-line content is excluded).
    pub conditional_directives: Vec<Span>,
    /// Lines omitted by inactive conditional branches.
    pub inactive: Vec<Span>,
    /// Include directives processed through an explicit provider.
    pub include_directives: Vec<Span>,
    /// Ordered original fragments for spans carrying a provenance index.
    pub provenance: Vec<Vec<Span>>,
}

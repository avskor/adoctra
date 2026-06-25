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
}

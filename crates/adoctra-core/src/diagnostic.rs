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
    /// For an omitted source line, `span` is an empty insertion-point anchor
    /// and this range identifies the exact bytes in `SourceFile::omitted_lines`.
    pub omitted_span: Option<crate::OmittedSourceSpan>,
    pub span: Span,
    pub severity: Severity,
    pub message: String,
    /// Conversion-facing presentation. Physical editor data above is retained.
    pub report: DiagnosticReport,
}

/// A logger cursor need not identify the physical span (selected includes and
/// reader lookahead can change its file and line in the reference processor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticCursor {
    pub source_id: crate::SourceId,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DiagnosticLocation {
    /// Use the physical span, or resolve its reader cursor before returning.
    #[default]
    Source,
    Unlocated,
    Cursor(DiagnosticCursor),
}

/// Presentation overrides for conversion logs; never used to rewrite spans or
/// remove editor diagnostics. Unspecified severity/text use the diagnostic's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiagnosticReport {
    pub suppressed: bool,
    pub severity: Option<Severity>,
    pub message: Option<String>,
    pub location: DiagnosticLocation,
}

impl DiagnosticReport {
    pub(crate) fn unterminated(name: &str) -> Self {
        Self {
            severity: Some(Severity::Warning),
            message: Some(format!("unterminated {name} block")),
            ..Self::default()
        }
    }
}

/// The result of parsing for editor use: always succeeds, carries diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseResult {
    /// Source highlighting from the same structural parse as `document`.
    /// Tokens crossing includes are split into physical source fragments.
    pub tokens: Vec<crate::Token>,
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

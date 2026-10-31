//! `adoctra`: umbrella crate. Re-exports the core editor/parse API and, behind
//! feature flags, the native backends.
//!
//! ```
//! let result = adoctra::parse("= Hello\n\nWorld");
//! assert!(result.diagnostics.is_empty());
//! # #[cfg(feature = "html")]
//! # {
//! use adoctra::Backend;
//! let html = adoctra::html::HtmlBackend::new().convert(&result.document).unwrap();
//! assert!(String::from_utf8(html).unwrap().contains("<h1>Hello</h1>"));
//! # }
//! ```

pub use adoctra_core::{
    parse, parse_resource, parse_resource_with_start, parse_with_options, parse_with_start,
    resolve_system_path, tokens, tokens_resource, tokens_with_options, Attributes, Author,
    AuthorDetails, Backend, BinaryResource, Block, BlockKind, ConversionStart, DateTime,
    Diagnostic, DiagnosticCursor, DiagnosticLocation, DiagnosticReport, DocinfoLocation, Document,
    DocumentDates, DocumentHeader, Error, IncludeInput, IncludeLimits, IncludeResource, Inline,
    MemoryResources, OmittedSourceLine, OmittedSourceSpan, OutputPaths, OutputTarget, ParseOptions,
    ParseResult, Registry, Resource, ResourceError, ResourceErrorKind, ResourceProvider,
    ResourceRequest, ResourceResult, Result, Revision, SafeMode, Severity, SourceFile, SourceId,
    SourceMap, SourcePosition, Span, SystemPath, Token, TokenKind, TreeProcessor,
};

#[cfg(feature = "html")]
pub use adoctra_html as html;

#[cfg(feature = "docx")]
pub use adoctra_docx as docx;

#[cfg(feature = "json")]
pub use adoctra_json as json;

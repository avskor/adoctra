//! `adoctra-core`: parser and document model for the adoctra AsciiDoc toolchain.
//!
//! Editor-oriented: [`parse`] never fails on malformed syntax — it returns a
//! [`ParseResult`] (partial document + diagnostics), nodes carry source
//! [`Span`]s, and [`tokens`] produces a highlight stream. Backends still use
//! the [`Backend`] trait and the [`Error`]/[`Result`] types for real failures.

pub mod ast;
pub mod attributes;
mod author_values;
pub mod backend;
mod block_attributes;
mod blockquote_credit;
mod body_subs;
mod builtin_attributes;
mod callouts;
mod comments;
mod conditional_expression;
mod context;
mod data_uri;
pub mod diagnostic;
mod docinfo;
mod entity_references;
pub mod error;
pub mod extension;
mod header;
mod highlight_stage;
mod image;
mod include_attributes;
mod include_indent;
mod include_selection;
mod indentation;
mod inline;
mod inline_attributes;
mod inline_pass;
mod inline_quotes;
pub mod json;
mod lists;
mod manpage;
mod media;
mod parser;
mod pass_subs;
mod post_replacements;
mod reader;
mod reftext;
mod replacements;
mod resolve;
pub mod resource;
mod scanner;
mod section_numbers;
pub mod source;
mod source_encoding;
mod substitution_context;
mod substitution_text;
mod system_path;
mod table;
mod tokenizer;
mod unicode_word;
pub mod visit;

pub use ast::{
    Author, AuthorDetails, Block, BlockKind, Document, DocumentHeader, Inline, Manpage,
    NameSection, Revision, Span,
};
pub use attributes::{
    AttributeFormat, AttributeLink, AttributePart, Attributes, Footnote, FootnoteKind, IndexTerm,
    InlineCallout, InlineImage, Stem, StemStyle, Svg, UiMacro, Xref,
};
mod anchor_catalog;
mod attribute_list;
mod attribute_macros;
mod attribute_quotes;
mod xref_target;
pub use backend::{Backend, ConversionStart, HighlightRequest, Highlighted};
pub use context::{
    DateTime, DocumentDates, IncludeLimits, OutputPaths, OutputTarget, ParseOptions, SafeMode,
};
pub use diagnostic::{
    Diagnostic, DiagnosticCursor, DiagnosticLocation, DiagnosticReport, ParseResult, Severity,
};
pub use docinfo::{docinfo, DocinfoLocation};
pub use entity_references::character_reference_end;
pub use error::{Error, Result};
pub use extension::{Registry, TreeProcessor};
pub use image::{inline_svg, normalize_web_path};
pub use resource::{
    BinaryResource, IncludeInput, IncludeResource, MemoryResources, OmittedSourceLine,
    OmittedSourceSpan, Resource, ResourceError, ResourceErrorKind, ResourceProvider,
    ResourceRequest, ResourceResult,
};
pub use source::{SourceFile, SourceId, SourceMap, SourcePosition};
pub use system_path::{resolve_system_path, SystemPath};
pub use tokenizer::{Token, TokenKind};

/// Parse AsciiDoc source into a [`ParseResult`] (never fails on syntax).
/// The Asciidoctor release whose behavior adoctra reproduces
/// (`compatibility/asciidoctor-version`), exposed to documents as the locked
/// `asciidoctor-version` attribute like Ruby's `Document#initialize`.
pub const COMPATIBILITY_VERSION: &str = "2.0.23";

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

/// Parse a registered snapshot, expanding includes through the supplied provider.
/// Only an invalid primary id returns Err; include failures are diagnostics.
/// If the provider supplies a [`ResourceProvider::document_path`], it determines
/// locked `docfile`, `docdir`, `docname` and `docfilesuffix` attributes for the
/// primary document, overriding those keys in `options.attributes`.
pub fn parse_resource(
    sources: &mut SourceMap,
    options: &ParseOptions,
    provider: &dyn ResourceProvider,
) -> ResourceResult<ParseResult> {
    let options = options.for_resource(sources, provider);
    let reader = reader::Reader::with_resources(sources, &options, provider)?;
    Ok(parser::parse_reader(&options, reader, None).0)
}

/// [`parse_resource`] with a converter's [`ConversionStart`] hook, called
/// where Ruby's converter starts the document (see the trait).
pub fn parse_resource_with_start(
    sources: &mut SourceMap,
    options: &ParseOptions,
    provider: &dyn ResourceProvider,
    start: &mut dyn ConversionStart,
) -> ResourceResult<ParseResult> {
    let options = options.for_resource(sources, provider);
    let reader = reader::Reader::with_resources(sources, &options, provider)?;
    Ok(parser::parse_reader(&options, reader, Some(start)).0)
}

/// [`parse_with_options`] with a converter's [`ConversionStart`] hook.
pub fn parse_with_start(
    source: &str,
    options: &ParseOptions,
    start: &mut dyn ConversionStart,
) -> ParseResult {
    parser::parse_with_start(source, options, start)
}

/// Highlight a registered document and its includes. Each token belongs to one
/// original source; tokens crossing source boundaries are split into fragments.
pub fn tokens_resource(
    sources: &mut SourceMap,
    options: &ParseOptions,
    provider: &dyn ResourceProvider,
) -> ResourceResult<Vec<Token>> {
    let options = options.for_resource(sources, provider);
    let reader = reader::Reader::with_resources(sources, &options, provider)?;
    Ok(tokenizer::tokens_reader(&options, reader))
}

//! The document model.
//!
//! Every block carries a [`Span`] — its byte range in the source — which is
//! what makes the model usable by an editor (highlighting, fold ranges,
//! "click in preview -> jump to source", diagnostics). Inline nodes stay
//! span-free for now; highlighting is driven by the separate token stream
//! (see [`crate::tokens`]).

use crate::attributes::Attributes;

/// A byte range `[start, end)` in the original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A whole parsed document: header attributes plus a sequence of blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub attributes: Attributes,
    pub blocks: Vec<Block>,
}

/// A block node: its source span plus its kind.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub kind: BlockKind,
}

/// Block-level element kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    /// A section title. `level` is the number of leading `=` minus one
    /// (so a document title `= Title` is level 0, `== Section` is level 1).
    Heading { level: u8, inlines: Vec<Inline> },
    /// A run of text forming a paragraph.
    Paragraph { inlines: Vec<Inline> },
    /// A literal/listing block (verbatim, no inline substitutions).
    Listing { text: String },
}

/// Inline (phrase-level) elements.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    // TODO: links, cross-references, inline macros, attribute references...
}

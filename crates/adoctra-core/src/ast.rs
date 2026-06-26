//! The document model.
//!
//! Every node — block *and* inline — carries a [`Span`]: its byte range in the
//! original source. That is what makes the model usable by an editor
//! (highlighting, fold ranges, "click in preview -> jump to source",
//! diagnostics) directly from the AST, without a duplicated grammar.
//!
//! Inline text is *resolved* (semantic): attribute references are substituted
//! and typographic replacements (`(C)` -> `©`, `--` -> `—`, …) are applied
//! here. The crucial invariant is that an inline's textual content and its
//! [`Span`] are **not** required to be the same length: the content is what the
//! source *means*, the span is where it *came from*. Output-encoding
//! (HTML-escaping `<` to `&lt;`, etc.) is deliberately left to each backend, so
//! the shared model stays format-neutral.

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

    /// Length of the source range in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Whether the range is empty.
    pub fn is_empty(&self) -> bool {
        self.end == self.start
    }
}

/// A whole parsed document: header attributes plus a sequence of blocks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Document {
    pub attributes: Attributes,
    pub blocks: Vec<Block>,
}

/// A block node: its source span, an optional id, and its kind.
///
/// `id` is the block's anchor — the target a `<<id>>` resolves to. It comes from
/// an explicit anchor (`[[id]]`, set by the parser) or, for a section heading
/// without one, an auto-generated [`sectids`] id assigned during the resolve
/// pass. It is `None` for any block with no anchor (a plain paragraph, the
/// level-0 document title, …) and before resolution runs for an auto-id section.
///
/// `reftext` is the inline display text from an explicit `[[id,reftext]]` anchor;
/// the resolve pass uses it as a textless reference's display text (preferring it
/// over a section's own title). `None` when no reftext was given.
///
/// [`sectids`]: crate::resolve
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    pub id: Option<String>,
    pub reftext: Option<Vec<Inline>>,
    pub kind: BlockKind,
}

impl Block {
    /// A block with no anchor: `id` and `reftext` default to `None`. The parser
    /// fills them in when an explicit `[[id]]` / `[[id,reftext]]` anchor precedes
    /// the block; the resolve pass fills `id` for an auto-id section.
    pub fn new(span: Span, kind: BlockKind) -> Self {
        Self {
            span,
            id: None,
            reftext: None,
            kind,
        }
    }
}

/// Block-level element kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    /// A section title. `level` is the number of leading `=` minus one
    /// (so a document title `= Title` is level 0, `== Section` is level 1).
    /// The section's anchor lives on the enclosing [`Block::id`] — auto-generated
    /// from the title during the resolve pass (asciidoctor's `sectids`, e.g.
    /// `== Hello World` → `_hello_world`) unless an explicit `[[id]]` anchor
    /// overrides it. Cross-references resolve against that id.
    Heading { level: u8, inlines: Vec<Inline> },
    /// A run of text forming a paragraph.
    Paragraph { inlines: Vec<Inline> },
    /// A literal/listing block (verbatim, no inline substitutions).
    Listing { text: String },
    /// An unordered (`*`/`-`) or ordered (`.`) list. `ordered` selects the
    /// rendered list kind. Nesting is structural: a deeper level is a
    /// [`BlockKind::List`] held in a [`ListItem`]'s `blocks`.
    List { ordered: bool, items: Vec<ListItem> },
}

/// One item of a [`BlockKind::List`]: its resolved inline content (the
/// *principal* text), any nested blocks (e.g. a sublist), and the source
/// [`Span`] of the whole item — from its marker through the end of any nested
/// content it contains.
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub inlines: Vec<Inline>,
    pub blocks: Vec<Block>,
    pub span: Span,
}

/// Inline (phrase-level) elements. Every variant carries its source [`Span`].
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Literal text. `value` is the resolved content; `span` is where it came
    /// from (the two may differ in length once substitutions apply).
    Text { value: String, span: Span },
    /// `*strong*` — bold. `span` covers the whole construct incl. markers.
    Strong { children: Vec<Inline>, span: Span },
    /// `_emphasis_` — italic. `span` covers the whole construct incl. markers.
    Emphasis { children: Vec<Inline>, span: Span },
    /// `` `monospace` `` — code/literal phrase. `span` covers the whole
    /// construct incl. markers.
    Monospace { children: Vec<Inline>, span: Span },
    /// `#mark#` — highlighted text. `span` covers the whole construct incl.
    /// markers.
    Mark { children: Vec<Inline>, span: Span },
    /// `^superscript^` — an *unconstrained* quote (matches mid-word, e.g.
    /// `E=mc^2^`). `span` covers the whole construct incl. markers.
    Superscript { children: Vec<Inline>, span: Span },
    /// `~subscript~` — an *unconstrained* quote (matches mid-word, e.g.
    /// `H~2~O`). `span` covers the whole construct incl. markers.
    Subscript { children: Vec<Inline>, span: Span },
    /// An attribute reference `{name}`, resolved once in core against the
    /// document attributes. `value` is `Some` when the attribute was found, and
    /// `None` for a missing attribute (consumers then fall back to the literal
    /// `{name}`, matching asciidoctor's default `attribute-missing=skip`).
    /// `span` covers the `{name}` token in the source.
    AttributeRef {
        name: String,
        value: Option<String>,
        span: Span,
    },
    /// A link: the `link:target[text]` macro, an autolinked URL macro
    /// `https://host[text]`, or a bare autolinked URL `https://host`. `target`
    /// is the resolved href; `children` is the display text (parsed inlines),
    /// which for a bare link is the target's own text. `bare` is `true` when no
    /// explicit link text was given — asciidoctor renders those with a
    /// `class="bare"` on the `<a>`. `span` covers the whole construct (markers,
    /// scheme and brackets) in the source.
    Link {
        target: String,
        children: Vec<Inline>,
        bare: bool,
        span: Span,
    },
    /// A cross-reference: `<<id>>`, `<<id,text>>`, or the `xref:id[text]` macro.
    /// `target` is the raw anchor id (without the leading `#`); the HTML backend
    /// renders it as `href="#id"`. `children` is the display text. The parser
    /// leaves it *empty* for a textless reference; the resolve pass then fills it
    /// in — with the referenced section's title inlines (formatting preserved,
    /// like asciidoctor's `xrefstyle=basic`) when the id is known, or the literal
    /// `[id]` text when it is not (asciidoctor's *unresolved*-reference rendering,
    /// which also emits a diagnostic). An explicit reftext is kept verbatim.
    /// `span` covers the whole construct (the `<<…>>` or `xref:…[…]`).
    CrossReference {
        target: String,
        children: Vec<Inline>,
        span: Span,
    },
    /// An inline anchor: `[[id]]` / `[[id,reftext]]` appearing *mid-text*, or the
    /// `anchor:id[reftext]` macro. It renders as an empty `<a id="id">` right at
    /// this point — the `reftext` is **not** shown here. Instead the resolve pass
    /// records `id -> reftext` so a textless `<<id>>` aimed at this anchor uses
    /// the reftext as its display text (and falls back to `[id]` when there is
    /// none), exactly like an explicit block anchor's reftext. `reftext` is parsed
    /// inlines (so `*bold*` etc. apply) or `None` when omitted. `span` covers the
    /// whole construct (`[[…]]` or `anchor:…[…]`).
    ///
    /// A *whole-line* `[[id]]` is the distinct [`block anchor`] construct, handled
    /// by the parser before inline scanning; only an anchor with other text on the
    /// line reaches here.
    ///
    /// [`block anchor`]: crate::ast::Block::id
    Anchor {
        id: String,
        reftext: Option<Vec<Inline>>,
        span: Span,
    },
    // TODO: block/inline macros (image:, footnote:)...
}

impl Inline {
    /// The source span of this inline node.
    pub fn span(&self) -> Span {
        match self {
            Inline::Text { span, .. }
            | Inline::Strong { span, .. }
            | Inline::Emphasis { span, .. }
            | Inline::Monospace { span, .. }
            | Inline::Mark { span, .. }
            | Inline::Superscript { span, .. }
            | Inline::Subscript { span, .. }
            | Inline::AttributeRef { span, .. }
            | Inline::Link { span, .. }
            | Inline::CrossReference { span, .. }
            | Inline::Anchor { span, .. } => *span,
        }
    }
}

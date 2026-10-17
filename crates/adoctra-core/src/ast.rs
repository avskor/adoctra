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
use crate::source::SourceId;
use std::collections::BTreeMap;
use std::sync::Arc;

/// A byte range `[start, end)` in the original source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub source_id: SourceId,
    pub start: usize,
    pub end: usize,
    /// Index into ParseResult.provenance for a range crossing source occurrences.
    /// The ordinary coordinates identify its first physical fragment.
    pub provenance: Option<usize>,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self::in_source(SourceId::default(), start, end)
    }

    pub fn in_source(source_id: SourceId, start: usize, end: usize) -> Self {
        Self {
            source_id,
            start,
            end,
            provenance: None,
        }
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
    pub header: Option<DocumentHeader>,
    pub blocks: Vec<Block>,
    /// Whether `blocks[0]` is the document title: a source `= Title` or a
    /// `doctitle` attribute without one. A leading level-0 heading that is not
    /// the title (for example, under a block title) is a part.
    pub has_title: bool,
}

/// Source declarations in a document header. Effective (possibly overridden)
/// metadata lives in `Document.attributes`. The title remains the first level-0
/// heading in `blocks`, avoiding a second mutable copy of its inline tree.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentHeader {
    pub span: Span,
    pub title_span: Span,
    /// Declared title text before substitutions or doctitle overrides.
    /// The effective inline tree remains in the first level-0 heading.
    pub title: String,
    pub authors: Vec<Author>,
    pub revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Author {
    pub span: Span,
    pub name: String,
    pub firstname: String,
    pub middlename: Option<String>,
    pub lastname: Option<String>,
    pub initials: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Revision {
    pub span: Span,
    pub number: Option<String>,
    pub date: Option<String>,
    pub remark: Option<String>,
}

/// A block node: its source span, an optional id, and its kind.
///
/// `id` is the block's anchor — the target a `<<id>>` resolves to. It comes from
/// an explicit anchor (`[[id]]`, set by the parser) or, for a section heading
/// without one, an auto-generated [`sectids`] id assigned during the resolve
/// pass. It is `None` for any block with no anchor, including an unanchored
/// document title, and before resolution runs for an auto-id section.
///
/// `reftext` is the inline display text from an explicit `[[id,reftext]]` anchor;
/// the resolve pass uses it as a textless reference's display text (preferring it
/// over a section's own title when nonempty). `None` when no reftext was given.
/// Block-anchor reftext executes an early attribute stage followed by c/q/r;
/// even an empty result can retain source reference nodes for editor consumers.
///
/// `roles` are the CSS roles attached by a `[.role]` / `[#id.role1.role2]`
/// shorthand block-attribute line — pure rendering metadata (the HTML backend
/// emits them as a `class`), not part of cross-reference resolution. Empty when
/// none were given. Order is source order.
///
/// `attrs` is the rest of a block-attribute list (`[style, name=value, %option]`):
/// the block `style`, named attributes and `%options`. `id` and `role` are *not*
/// here — they fold into `id` / `roles` above (the canonical anchor/role
/// targets), regardless of whether they came from shorthand (`#id` / `.role`) or
/// the longhand (`id=` / `role=`). See [`BlockAttrs`].
///
/// [`sectids`]: crate::resolve
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub span: Span,
    /// Immutable snapshot of attributes when this block finished parsing.
    /// A container sees assignments made while parsing its children; an already
    /// completed block keeps its own snapshot. Inline values retain the context
    /// in which they were scanned.
    /// Shared until an attribute assignment changes the parse context.
    pub attribute_context: Arc<Attributes>,
    pub id: Option<String>,
    pub reftext: Option<Vec<Inline>>,
    pub roles: Vec<String>,
    /// The block's *title* — a `.Title` line preceding the block (asciidoctor's
    /// [`BlockTitleRx`]). `None` when the block has no title. The text is parsed
    /// inlines (inline substitutions apply, e.g. `.A *bold* caption`), so it
    /// carries formatting. The title lives on the [`Block`] (not inside
    /// [`BlockKind`]) so it survives the type-changing style fixups — a `.Title`
    /// before a `[NOTE]`/`[example]` block stays on the resulting container, not
    /// on the wrapped child.
    /// An unrendered passthrough title keeps its source text unless it was
    /// evaluated during reading for an explicit block ID.
    ///
    /// [`BlockTitleRx`]: crate::parser
    pub title: Option<Vec<Inline>>,
    /// The block's resolved **caption** — the counter prefix a titled block
    /// carries before its [`title`], e.g. `"Example 1. "` (note the trailing
    /// space). Filled at parse completion and *only* for a titled
    /// [`BlockKind::Example`] (asciidoctor numbers example blocks via the
    /// `example-caption` attribute; an untitled example gets no caption and does
    /// not advance the counter). An explicit block/document `caption` replaces
    /// the prefix without advancing the counter, including an empty caption.
    /// `None` for every other block — a titled listing
    /// or paragraph renders its bare title, matching asciidoctor's unset
    /// `listing-caption`. Stored pre-rendered, like asciidoctor's `@caption`
    /// (`#{caption}#{title}`): backends prepend it to the title verbatim.
    ///
    /// [`title`]: Block::title
    pub caption: Option<String>,
    pub attrs: BlockAttrs,
    pub kind: BlockKind,
}

impl Block {
    /// A block with no anchor: `id`/`reftext` default to `None` and `roles` /
    /// `attrs` are empty. The parser fills them in from an explicit `[[id]]` /
    /// `[[id,reftext]]` anchor or a `[style, #id.role, name=value]` attribute
    /// line preceding the block; the resolve pass fills `id` for an auto-id
    /// section.
    pub fn new(span: Span, kind: BlockKind) -> Self {
        Self {
            span,
            attribute_context: Arc::new(Attributes::new()),
            id: None,
            reftext: None,
            roles: Vec::new(),
            title: None,
            caption: None,
            attrs: BlockAttrs::default(),
            kind,
        }
    }
}

/// The non-id, non-role part of a block-attribute list
/// (`[style, positional, name=value, %option]`).
///
/// adoctra keeps the anchor id (`#id` / `id=`) on [`Block::id`] and CSS roles
/// (`.role` / `role=`) on [`Block::roles`], because those drive cross-reference
/// resolution and class output respectively. Everything else from the list lands
/// here:
///
/// - `style` — the first positional attribute (`source`, `literal`, `NOTE`, …).
///   `source` selects a listing's highlight form (its language); `literal` turns
///   a paragraph into a [`BlockKind::Literal`]. Other styles are parsed and
///   carried but do not yet change a block's type.
/// - `named` — `name=value` pairs, deterministic order (a [`BTreeMap`] so JSON /
///   snapshot output stays stable). For a `source`-style block the language
///   positional is folded in here under the `language` key, in core, so every
///   backend reads it the same way.
/// - `options` — `%option` flags, in source order.
/// - `positional` — asciidoctor's positional attributes after the style (and
///   after a `source` language), by field index: named and skipped fields leave
///   an empty slot, and a later attribute line replaces the same index.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockAttrs {
    pub style: Option<String>,
    pub named: BTreeMap<String, String>,
    pub options: Vec<String>,
    pub positional: Vec<String>,
}

/// The five admonition types. An admonition is a callout produced by an
/// uppercase block style (`[NOTE]`, `[TIP]`, `[IMPORTANT]`, `[WARNING]`,
/// `[CAUTION]`) on a paragraph or a `====` block, or by an uppercase label
/// prefix on a paragraph's first line (`NOTE: …`). All forms are
/// case-sensitive, matching asciidoctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmonitionKind {
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

impl AdmonitionKind {
    /// The lowercase identifier (`"note"`, …): the HTML `data-type` and the JSON
    /// `admonition-type`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Tip => "tip",
            Self::Important => "important",
            Self::Warning => "warning",
            Self::Caution => "caution",
        }
    }

    /// The default label (`"Note"`, …): the built-in `<kind>-caption` value.
    pub fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Important => "Important",
            Self::Warning => "Warning",
            Self::Caution => "Caution",
        }
    }

    /// Match an uppercase block style word (`"NOTE"`, …). Case-sensitive: `[note]`
    /// is *not* an admonition (asciidoctor treats it as a custom paragraph style).
    pub fn from_style(style: &str) -> Option<Self> {
        match style {
            "NOTE" => Some(Self::Note),
            "TIP" => Some(Self::Tip),
            "IMPORTANT" => Some(Self::Important),
            "WARNING" => Some(Self::Warning),
            "CAUTION" => Some(Self::Caution),
            _ => None,
        }
    }

    /// Recognise an admonition label at the start of `line`: an uppercase type
    /// word followed by a colon and a single space (`"NOTE: "`, …). Returns the
    /// kind and the byte length of the prefix to strip (incl. the space). `note:`
    /// and `NOTE:` without the trailing space are *not* admonitions, matching
    /// asciidoctor.
    pub fn from_label_prefix(line: &str) -> Option<(Self, usize)> {
        const LABELS: [(&str, AdmonitionKind); 5] = [
            ("NOTE", AdmonitionKind::Note),
            ("TIP", AdmonitionKind::Tip),
            ("IMPORTANT", AdmonitionKind::Important),
            ("WARNING", AdmonitionKind::Warning),
            ("CAUTION", AdmonitionKind::Caution),
        ];
        for (word, kind) in LABELS {
            if let Some(rest) = line.strip_prefix(word) {
                if rest.strip_prefix(": ").is_some() {
                    return Some((kind, word.len() + 2));
                }
            }
        }
        None
    }
}

/// The attribution of a [`BlockKind::Quote`] / [`BlockKind::Verse`] block:
/// asciidoctor's second and third positional attributes
/// (`[quote, author, citetitle]`). Field indexes count named fields, so
/// `[quote, role=x, Source]` carries only a `citetitle`. A nil field is absent
/// (`[quote, , Source]`); a quoted `""` is an empty part. A block with neither
/// part carries `None` rather than an empty `Attribution`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attribution {
    /// The cited author (asciidoctor's `attribution`), rendered before the
    /// `citetitle`. `None` when omitted.
    pub author: Option<Vec<Inline>>,
    /// The cited work (asciidoctor's `citetitle`), rendered inside a `<cite>`.
    /// `None` when omitted.
    pub citetitle: Option<Vec<Inline>>,
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
    /// A listing block (verbatim, no inline substitutions), fenced by `----`.
    /// Rendered as a `listingblock`; a `[source,LANG]` style further selects the
    /// highlight form (see [`BlockAttrs::style`]).
    Listing { text: String },
    /// A literal block: verbatim, no inline substitutions, either fenced by
    /// `....` or produced by a `[literal]` style on a paragraph. Same verbatim
    /// `text` as [`BlockKind::Listing`] but rendered as a `literalblock` — no
    /// language / `<code>` handling.
    Literal { text: String },
    /// A passthrough block: verbatim, fenced by `++++` (or produced by a `[pass]`
    /// style on a paragraph or `--` block). Like [`BlockKind::Literal`] the body
    /// is the raw source `text` with no inline substitutions (`*bold*`, `{attr}`
    /// stay literal) — but unlike it, the backend emits the text **completely
    /// raw**: no HTML escaping and no wrapper element at all (asciidoctor's
    /// `:pass` context), so even a block id/role is dropped on output.
    Passthrough { text: String },
    /// An unordered (`*`/`-`) or ordered (`.`) list. `ordered` selects the
    /// rendered list kind. Nesting is structural: a deeper level is a
    /// [`BlockKind::List`] held in a [`ListItem`]'s `blocks`.
    List { ordered: bool, items: Vec<ListItem> },
    /// An admonition callout (`NOTE`/`TIP`/`IMPORTANT`/`WARNING`/`CAUTION`),
    /// produced by an uppercase block style (`[NOTE]`), an uppercase label prefix
    /// (`NOTE: …`) or an uppercase style on a `====` block. Like asciidoctor, the
    /// body is a parsed *container* of child blocks: the paragraph and label forms
    /// wrap their inlines in a single child [`BlockKind::Paragraph`] (the label
    /// prefix, if any, stripped first), and the delimited form carries the `====`
    /// block's children. `kind` selects the type.
    Admonition {
        kind: AdmonitionKind,
        /// asciidoctor's `textlabel`: the block's `caption` attribute (raw, as
        /// asciidoctor emits it), else the document's `<kind>-caption` while
        /// reading (default `Note`, …) with its own Text/Raw parts. Empty when
        /// that attribute is unset.
        label: Vec<crate::AttributePart>,
        blocks: Vec<Block>,
    },
    /// An example block: a parsed container of child blocks, delimited by `====`
    /// (or produced by an `[example]` style on a paragraph). The first container
    /// kind besides a [`BlockKind::List`] — unlike the verbatim
    /// [`BlockKind::Listing`]/[`BlockKind::Literal`], `blocks` are parsed *as
    /// blocks* (not raw text), so inline substitutions, nested blocks and anchors
    /// all apply inside.
    Example { blocks: Vec<Block> },
    /// A quote block: a parsed container of child blocks, delimited by `____`
    /// (or produced by a `[quote]` style on a paragraph), with an optional
    /// [`Attribution`] from the positional attributes (`[quote, author, work]`).
    /// Like [`BlockKind::Example`] the body is parsed *as blocks* — inline
    /// substitutions, nested blocks and anchors apply inside `blocks`. The default
    /// style of a `____` block is `quote` (asciidoctor), so a bare `____` lands
    /// here; a `[verse]` style on `____` is the verbatim [`BlockKind::Verse`].
    Quote {
        attribution: Option<Attribution>,
        blocks: Vec<Block>,
    },
    /// A verse block: a `[verse]` style on a `____` block or a paragraph. Unlike
    /// the container [`BlockKind::Quote`], a verse is **verbatim** — its source
    /// whitespace and line breaks are preserved (rendered in a `<pre>`) — yet the
    /// normal *inline* substitutions still apply (so `*bold*` formats), which is
    /// why the body is `inlines` (scanned with newlines kept as text) rather than
    /// parsed child blocks or the raw `text` of a [`BlockKind::Literal`]. Carries
    /// the same optional [`Attribution`] as a quote.
    Verse {
        attribution: Option<Attribution>,
        inlines: Vec<Inline>,
    },
    /// A sidebar block: a parsed container of child blocks, delimited by `****`
    /// (or produced by a `[sidebar]` style on a paragraph). Like
    /// [`BlockKind::Example`] the body is parsed *as blocks* — inline
    /// substitutions, nested blocks and anchors all apply inside `blocks`. Unlike
    /// a `====` example, an uppercase admonition style on a `****` block is
    /// *inert* (asciidoctor leaves `[NOTE]` on `****` a plain sidebar), so the
    /// style fixup never folds a sidebar into an [`BlockKind::Admonition`].
    Sidebar { blocks: Vec<Block> },
    /// An open block: a parsed container of child blocks, delimited by `--`
    /// (or produced by an `[open]` style on a paragraph). The *generic*
    /// container — with no style it renders as an `openblock`. A **container**
    /// style "masquerades" the open block into that kind, handled in
    /// `apply_style_fixup`: `[quote]` → [`BlockKind::Quote`], `[example]` →
    /// [`BlockKind::Example`], `[sidebar]` → [`BlockKind::Sidebar`] and an
    /// uppercase admonition style (`[NOTE]`, …) → [`BlockKind::Admonition`],
    /// each moving the parsed `blocks` over unchanged. Like the other
    /// containers the body is parsed *as blocks* — inline substitutions, nested
    /// blocks and anchors apply inside, and the resolve pass recurses in. The
    /// **verbatim** masquerades (`[source]`/`[literal]`/`[verse]` on `--`) switch
    /// the content model at the `--` fence in `parse_block`, producing a
    /// [`BlockKind::Listing`] / [`BlockKind::Literal`] / [`BlockKind::Verse`]
    /// directly rather than an `Open`.
    Open { blocks: Vec<Block> },
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
    /// Entity syntax restored by substitutions, emitted without escaping.
    /// Unknown entity names are retained; `span` covers their source construct.
    Raw { value: String, span: Span },
    /// Semantic text generated by substitutions starting in ordinary source
    /// text. Parts carry neutral formatting/link boundaries, never generated
    /// backend markup. The span covers the consumed source constructs.
    Substituted {
        value: String,
        parts: Vec<crate::AttributePart>,
        span: Span,
    },
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
        /// Empty for ordinary text; otherwise preserves literal/raw parts and
        /// semantic formatting boundaries. `value` omits formatting boundaries.
        parts: Vec<crate::AttributePart>,
        span: Span,
    },
    /// A programmatically constructed link. Parsed links use `Substituted`
    /// with semantic link parts after ordered substitutions. `target`
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
    /// this point. Like Ruby, it is recognized by the macros stage, after quotes,
    /// attribute references and replacements, so the node consumes everything in
    /// its source range (expanded references inside it remain as empty
    /// [`Inline::AttributeRef`] nodes after it). The reftext is not content: the
    /// anchor catalog built while reading supplies it to a textless `<<id>>`.
    /// `span` covers the whole construct (`[[…]]` or `anchor:…[…]`). An anchor
    /// generated entirely by an attribute value stays an
    /// [`AttributePart::Anchor`](crate::AttributePart::Anchor) of its reference.
    ///
    /// A *whole-line* `[[id]]` is the distinct [`block anchor`] construct, handled
    /// by the parser before inline scanning; only an anchor with other text on the
    /// line reaches here.
    ///
    /// [`block anchor`]: crate::ast::Block::id
    Anchor { id: String, span: Span },
    // TODO: block/inline macros (image:, footnote:)...
}

impl Inline {
    pub fn span_mut(&mut self) -> &mut Span {
        match self {
            Inline::Text { span, .. }
            | Inline::Raw { span, .. }
            | Inline::Substituted { span, .. }
            | Inline::Strong { span, .. }
            | Inline::Emphasis { span, .. }
            | Inline::Monospace { span, .. }
            | Inline::Mark { span, .. }
            | Inline::Superscript { span, .. }
            | Inline::Subscript { span, .. }
            | Inline::AttributeRef { span, .. }
            | Inline::Link { span, .. }
            | Inline::CrossReference { span, .. }
            | Inline::Anchor { span, .. } => span,
        }
    }

    /// The source span of this inline node.
    pub fn span(&self) -> Span {
        match self {
            Inline::Text { span, .. }
            | Inline::Raw { span, .. }
            | Inline::Substituted { span, .. }
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

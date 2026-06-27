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
use std::collections::BTreeMap;

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
    pub id: Option<String>,
    pub reftext: Option<Vec<Inline>>,
    pub roles: Vec<String>,
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
            id: None,
            reftext: None,
            roles: Vec::new(),
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
/// - `positional` — positional attributes after the style that core did not map
///   to a named key, verbatim and in source order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlockAttrs {
    pub style: Option<String>,
    pub named: BTreeMap<String, String>,
    pub options: Vec<String>,
    pub positional: Vec<String>,
}

/// The five admonition types. An admonition is a callout paragraph produced
/// either by an uppercase block style (`[NOTE]`, `[TIP]`, `[IMPORTANT]`,
/// `[WARNING]`, `[CAUTION]`) or by an uppercase label prefix on a paragraph's
/// first line (`NOTE: …`). Both forms are case-sensitive, matching asciidoctor.
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

    /// The human-readable label (`"Note"`, …): the rendered title / `aria-label`.
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
    /// An unordered (`*`/`-`) or ordered (`.`) list. `ordered` selects the
    /// rendered list kind. Nesting is structural: a deeper level is a
    /// [`BlockKind::List`] held in a [`ListItem`]'s `blocks`.
    List { ordered: bool, items: Vec<ListItem> },
    /// An admonition callout (`NOTE`/`TIP`/`IMPORTANT`/`WARNING`/`CAUTION`),
    /// produced by an uppercase block style (`[NOTE]`) or an uppercase label
    /// prefix (`NOTE: …`). Carries resolved `inlines` like a [`BlockKind::Paragraph`]
    /// (the label prefix, if any, is stripped before scanning); `kind` selects
    /// the type.
    Admonition {
        kind: AdmonitionKind,
        inlines: Vec<Inline>,
    },
    /// An example block: a parsed container of child blocks, delimited by `====`
    /// (or produced by an `[example]` style on a paragraph). The first container
    /// kind besides a [`BlockKind::List`] — unlike the verbatim
    /// [`BlockKind::Listing`]/[`BlockKind::Literal`], `blocks` are parsed *as
    /// blocks* (not raw text), so inline substitutions, nested blocks and anchors
    /// all apply inside.
    Example { blocks: Vec<Block> },
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

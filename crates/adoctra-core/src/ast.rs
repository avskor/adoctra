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
    /// Effective attributes after body conversion, for document-level output
    /// such as math typesetting configuration. The header snapshot is unchanged.
    pub final_attributes: Attributes,
    pub header: Option<DocumentHeader>,
    pub blocks: Vec<Block>,
    /// Whether `blocks[0]` is the document title: a source `= Title` or a
    /// `doctitle` attribute without one. A leading level-0 heading that is not
    /// the title (for example, under a block title) is a part.
    pub has_title: bool,
    /// The document's footnotes in registration order (Ruby's
    /// `Document#footnotes`), rendered after the body.
    pub footnotes: Vec<FootnoteDefinition>,
    /// A manual page's names (doctype `manpage`), read by the header.
    pub manpage: Option<Manpage>,
}

/// The names of a manual page, like Ruby's `parse_manpage_header`. The header
/// attributes keep the values (`mantitle`, `manvolnum`, `manname`,
/// `mannames`, `manpurpose`, `manname-title`, `manname-id`); this keeps the
/// name list and the NAME section the header consumed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Manpage {
    /// Ruby's `mannames`, in lexical spelling (the attribute holds Ruby's
    /// array inspection).
    pub names: Vec<String>,
    /// The NAME section, when its name and purpose come from the source.
    pub name_section: Option<NameSection>,
    /// Whether `manname-title` is that section's title (the header did not
    /// set it). The attribute holds its flat label parts; the HTML backend
    /// renders the heading's inlines, links included.
    pub section_title: bool,
}

/// The NAME section read by a manual page's header: its title (a level-1
/// heading with the section's id, registered for references) and the range
/// of the name and purpose lines.
#[derive(Debug, Clone, PartialEq)]
pub struct NameSection {
    pub heading: Block,
    /// `None` when the lines do not conform to `name - purpose`: the body
    /// then reads the section again, while its title stays registered under
    /// its id (Ruby's registration before the check), so a duplicate
    /// automatic id gets a suffix.
    pub body: Option<Span>,
}

/// A registered footnote: its number, optional id, converted content and the
/// source range of its definition macro.
#[derive(Debug, Clone, PartialEq)]
pub struct FootnoteDefinition {
    pub index: String,
    pub id: Option<String>,
    pub content: Vec<crate::AttributePart>,
    pub span: Span,
}

impl Document {
    /// The authors of the header attributes, like Ruby's `Document#authors`
    /// as the HTML header converts them: `author`/`email`, then
    /// `author_N`/`email_N` up to `authorcount`, names with replacements and
    /// emails with macros (an address becomes a `mailto:` link).
    pub fn authors(&self) -> Vec<AuthorDetails> {
        crate::author_values::document_authors(&self.attributes)
    }

    /// The `authors` header attribute with replacements, like Ruby's
    /// `sub_replacements` for the `author` meta element.
    pub fn authors_line(&self) -> Option<Vec<crate::AttributePart>> {
        crate::author_values::replaced(&self.attributes, "authors")
    }

    /// The blocks in document order with the section structure flattened: a
    /// [`BlockKind::Section`] contributes its title heading followed by its
    /// content, and a [`BlockKind::Preamble`] its blocks. Other containers are
    /// returned as they are. Useful for consumers that ignore the hierarchy.
    pub fn flat_blocks(&self) -> Vec<&Block> {
        fn push<'a>(blocks: &'a [Block], out: &mut Vec<&'a Block>) {
            for block in blocks {
                match &block.kind {
                    BlockKind::Section { blocks, .. } | BlockKind::Preamble { blocks } => {
                        push(blocks, out)
                    }
                    _ => out.push(block),
                }
            }
        }
        let mut out = Vec::new();
        push(&self.blocks, &mut out);
        out
    }

    /// The owned counterpart of [`Document::flat_blocks`].
    pub fn into_flat_blocks(self) -> Vec<Block> {
        fn push(blocks: Vec<Block>, out: &mut Vec<Block>) {
            for block in blocks {
                match block.kind {
                    BlockKind::Section { blocks, .. } | BlockKind::Preamble { blocks } => {
                        push(blocks, out)
                    }
                    _ => out.push(block),
                }
            }
        }
        let mut out = Vec::new();
        push(self.blocks, &mut out);
        out
    }
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

/// An author as the HTML header shows it ([`Document::authors`]): the
/// effective name after replacements and the email after macros.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorDetails {
    pub name: Vec<crate::AttributePart>,
    pub email: Option<Vec<crate::AttributePart>>,
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
    /// The document attributes when this block converts, which Ruby's
    /// converter reads (`sectanchors`, `prewrap`, `toc-placement`, …): the
    /// header snapshot with the declarations replayed up to the block and the
    /// effects of earlier conversions, taken on entry before its title and
    /// content. Unlike [`attribute_context`](Block::attribute_context), it has
    /// no reading-only effects (orphan declarations, section-title
    /// expressions). `None` until conversion visits the block; see
    /// [`Block::conversion_attributes`].
    pub conversion_context: Option<Arc<Attributes>>,
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
    /// A numbered section's [`BlockKind::Heading`] carries its number prefix
    /// here instead (`"1.2. "`, `"Chapter 3. "`, `"II: "`), computed during
    /// conversion like asciidoctor's `convert_section` (`sectnumlevels` and
    /// the signifiers at the section); backends render it before the inlines.
    /// `None` for every other block — a titled listing
    /// or paragraph renders its bare title, matching asciidoctor's unset
    /// `listing-caption`. Stored pre-rendered, like asciidoctor's `@caption`
    /// (`#{caption}#{title}`): backends prepend it to the title verbatim.
    ///
    /// [`title`]: Block::title
    pub caption: Option<String>,
    /// The caption's semantic parts when it is not plain text, like the
    /// HTML Ruby stores in `@caption`: a caption attribute's formatting or
    /// raw markup (`pass:q[*Ex*]`), a raw unquoted or substituted
    /// single-quoted `caption=`, a signifier. Empty when [`caption`] is plain
    /// text; [`caption`] is always the flat text of these parts.
    ///
    /// [`caption`]: Block::caption
    pub caption_parts: Vec<crate::AttributePart>,
    pub attrs: BlockAttrs,
    /// Ruby's `:simple` content model: a container built from a styled
    /// paragraph (`[NOTE]`/`NOTE:`, `[example]`, `[sidebar]`, `[open]`,
    /// `[abstract]`, `[partintro]`, `[quote]`) holding that paragraph as its
    /// only child. Converters write the paragraph's inlines directly, without
    /// a paragraph element, like Ruby's `content` of a simple block. `false`
    /// for every delimited or other block.
    pub simple: bool,
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
            conversion_context: None,
            id: None,
            reftext: None,
            roles: Vec::new(),
            title: None,
            caption: None,
            caption_parts: Vec::new(),
            attrs: BlockAttrs::default(),
            simple: false,
            kind,
        }
    }

    /// Set the caption from its parts (see [`caption_parts`]).
    ///
    /// [`caption_parts`]: Block::caption_parts
    pub fn set_caption(&mut self, parts: Option<Vec<crate::AttributePart>>) {
        self.caption = parts.as_deref().map(crate::attributes::parts_text);
        self.caption_parts = match parts {
            Some(parts) if !matches!(parts.as_slice(), [] | [crate::AttributePart::Text(_)]) => {
                parts
            }
            _ => Vec::new(),
        };
    }

    /// The attributes a converter reads for this block: its
    /// [`conversion_context`](Block::conversion_context), else (for a block
    /// conversion did not visit) its reading snapshot.
    pub fn conversion_attributes(&self) -> &Attributes {
        self.conversion_context
            .as_deref()
            .unwrap_or(&self.attribute_context)
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
    /// The parts of a single-quoted `caption=` after its substitutions (Ruby
    /// stores the converted markup); another `caption` value is raw.
    pub(crate) caption: Option<Vec<crate::AttributePart>>,
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
    /// Credit text rendered after the `<cite>`. Ruby splits a blockquote
    /// credit line in its converted markup; when the `, ` lies inside an
    /// element, the browser closes the `cite` with that element, so the rest
    /// of the line is not part of the citetitle. Empty otherwise.
    pub trailing: Vec<Inline>,
}

/// Block-level element kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    /// A section title. `level` is the number of leading `=` minus one
    /// (so a document title `= Title` is level 0, `== Section` is level 1).
    /// A body section title is the first block of its [`BlockKind::Section`];
    /// the document title and discrete headings stand alone.
    /// The section's anchor lives on the enclosing [`Block::id`] — auto-generated
    /// from the title during the resolve pass (asciidoctor's `sectids`, e.g.
    /// `== Hello World` → `_hello_world`) unless an explicit `[[id]]` anchor
    /// overrides it. Cross-references resolve against that id.
    Heading { level: u8, inlines: Vec<Inline> },
    /// A run of text forming a paragraph.
    Paragraph { inlines: Vec<Inline> },
    /// A listing block (verbatim, no inline substitutions), fenced by `----`.
    /// Rendered as a `listingblock`; a `[source,LANG]` style further selects the
    /// highlight form (see [`BlockAttrs::style`]). `callouts` are the callout
    /// marks at its line ends (Ruby's verbatim `callouts` substitution).
    Listing {
        text: String,
        callouts: Vec<Callout>,
        /// The content after a `subs` attribute selected other substitutions
        /// than the verbatim defaults (one [`Inline::Substituted`] node);
        /// backends then render it instead of `text`.
        inlines: Option<Vec<Inline>>,
    },
    /// A literal block: verbatim, no inline substitutions, either fenced by
    /// `....` or produced by a `[literal]` style on a paragraph. Same verbatim
    /// `text` as [`BlockKind::Listing`] but rendered as a `literalblock` — no
    /// language / `<code>` handling.
    Literal {
        text: String,
        callouts: Vec<Callout>,
        /// Content after custom substitutions, as for [`BlockKind::Listing`].
        inlines: Option<Vec<Inline>>,
    },
    /// A passthrough block: verbatim, fenced by `++++` (or produced by a `[pass]`
    /// style on a paragraph or `--` block). Like [`BlockKind::Literal`] the body
    /// is the raw source `text` with no inline substitutions (`*bold*`, `{attr}`
    /// stay literal) — but unlike it, the backend emits the text **completely
    /// raw**: no HTML escaping and no wrapper element at all (asciidoctor's
    /// `:pass` context), so even a block id/role is dropped on output.
    /// A `subs` attribute selecting substitutions (Ruby applies none by
    /// default) gives the converted `inlines`, still emitted raw.
    Passthrough {
        text: String,
        inlines: Option<Vec<Inline>>,
    },

    /// A `++++` block styled `stem`, `asciimath` or `latexmath`. Content uses
    /// special-character protection by default; explicit substitutions retain
    /// neutral inline parts and their source ranges.
    Stem {
        style: crate::StemStyle,
        inlines: Vec<Inline>,
    },
    /// An unordered (`*`, `-`, `•`) or ordered (`.`, `1.`, `a.`, `A.`, `i)`,
    /// `I)`) list, parsed like Ruby's `parse_list`: each item's lines are
    /// collected (`read_lines_for_list_item`) and parsed as its blocks, a
    /// paragraph adjacent to the marker line folds into the item text, and a
    /// different marker nests. An ordered list's implicit style (`arabic`,
    /// `loweralpha`, …) is its [`BlockAttrs::style`]; a checklist has the
    /// `checklist` option.
    List { ordered: bool, items: Vec<ListItem> },
    /// A callout list (`<1> text`, `<.> text`), like Ruby's
    /// `parse_callout_list`: each item names the callout marks of the
    /// preceding verbatim blocks with its number (`coids`).
    CalloutList { items: Vec<CalloutListItem> },
    /// A description list (`term:: text`, also `:::`, `::::` and `;;`), like
    /// Ruby's `parse_description_list`: consecutive terms without a
    /// description share the next one. The `horizontal` and `qanda` styles
    /// are in [`BlockAttrs::style`].
    DescriptionList { items: Vec<DescriptionListItem> },
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
        /// asciidoctor emits it, or the substituted parts of a single-quoted
        /// value), else the document's `<kind>-caption` while
        /// reading (default `Note`, …) with its own Text/Raw parts. Empty when
        /// that attribute is unset.
        label: Vec<crate::AttributePart>,
        blocks: Vec<Block>,
        /// The document `icons` mode when the block converts, like Ruby's
        /// `convert_admonition`: `None` shows the label, `font` an icon font
        /// glyph unless the block names an `icon`, any other value an image.
        icons: Option<String>,
        /// The image icon (Ruby's `icon_uri` of the block's `icon` attribute
        /// or the admonition name under `iconsdir`); empty unless an image.
        src: String,
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
    /// A section (asciidoctor's `Section`): the document-level structure built
    /// from section titles, like Ruby's `next_section`. `blocks[0]` is always the
    /// section's title, a [`BlockKind::Heading`] of the same `level` that keeps
    /// the id, reftext, roles and attributes; the remaining blocks are the
    /// section's content, including nested subsections. Discrete headings and
    /// the document title stay plain headings. The span covers the title through
    /// the end of the last nested block.
    Section { level: u8, blocks: Vec<Block> },
    /// asciidoctor's preamble: the blocks between a document header and the first
    /// section. Present when the document has a title and at least one section
    /// (a book keeps it without sections, and opens one without a header unless
    /// it starts with an abstract); otherwise those blocks stay at the top level.
    Preamble { blocks: Vec<Block> },
    /// The `toc::[]` block macro (Ruby's `:toc` block): where the table of
    /// contents goes when `toc-placement` is `macro`. Its named attributes
    /// (`levels`, …) are in [`Block::attrs`]; `id`, `role` and `title` from the
    /// macro's brackets replace the block's metadata. Backends build the outline
    /// from the document's sections.
    Toc,
    /// A thematic break (Ruby's `:thematic_break`): `'''` (three or more
    /// apostrophes) or Markdown's `---`, `***`, `___` with optional uniform
    /// spaces between the characters (`- - -`), indented up to three spaces.
    /// It has no content; backends ignore its metadata, like Ruby.
    ThematicBreak,
    /// A page break (Ruby's `:page_break`): three or more `<` on a line.
    PageBreak,
    /// The `image::target[alt, width, height]` block macro (Ruby's `:image`
    /// block). `target` is the macro target after its attribute references;
    /// `src` is the resolved web path (`imagesdir`, `.`/`..` segments and
    /// spaces, as Ruby's `image_uri` in secure mode) at the block during
    /// conversion. `alt` is Ruby's `alt`: the explicit `alt`, else the style
    /// of a preceding attribute line, else the target's basename with `_`/`-`
    /// as spaces; a non-default value gets replacements (generated entities
    /// stay [`crate::AttributePart::Raw`]). The macro's named attributes (`width`,
    /// `height`, `link`, `window`, `float`, `align`, `scaledwidth`, …) are in
    /// [`Block::attrs`]; a `.Title` makes it a numbered figure
    /// (`figure-caption`, [`Block::caption`]).
    Image {
        target: String,
        src: String,
        alt: Vec<crate::AttributePart>,
    },
    /// The `video::target[poster, width, height]` block macro (Ruby's `:video`
    /// block). `target` is the macro target after its attribute references.
    /// `src` is resolved at the block during conversion, as Ruby's
    /// `convert_video` builds it: a `poster` of `youtube`/`vimeo` makes it an
    /// `embedded` player URL (`asset-uri-scheme`, the target's list/playlist or
    /// hash and the `start`/`end`/`theme`/`lang` attributes and options, with
    /// literal `&` separators); otherwise it is the target's web path under
    /// `imagesdir` with a `#t=start,end` anchor, and a non-empty `poster` is
    /// resolved the same way. Named attributes and options stay in
    /// [`Block::attrs`]; a title is not captioned.
    Video {
        target: String,
        src: String,
        poster: Option<String>,
        embedded: bool,
    },
    /// The `audio::target[]` block macro (Ruby's `:audio` block): `src` is the
    /// target's web path under `imagesdir` with a `#t=start,end` anchor,
    /// resolved during conversion.
    Audio { target: String, src: String },
    /// A table (Ruby's `:table` block), delimited by `|===`, `,===`, `:===` or
    /// `!===` (a longer `=` run must close with the same line). See [`Table`].
    Table(Box<Table>),
}

/// A table's columns and rows, parsed like Ruby's `parse_table`. The `cols`
/// attribute, `format`/`separator`, `header`/`noheader`/`footer` and
/// `autowidth` options stay in [`Block::attrs`]; a `.Title` makes it a numbered
/// table (`table-caption`, [`Block::caption`]).
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub columns: Vec<TableColumn>,
    /// The header row (explicit `%header`, or Ruby's implicit header: a first
    /// line holding the whole first row followed by a blank line).
    pub head: Vec<Vec<TableCell>>,
    pub body: Vec<Vec<TableCell>>,
    /// The last row with `%footer`.
    pub foot: Vec<Vec<TableCell>>,
    /// Ruby's `tablepcwidth`: the `width` attribute as an integer percentage
    /// (1–100, or 0 for an explicit `0`/`0%`), else 100.
    pub width: u32,
    /// Ruby's `frame`, `grid` and `stripes` with the `table-frame`,
    /// `table-grid` and `table-stripes` document fallbacks at the table during
    /// conversion (`topbot` becomes `ends`; defaults `all`).
    pub frame: String,
    pub grid: String,
    pub stripes: Option<String>,
}

impl Table {
    /// Every cell in Ruby's conversion order: head, body, foot.
    pub fn cells(&self) -> impl Iterator<Item = &TableCell> {
        self.head
            .iter()
            .chain(&self.body)
            .chain(&self.foot)
            .flatten()
    }

    pub fn cells_mut(&mut self) -> impl Iterator<Item = &mut TableCell> {
        self.head
            .iter_mut()
            .chain(&mut self.body)
            .chain(&mut self.foot)
            .flatten()
    }
}

/// A table column (Ruby's `Table::Column`) from the `cols` specs, or an equal
/// share of the width for columns implied by the first row.
#[derive(Debug, Clone, PartialEq)]
pub struct TableColumn {
    pub halign: HAlign,
    pub valign: VAlign,
    pub style: Option<CellStyle>,
    /// Ruby's `colpcwidth`, spelled as Ruby prints the number (`25`,
    /// `33.3333`, `50.0001`).
    pub width: String,
    /// A `~` width (Ruby's `autowidth-option`).
    pub autowidth: bool,
}

/// A table cell (Ruby's `Table::Cell`). The alignments and style combine the
/// column's with the cell spec's; header-row cells have no style.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    pub content: CellContent,
    pub style: Option<CellStyle>,
    pub halign: HAlign,
    pub valign: VAlign,
    pub colspan: Option<u32>,
    pub rowspan: Option<u32>,
    /// The `cellbgcolor` document attribute after converting the cell's
    /// content (a `{set:cellbgcolor:…}` in the cell applies to it).
    pub background: Option<String>,
    /// From the cell spec (or the separator) through the end of the cell text.
    pub span: Span,
}

/// What a cell holds.
#[derive(Debug, Clone, PartialEq)]
pub enum CellContent {
    /// Normal substitutions. Body and footer cells split into paragraphs at
    /// blank lines (Ruby's `Cell#content`); a header-row cell is one run.
    Paragraphs(Vec<Vec<Inline>>),
    /// An AsciiDoc (`a`) cell: the blocks of its nested document (Ruby's
    /// `inner_document`), read when the cell is created with the parent's
    /// attributes locked and converted with its own attribute entries, and
    /// that document's own footnotes.
    Blocks {
        blocks: Vec<Block>,
        footnotes: Vec<FootnoteDefinition>,
        /// The nested document has a title: `blocks[0]` is its level-0
        /// heading (like [`Document::has_title`]), shown unless `notitle`
        /// (the nested default, which its header entries may unset).
        has_title: bool,
        /// The nested document's header attributes, which Ruby's
        /// `convert_embedded` reads (its automatic TOC).
        attributes: Arc<Attributes>,
    },
    /// A literal (`l`) cell: verbatim text (Ruby's basic substitutions are
    /// output escaping).
    Literal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

impl HAlign {
    pub fn as_str(self) -> &'static str {
        match self {
            HAlign::Left => "left",
            HAlign::Center => "center",
            HAlign::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VAlign {
    Top,
    Middle,
    Bottom,
}

impl VAlign {
    pub fn as_str(self) -> &'static str {
        match self {
            VAlign::Top => "top",
            VAlign::Middle => "middle",
            VAlign::Bottom => "bottom",
        }
    }
}

/// Ruby's `TableCellStyles`: `d`, `s`, `e`, `m`, `h`, `l` and `a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStyle {
    Default,
    Strong,
    Emphasis,
    Monospaced,
    Header,
    Literal,
    Asciidoc,
}

impl CellStyle {
    pub fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'd' => CellStyle::Default,
            's' => CellStyle::Strong,
            'e' => CellStyle::Emphasis,
            'm' => CellStyle::Monospaced,
            'h' => CellStyle::Header,
            'l' => CellStyle::Literal,
            'a' => CellStyle::Asciidoc,
            _ => return None,
        })
    }

    /// Ruby's style symbol (`none` for `d`).
    pub fn as_str(self) -> &'static str {
        match self {
            CellStyle::Default => "none",
            CellStyle::Strong => "strong",
            CellStyle::Emphasis => "emphasis",
            CellStyle::Monospaced => "monospaced",
            CellStyle::Header => "header",
            CellStyle::Literal => "literal",
            CellStyle::Asciidoc => "asciidoc",
        }
    }
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
    /// A checklist item's state (`[x]`/`[*]` checked, `[ ]` not).
    pub checkbox: Option<bool>,
    /// Ruby's `ListItem#marker`: the unordered marker (`*`, `**`, `-`) or the
    /// first marker of the ordered series (`.`, `..`, `1.`, `a.`, `i)`, …);
    /// none for a description.
    pub marker: Option<String>,
}

/// A callout mark at the end of a verbatim line (`<1>`, `<.>`, `<!--1-->`),
/// like Ruby's `sub_callouts`: `range` is the replaced bytes of the block
/// text, from any comment `guard` (`// `, `# `, `-- `, `;; ` or the
/// `line-comment` attribute) through the mark; `xml` marks the `<!--N-->`
/// form. `id` is Ruby's callout id (`CO1-2`). `span` is the mark's source.
/// `icons` is the document `icons` mode when the block converts and `src` the
/// image icon (`icon_uri "callouts/N"`) when icons are images.
#[derive(Debug, Clone, PartialEq)]
pub struct Callout {
    pub start: usize,
    pub end: usize,
    pub number: String,
    pub guard: Option<String>,
    pub xml: bool,
    pub id: String,
    pub span: Span,
    pub icons: Option<String>,
    pub src: String,
}

/// One item of a [`BlockKind::CalloutList`] and the ids of the callout marks
/// it explains (Ruby's `coids`). `icons` and `src` are the item number's
/// icon when the list converts, as for [`Callout`].
#[derive(Debug, Clone, PartialEq)]
pub struct CalloutListItem {
    pub item: ListItem,
    pub coids: Vec<String>,
    pub icons: Option<String>,
    pub src: String,
}

/// One entry of a [`BlockKind::DescriptionList`]: its terms and the shared
/// description (text and blocks, like a [`ListItem`]), if any.
#[derive(Debug, Clone, PartialEq)]
pub struct DescriptionListItem {
    pub terms: Vec<ListTerm>,
    pub description: Option<ListItem>,
    pub span: Span,
}

/// A description list term.
#[derive(Debug, Clone, PartialEq)]
pub struct ListTerm {
    pub inlines: Vec<Inline>,
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
    /// Like Ruby, it is recognized by the macros stage (after links, emails and
    /// inline anchors) over the result of quotes, attribute references and
    /// replacements, so `<<{id}>>` takes the expanded id and a consumed label
    /// keeps its formatting as [`Inline::Substituted`] parts.
    /// `xref` holds the resolved target (decoded; character references keep
    /// their spelling, as Ruby inserts them): `#id` within this document
    /// (a leading `#` removed outside compat mode; an unknown bare id may name
    /// a title or reftext), or another document's path (`doc.adoc#id`,
    /// `xref:doc.adoc[]`) with `relfileprefix`/`relfilesuffix` applied, unless
    /// it names this document or an included one. `role` comes from the
    /// macro's attribute list (`xref:id[text,role=r]`). `children` is the
    /// display text. The parser leaves it *empty* for a textless reference;
    /// conversion then fills it in — with the target's reftext or title
    /// (formatting preserved, like asciidoctor's `xrefstyle=basic`), the
    /// interdocument path, or the literal `[id]` text when the id is unknown
    /// (asciidoctor's *unresolved*-reference rendering, which also emits a
    /// diagnostic). An explicit reftext is kept verbatim.
    /// `span` covers the whole construct (the `<<…>>` or `xref:…[…]`). A
    /// reference generated entirely by an attribute value stays a pair of
    /// [`AttributePart::OpenXref`](crate::AttributePart::OpenXref) /
    /// `CloseXref` parts of its reference.
    CrossReference {
        xref: crate::Xref,
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
    /// An inline `image:target[alt, width, height]` or `icon:name[size]` macro,
    /// recognized like Ruby's `InlineImageMacroRx`: in the macros stage before
    /// links, over the lexical result of quotes, attribute references and
    /// replacements. Its fields keep their lexical spelling (see
    /// [`crate::InlineImage`]); `span` covers the whole macro, and consumed
    /// references remain empty [`Inline::AttributeRef`] nodes after it. An image
    /// generated entirely by an attribute value stays an
    /// [`AttributePart::Image`](crate::AttributePart::Image) of its reference.
    Image {
        image: crate::InlineImage,
        span: Span,
    },
    /// A footnote macro (`footnote:[text]`, `footnote:id[text]`,
    /// `footnote:id[]`, legacy `footnoteref:[id,text]`) recognized like Ruby's
    /// `InlineFootnoteMacroRx`, the last macro of the macros stage, and
    /// numbered when converted (`footnote-number`). `span` covers the macro.
    /// A footnote generated entirely by an attribute value stays an
    /// [`AttributePart::Footnote`](crate::AttributePart::Footnote).
    Footnote {
        footnote: crate::Footnote,
        span: Span,
    },
    /// A keyboard (`kbd:[Ctrl+T]`), button (`btn:[OK]`) or menu
    /// (`menu:File[Save]`, `"File > Save"`) macro, recognized only when the
    /// `experimental` attribute is set, like Ruby: first in the macros stage
    /// (before images), over the lexical result of the earlier stages. `span`
    /// covers the macro. One generated entirely by an attribute value stays an
    /// [`AttributePart::Ui`](crate::AttributePart::Ui).
    Ui { ui: crate::UiMacro, span: Span },
    /// An index term (`((Tigers))`, `(((Big cats,Tigers)))`,
    /// `indexterm2:[Tigers]`, `indexterm:[Big cats,Tigers]`), recognized like
    /// Ruby's `InlineIndextermMacroRx` in the macros stage after images. A
    /// flow term shows its text, a concealed term nothing. `span` covers the
    /// construct; one generated entirely by an attribute value stays an
    /// [`AttributePart::IndexTerm`](crate::AttributePart::IndexTerm).
    IndexTerm { term: crate::IndexTerm, span: Span },
    /// A protected inline formula (`stem:`, `asciimath:` or `latexmath:`),
    /// extracted before quotes/attributes, restored after the outer subs.
    Stem { stem: crate::Stem, span: Span },
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
            | Inline::Anchor { span, .. }
            | Inline::Image { span, .. }
            | Inline::Footnote { span, .. }
            | Inline::Ui { span, .. }
            | Inline::IndexTerm { span, .. }
            | Inline::Stem { span, .. } => span,
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
            | Inline::Anchor { span, .. }
            | Inline::Image { span, .. }
            | Inline::Footnote { span, .. }
            | Inline::Ui { span, .. }
            | Inline::IndexTerm { span, .. }
            | Inline::Stem { span, .. } => *span,
        }
    }
}

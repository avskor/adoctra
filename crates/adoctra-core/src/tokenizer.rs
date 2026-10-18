//! Tokenizer for syntax highlighting.
//!
//! Produces a flat list of [`Token`]s with byte spans, classifying the
//! "interesting" parts of the source (headings, attribute entries, listing
//! delimiters/content, inline `*strong*` / `_emphasis_` / `{attribute}`).
//! Plain text is left untokenized.
//!
//! Inline tokens are a *projection of the parser's inline scan*: the same
//! [`scan_inlines`] that builds the document model is flattened to tokens here,
//! so highlighting can never disagree with parsing about where a `*strong*`
//! starts. There is no second grammar.

use crate::ast::{AdmonitionKind, Inline, Span};
use crate::context::{ParseContext, ParseOptions};
use crate::header::collect_header;
use crate::inline::scan_for_tokens as scan_inlines;
use crate::parser::{parse_block_anchor, parse_block_title};
use crate::scanner::lines_with_offsets;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub span: Span,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Comment,
    ConditionalDirective,
    IncludeDirective,
    Inactive,
    Heading,
    Author,
    Revision,
    Anchor,
    BlockAttributeList,
    AttributeEntry,
    ListingDelimiter,
    ListingText,
    LiteralDelimiter,
    LiteralText,
    PassthroughDelimiter,
    PassthroughText,
    StemDelimiter,
    StemText,
    ExampleDelimiter,
    QuoteDelimiter,
    SidebarDelimiter,
    OpenDelimiter,
    TableDelimiter,
    CellSeparator,
    BlockTitle,
    AdmonitionLabel,
    Strong,
    Emphasis,
    Monospace,
    Mark,
    Superscript,
    Subscript,
    AttributeRef,
    Link,
    CrossReference,
    ListMarker,
    LineBreak,
    BlockMacro,
    ThematicBreak,
    PageBreak,
    InlineMacro,
    Callout,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Comment => "comment",
            TokenKind::ConditionalDirective => "conditional-directive",
            TokenKind::IncludeDirective => "include-directive",
            TokenKind::Inactive => "inactive",
            TokenKind::Heading => "heading",
            TokenKind::Author => "author",
            TokenKind::Revision => "revision",
            TokenKind::Anchor => "anchor",
            TokenKind::BlockAttributeList => "block-attribute-list",
            TokenKind::AttributeEntry => "attribute-entry",
            TokenKind::ListingDelimiter => "listing-delimiter",
            TokenKind::ListingText => "listing-text",
            TokenKind::LiteralDelimiter => "literal-delimiter",
            TokenKind::LiteralText => "literal-text",
            TokenKind::PassthroughDelimiter => "pass-delimiter",
            TokenKind::PassthroughText => "pass-text",
            TokenKind::StemDelimiter => "stem-delimiter",
            TokenKind::StemText => "stem-text",
            TokenKind::ExampleDelimiter => "example-delimiter",
            TokenKind::QuoteDelimiter => "quote-delimiter",
            TokenKind::SidebarDelimiter => "sidebar-delimiter",
            TokenKind::OpenDelimiter => "open-delimiter",
            TokenKind::TableDelimiter => "table-delimiter",
            TokenKind::CellSeparator => "cell-separator",
            TokenKind::BlockTitle => "block-title",
            TokenKind::AdmonitionLabel => "admonition-label",
            TokenKind::Strong => "strong",
            TokenKind::Emphasis => "emphasis",
            TokenKind::Monospace => "monospace",
            TokenKind::Mark => "mark",
            TokenKind::Superscript => "superscript",
            TokenKind::Subscript => "subscript",
            TokenKind::AttributeRef => "attribute-ref",
            TokenKind::Link => "link",
            TokenKind::CrossReference => "xref",
            TokenKind::ListMarker => "list-marker",
            TokenKind::LineBreak => "line-break",
            TokenKind::BlockMacro => "block-macro",
            TokenKind::ThematicBreak => "thematic-break",
            TokenKind::PageBreak => "page-break",
            TokenKind::InlineMacro => "inline-macro",
            TokenKind::Callout => "callout",
        }
    }
}

pub fn tokens_with_options(source: &str, options: &ParseOptions) -> Vec<Token> {
    tokens_reader(options, crate::reader::Reader::new(source))
}

pub(crate) fn tokens_reader<'a>(
    options: &ParseOptions,
    reader: crate::reader::Reader<'a>,
) -> Vec<Token> {
    let (mut result, reader) = crate::parser::parse_reader(options, reader);
    let processed = reader.content(0, usize::MAX);
    let comments: Vec<_> = reader
        .comments
        .iter()
        .map(|span| processed.transformed_span(*span))
        .collect();
    let entries: Vec<_> = reader
        .attribute_entries
        .iter()
        .map(|(span, entry)| (processed.transformed_span(*span), entry.clone()))
        .collect();
    let headings: Vec<_> = reader
        .heading_lines
        .iter()
        .map(|span| processed.transformed_span(*span))
        .collect();
    let mut tables = Tables {
        delimited: reader
            .block_tokens
            .iter()
            .map(|t| Token {
                span: processed.transformed_span(t.span),
                kind: t.kind,
            })
            .collect(),
        blocks: reader
            .tables
            .iter()
            .map(|span| processed.transformed_span(*span))
            .collect(),
        separators: reader
            .table_separators
            .iter()
            .map(|span| processed.transformed_span(*span))
            .collect(),
        list_markers: reader
            .list_markers
            .iter()
            .map(|span| processed.transformed_span(*span))
            .collect(),
    };
    tables.delimited.sort_by_key(|t| t.span.start);
    let custom: Vec<_> = reader
        .custom_subs
        .iter()
        .map(|(span, stages)| (processed.transformed_span(*span), stages))
        .collect();
    let mut out: Vec<_> = tokens_processed(
        &processed.text,
        options,
        &comments,
        &entries,
        &headings,
        &tables,
    )
    .into_iter()
    // Text with custom substitutions keeps the inline tokens of executed stages.
    .filter(|token| {
        custom
            .iter()
            .find(|(span, _)| span.start <= token.span.start && token.span.end <= span.end)
            .and_then(|(_, stages)| Some(stages.contains(&inline_stage(token.kind)?)))
            .unwrap_or(true)
    })
    .flat_map(|token| {
        reader
            .token_spans(processed.original_span(token.span))
            .into_iter()
            .map(move |span| Token {
                span,
                kind: token.kind,
            })
    })
    .collect();
    for &span in &reader.callout_marks {
        out.extend(reader.token_spans(span).into_iter().map(|span| Token {
            span,
            kind: TokenKind::Callout,
        }));
    }
    for &span in &reader.metadata_references {
        out.extend(reader.token_spans(span).into_iter().map(|span| Token {
            span,
            kind: TokenKind::AttributeRef,
        }));
    }
    // Project source-owned breaks and executed expressions from the complete
    // parse. The syntax-only scan must not execute body expressions again.
    // The line-based highlighting scan cannot know a block's final line or local hardbreaks
    // option. Generated attribute content retains its attribute-ref token;
    // passthroughs retain their existing outer token.
    struct SourceTokens(Vec<(Span, TokenKind)>, Vec<Span>);
    impl crate::visit::VisitorMut for SourceTokens {
        // Callout marks at the ends of verbatim lines.
        fn visit_block(&mut self, block: &mut crate::Block) {
            if let crate::BlockKind::Listing { callouts, .. }
            | crate::BlockKind::Literal { callouts, .. } = &block.kind
            {
                for callout in callouts {
                    self.0.push((callout.span, TokenKind::Callout));
                }
            }
            crate::visit::walk_block_mut(self, block);
        }

        fn visit_inline(&mut self, inline: &mut Inline) {
            use crate::AttributePart as P;
            if let Inline::Substituted { parts, span, .. } = inline {
                // Bibliographic anchors include their visible bracketed label
                // in the same substituted value. Use the executed source span;
                // a syntax-only line scan has no parent-list context.
                if parts.iter().any(|p| matches!(p, P::Anchor(_))) {
                    self.0.push((*span, TokenKind::Anchor));
                }
                if parts.contains(&P::LineBreak)
                    && parts.iter().all(|p| match p {
                        P::LineBreak => true,
                        P::Text(s) => s.chars().all(char::is_whitespace),
                        _ => false,
                    })
                {
                    self.0.push((*span, TokenKind::LineBreak));
                }
            }
            // Footnotes are numbered during conversion; the syntax-only scan
            // leaves their macros as text.
            if let Inline::Footnote { span, .. } = inline {
                self.0.push((*span, TokenKind::InlineMacro));
            }
            // The complete parse sees formulas spanning physical lines; the
            // line scanner must not highlight protected contents as quotes.
            if let Inline::Stem { span, .. } = inline {
                self.0.push((*span, TokenKind::InlineMacro));
                self.1.push(*span);
            }
            if let Inline::AttributeRef { name, span, .. } = inline {
                if name.starts_with("set:")
                    || name.starts_with("counter:")
                    || name.starts_with("counter2:")
                {
                    self.0.push((*span, TokenKind::AttributeRef));
                }
            }
            crate::visit::walk_inline_mut(self, inline);
        }
    }
    let mut source_tokens = SourceTokens(Vec::new(), Vec::new());
    crate::visit::walk_document_mut(&mut source_tokens, &mut result.document);
    // A transformed doctitle carries its entire declaration as origin. Reject
    // that origin before splitting provenance: fragments may include newlines
    // beyond the individual attribute-entry tokens, or cross source files.
    let declarations: std::collections::BTreeSet<_> = out
        .iter()
        .filter(|t| t.kind == TokenKind::AttributeEntry)
        .map(|t| (t.span.source_id, t.span.start))
        .collect();
    let mut protected: Vec<_> = out
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::PassthroughText | TokenKind::StemText))
        .map(|t| t.span)
        .collect();
    protected.sort_by_key(|s| (s.source_id, s.start, s.end));
    let covered_by_pass = |span: Span| {
        let end =
            protected.partition_point(|s| (s.source_id, s.start) <= (span.source_id, span.start));
        protected[..end]
            .last()
            .is_some_and(|s| s.source_id == span.source_id && s.end >= span.end)
    };
    let mut formulas = Vec::new();
    for span in source_tokens.1 {
        if declarations.contains(&(span.source_id, span.start)) {
            continue;
        }
        let fragments = span.provenance.map(|p| result.provenance[p].as_slice());
        formulas.extend(
            fragments
                .unwrap_or(std::slice::from_ref(&span))
                .iter()
                .copied()
                .filter(|span| !covered_by_pass(*span)),
        );
    }
    formulas.sort_by_key(|s| (s.source_id, s.start, s.end));
    out.retain(|token| {
        let span = token.span;
        let end =
            formulas.partition_point(|s| (s.source_id, s.start) <= (span.source_id, span.start));
        !formulas[..end].last().is_some_and(|s| {
            s.source_id == span.source_id
                && s.end >= span.end
                && !(token.kind == TokenKind::InlineMacro && *s == span)
        })
    });
    for (span, kind) in source_tokens.0 {
        if declarations.contains(&(span.source_id, span.start)) {
            continue;
        }
        let fragments = span.provenance.map(|p| result.provenance[p].as_slice());
        for &span in fragments.unwrap_or(std::slice::from_ref(&span)) {
            let end = protected
                .partition_point(|s| (s.source_id, s.start) <= (span.source_id, span.start));
            let covered = protected[..end]
                .last()
                .is_some_and(|s| s.source_id == span.source_id && s.end >= span.end);
            if span.start < span.end && !covered {
                out.push(Token { span, kind });
            }
        }
    }
    for (spans, kind) in [
        (
            result.conditional_directives,
            TokenKind::ConditionalDirective,
        ),
        (result.include_directives, TokenKind::IncludeDirective),
        (result.inactive, TokenKind::Inactive),
    ] {
        out.extend(spans.into_iter().map(|span| Token { span, kind }));
    }
    out.sort_by_key(|token| (token.span.source_id, token.span.start, token.span.end));
    out.dedup();
    out
}

/// The substitution that produces an inline token, if any.
fn inline_stage(kind: TokenKind) -> Option<crate::pass_subs::Stage> {
    use crate::pass_subs::Stage;
    match kind {
        TokenKind::Strong
        | TokenKind::Emphasis
        | TokenKind::Monospace
        | TokenKind::Mark
        | TokenKind::Superscript
        | TokenKind::Subscript => Some(Stage::Quotes),
        TokenKind::AttributeRef => Some(Stage::Attributes),
        TokenKind::Link
        | TokenKind::CrossReference
        | TokenKind::Anchor
        | TokenKind::InlineMacro
        | TokenKind::PassthroughText => Some(Stage::Macros),
        TokenKind::LineBreak => Some(Stage::Post),
        _ => None,
    }
}

/// Table blocks and their cell separators, and list markers (description
/// list delimiters too), from the parser.
struct Tables {
    delimited: Vec<Token>,
    blocks: Vec<Span>,
    separators: Vec<Span>,
    list_markers: Vec<Span>,
}

fn tokens_processed(
    source: &str,
    options: &ParseOptions,
    comments: &[Span],
    entries: &[(Span, Option<crate::context::AttributeEntry>)],
    // Body section title lines (ATX, setext title and underline), classified by
    // the structural parser: only it knows section contexts and paragraph
    // continuation, and a setext underline may look like a fence.
    headings: &[Span],
    tables: &Tables,
) -> Vec<Token> {
    // Comment classification is owned by the structural parser, including
    // verbatim styles and list continuations. Do not duplicate those rules here.
    let mut comment_index = 0;
    let mut entry_index = 0;
    let lines = lines_with_offsets(source);
    // Resolve `{attr}` against the same header table the parser uses.
    let mut context = ParseContext::new(options);
    let header = collect_header(&mut crate::reader::Reader::plain(source), &mut context);
    context.start_body();
    let mut out = Vec::new();
    // Whether the *previous* line continued a running paragraph (or list item).
    // Captured per line as `at_para_start` so an admonition label (`NOTE: `) is
    // only recognised at a paragraph start, exactly like the parser — a `NOTE:`
    // folded into a running paragraph is plain text.
    let mut mid_paragraph = false;

    for (line_index, (off, line)) in lines.into_iter().enumerate() {
        let line_span = Span::new(off, off + line.len());
        while entry_index < entries.len() && entries[entry_index].0.end <= off {
            entry_index += 1;
        }
        if let Some((span, entry)) = entries
            .get(entry_index)
            .filter(|(span, _)| span.start == off)
        {
            if line_index >= header.body_start {
                if let Some(entry) = entry {
                    context.apply(entry.clone());
                }
            }
            out.push(Token {
                span: *span,
                kind: TokenKind::AttributeEntry,
            });
            mid_paragraph = false;
            continue;
        }
        while comment_index < comments.len() && comments[comment_index].end <= off {
            comment_index += 1;
        }
        if let Some(span) = comments
            .get(comment_index)
            .filter(|span| span.start <= off && off < span.end)
        {
            if span.start == off {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Comment,
                });
            }
            if !crate::comments::is_line_comment(line) {
                mid_paragraph = false;
            }
            continue;
        }
        if header.title_line == Some(line_index)
            || header.title_underline == Some(line_index)
            || headings
                .binary_search_by_key(&off, |span| span.start)
                .is_ok()
        {
            out.push(Token {
                span: line_span,
                kind: TokenKind::Heading,
            });
            mid_paragraph = false;
            continue;
        }
        if header.author_line == Some(line_index) || header.revision_line == Some(line_index) {
            out.push(Token {
                span: line_span,
                kind: if header.author_line == Some(line_index) {
                    TokenKind::Author
                } else {
                    TokenKind::Revision
                },
            });
            continue;
        }
        // Delimited block fences and verbatim/raw bodies come from the parser,
        // including blocks in list continuations and AsciiDoc table cells. Only
        // it knows which fence closes a block; never reinterpret verbatim text
        // or raw math as formatting, comments or attribute entries.
        let delimited_start = tables.delimited.partition_point(|t| t.span.end <= off);
        let delimited_end = tables
            .delimited
            .partition_point(|t| t.span.start < line_span.end);
        let delimited = &tables.delimited[delimited_start..delimited_end];
        if !delimited.is_empty() {
            out.extend(
                tables
                    .separators
                    .iter()
                    .filter(|s| s.start >= off && s.end <= delimited[0].span.start)
                    .map(|span| Token {
                        span: *span,
                        kind: TokenKind::CellSeparator,
                    }),
            );
            out.extend_from_slice(delimited);
            mid_paragraph = false;
            continue;
        }
        // A table, as the parser read it: its fences, then each line's cell
        // separators (with their specs) and the cell text between them.
        // The innermost table: a nested one inside an AsciiDoc cell.
        if let Some(table) = tables
            .blocks
            .iter()
            .filter(|t| t.start <= off && off < t.end)
            .max_by_key(|t| t.start)
        {
            mid_paragraph = false;
            if (off == table.start || off + line.len() == table.end)
                && crate::table::delimiter(line).is_some()
            {
                out.push(Token {
                    span: line_span,
                    kind: TokenKind::TableDelimiter,
                });
                continue;
            }
            let mut at = off;
            let first = tables.separators.partition_point(|s| s.start < off);
            for separator in tables.separators[first..]
                .iter()
                .take_while(|s| s.end <= off + line.len())
            {
                out.extend(scan_inlines(
                    &line[at - off..separator.start - off],
                    at,
                    &context.render_attributes(),
                ));
                out.push(Token {
                    span: *separator,
                    kind: TokenKind::CellSeparator,
                });
                at = separator.end;
            }
            out.extend(scan_inlines(
                &line[at - off..],
                at,
                &context.render_attributes(),
            ));
            continue;
        }
        let at_para_start = !mid_paragraph;
        // Default: structural / blank lines (all of which `continue` below) leave
        // the next line a fresh block. The body-text and list-item branches set it
        // back to `true`.
        mid_paragraph = false;

        if line.trim().is_empty() {
            continue;
        }
        // A `[[id]]` / `[[id,reftext]]` block anchor: one token over the whole
        // line. Recognised the same way the parser attaches it to the next block,
        // so highlighting and parsing never disagree.
        if parse_block_anchor(line).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::Anchor,
            });
            continue;
        }
        // A whole-line block-attribute list (`[style, #id.role, name=value]`):
        // one token over the line, recognised exactly as the parser consumes it,
        // so highlighting and parsing never disagree.
        if crate::block_attributes::attr_list_inner(line).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::BlockAttributeList,
            });
            continue;
        }
        // A `.Title` line: one token over the whole line, recognised with the same
        // helper the parser uses to attach the title to the next block (so
        // highlighting and parsing never disagree). Like the anchor / attribute
        // lines it leaves the next line a fresh block start, and like the block
        // anchor's reftext its inline markup is not sub-tokenized.
        if parse_block_title(line).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::BlockTitle,
            });
            continue;
        }
        // A `toc::[]` or `image::target[]` block macro line at a block start,
        // recognized with the parser's grammar (inside a paragraph it stays text).
        if at_para_start
            && (crate::parser::parse_toc_macro(line.trim_end(), off).is_some()
                || crate::image::match_macro(line.trim_end()).is_some())
        {
            out.push(Token {
                span: Span::new(off, off + line.trim_end().len()),
                kind: TokenKind::BlockMacro,
            });
            continue;
        }
        // List items, as the parser read them: each marker (or description
        // delimiter) is a token and the text around it is scanned like any
        // other body line, so token spans coincide with the item's inlines.
        let first = tables.list_markers.partition_point(|s| s.start < off);
        let markers: Vec<Span> = tables.list_markers[first..]
            .iter()
            .take_while(|s| s.end <= off + line.len())
            .copied()
            .collect();
        if !markers.is_empty() {
            mid_paragraph = true; // a folded line after the marker is item text
            let mut at = off;
            for marker in markers {
                out.extend(scan_inlines(
                    &line[at - off..marker.start - off],
                    at,
                    &context.render_attributes(),
                ));
                out.push(Token {
                    span: marker,
                    kind: TokenKind::ListMarker,
                });
                at = marker.end;
            }
            out.extend(scan_inlines(
                &line[at - off..],
                at,
                &context.render_attributes(),
            ));
            continue;
        }
        // Body text line: flatten the shared inline scan to highlight tokens. An
        // admonition label (`NOTE: `, …) at a paragraph start is one token, then
        // the rest of the line is scanned normally — matching the parser's label
        // form (recognised only at a paragraph start, never mid-paragraph).
        if at_para_start {
            if let Some((_, prefix_len)) = AdmonitionKind::from_label_prefix(line) {
                // The token covers `NOTE:` (label + colon); the trailing space and
                // the body are scanned/untokenized as usual.
                out.push(Token {
                    span: Span::new(off, off + prefix_len - 1),
                    kind: TokenKind::AdmonitionLabel,
                });
                out.extend(scan_inlines(
                    &line[prefix_len..],
                    off + prefix_len,
                    &context.render_attributes(),
                ));
                mid_paragraph = true;
                continue;
            }
        }
        out.extend(scan_inlines(line, off, &context.render_attributes()));
        mid_paragraph = true;
    }

    for token in &mut out {
        token.span.source_id = options.source_id;
    }
    out
}

/// Project the inline AST onto highlight tokens (depth-first, document order).
pub(crate) fn flatten_inlines(inlines: &[Inline], out: &mut Vec<Token>) {
    for node in inlines {
        match node {
            Inline::Text { .. } | Inline::Raw { .. } | Inline::Substituted { .. } => {}
            Inline::Strong { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Strong,
                });
                flatten_inlines(children, out);
            }
            Inline::Emphasis { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Emphasis,
                });
                flatten_inlines(children, out);
            }
            Inline::Monospace { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Monospace,
                });
                flatten_inlines(children, out);
            }
            Inline::Mark { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Mark,
                });
                flatten_inlines(children, out);
            }
            Inline::Superscript { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Superscript,
                });
                flatten_inlines(children, out);
            }
            Inline::Subscript { children, span } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Subscript,
                });
                flatten_inlines(children, out);
            }
            Inline::AttributeRef { span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::AttributeRef,
                });
            }
            // One token for the whole link, then highlight any markup in its
            // display text (for a bare link the child is plain Text → no token).
            Inline::Link { children, span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Link,
                });
                flatten_inlines(children, out);
            }
            // One token for the whole cross-reference, then highlight any markup
            // in its display text (a textless `<<id>>` has a plain Text child).
            Inline::CrossReference { children, span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::CrossReference,
                });
                flatten_inlines(children, out);
            }
            // One `Anchor` token over the whole `[[id]]` / `anchor:id[…]` (the
            // same kind the whole-line block anchor uses, so inline and block
            // anchors read alike). Markup in its reftext keeps the tokens of the
            // source syntax the macros stage consumed.
            Inline::Anchor { span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Anchor,
                });
            }
            // One token over the whole footnote macro.
            Inline::Footnote { span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::InlineMacro,
                });
            }
            // One token over the whole `image:…[…]` / `icon:…[…]` macro, a
            // keyboard, button or menu macro, or an index term.
            Inline::Image { span, .. }
            | Inline::Ui { span, .. }
            | Inline::IndexTerm { span, .. }
            | Inline::Stem { span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::InlineMacro,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokens;

    #[test]
    fn classifies_lines() {
        let toks = tokens(":a: 1\n= Title\n\ntext *bold* end");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::AttributeEntry));
        assert!(kinds.contains(&TokenKind::Heading));
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn strong_span_is_global() {
        let src = "text *bold* end";
        let toks = tokens(src);
        let strong = toks.iter().find(|t| t.kind == TokenKind::Strong).unwrap();
        assert_eq!(&src[strong.span.start..strong.span.end], "*bold*");
    }

    #[test]
    fn attribute_ref_is_tokenized() {
        let toks = tokens(":v: 1\n\nusing {v} here");
        let attr = toks.iter().find(|t| t.kind == TokenKind::AttributeRef);
        assert!(attr.is_some());
    }

    #[test]
    fn new_quotes_are_tokenized() {
        let toks = tokens("a `c`, #m#, x^2^, y~1~");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::Monospace));
        assert!(kinds.contains(&TokenKind::Mark));
        assert!(kinds.contains(&TokenKind::Superscript));
        assert!(kinds.contains(&TokenKind::Subscript));
    }

    #[test]
    fn monospace_token_span_is_global() {
        let src = "use `code` now";
        let toks = tokens(src);
        let t = toks
            .iter()
            .find(|t| t.kind == TokenKind::Monospace)
            .unwrap();
        assert_eq!(&src[t.span.start..t.span.end], "`code`");
    }

    #[test]
    fn list_marker_is_tokenized_with_inline_content() {
        let src = "* one *b*";
        let toks = tokens(src);
        let marker = toks
            .iter()
            .find(|t| t.kind == TokenKind::ListMarker)
            .expect("no list marker token");
        assert_eq!(&src[marker.span.start..marker.span.end], "*");
        // Inline markup inside the item is still tokenized.
        assert!(toks.iter().any(|t| t.kind == TokenKind::Strong));
    }

    #[test]
    fn ordered_list_emits_a_marker_per_item() {
        let toks = tokens(". a\n. b");
        let markers = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .count();
        assert_eq!(markers, 2);
    }

    #[test]
    fn nested_marker_token_covers_the_whole_run() {
        // A `**` marker token spans both stars.
        let src = "* one\n** deep";
        let toks = tokens(src);
        let nested = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .find(|t| t.span.start == 6)
            .expect("no nested list marker token");
        assert_eq!(&src[nested.span.start..nested.span.end], "**");
    }

    #[test]
    fn link_is_tokenized_with_global_span() {
        let src = "See link:https://h[the site] now";
        let toks = tokens(src);
        let link = toks.iter().find(|t| t.kind == TokenKind::Link).unwrap();
        assert_eq!(
            &src[link.span.start..link.span.end],
            "link:https://h[the site]"
        );
    }

    #[test]
    fn xref_is_tokenized_with_global_span() {
        // Both spellings emit one `xref` token spanning the whole construct.
        let src = "See <<sec,Two>> and xref:sec[M] now";
        let toks = tokens(src);
        let spans: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::CrossReference)
            .map(|t| &src[t.span.start..t.span.end])
            .collect();
        assert_eq!(spans, vec!["<<sec,Two>>", "xref:sec[M]"]);
    }

    #[test]
    fn token_spans_match_ast_spans() {
        // The whole point of the refactor: tokens are a projection of the same
        // inline scan the parser uses, so their spans must coincide.
        let src = "lead *bold* and _it_ tail";
        let toks = tokens(src);
        let blocks = crate::parse(src).document.blocks;
        let inlines = match &blocks[0].kind {
            crate::ast::BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        for inline in &inlines {
            let (want_span, want_kind) = match inline {
                crate::ast::Inline::Strong { span, .. } => (*span, TokenKind::Strong),
                crate::ast::Inline::Emphasis { span, .. } => (*span, TokenKind::Emphasis),
                _ => continue,
            };
            assert!(
                toks.iter()
                    .any(|t| t.kind == want_kind && t.span == want_span),
                "no token matches AST node span {want_span:?}"
            );
        }
    }

    #[test]
    fn indented_marker_is_tokenized_inside_a_list() {
        // An indented marker is still a marker; the token excludes the
        // leading spaces.
        let src = "* one\n  ** deep";
        let toks = tokens(src);
        let nested = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .find(|t| t.span.start == 8) // "* one\n  " is 8 bytes
            .expect("no indented list marker token");
        assert_eq!(&src[nested.span.start..nested.span.end], "**");
    }

    #[test]
    fn indented_marker_starts_a_list() {
        // Like Ruby's `UnorderedListRx`, leading whitespace may precede a marker.
        let src = "  * a list";
        let toks = tokens(src);
        let marker = toks
            .iter()
            .find(|t| t.kind == TokenKind::ListMarker)
            .unwrap();
        assert_eq!(&src[marker.span.start..marker.span.end], "*");
    }

    #[test]
    fn description_delimiters_are_list_markers() {
        let src = "CPU:: The *brain*
RAM::";
        let toks = tokens(src);
        let markers: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .map(|t| &src[t.span.start..t.span.end])
            .collect();
        assert_eq!(markers, ["::", "::"]);
        assert!(toks.iter().any(|t| t.kind == TokenKind::Strong));
    }

    #[test]
    fn wrapped_item_line_still_tokenizes_inline_markup() {
        // A plain continuation line of a list item is highlighted like body
        // text — its inline markup still yields tokens.
        let toks = tokens("* one\n  *bold* wrap");
        assert!(toks.iter().any(|t| t.kind == TokenKind::Strong));
    }

    #[test]
    fn explicit_numbered_marker_is_tokenized() {
        // The marker token covers the digits and the dot; multi-digit markers
        // extend it accordingly.
        let src = "1. one\n10. ten";
        let toks = tokens(src);
        let markers: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(&src[markers[0].span.start..markers[0].span.end], "1.");
        assert_eq!(&src[markers[1].span.start..markers[1].span.end], "10.");
    }

    #[test]
    fn block_anchor_line_is_one_anchor_token() {
        let src = "[[pa,The Para]]\ntext";
        let toks = tokens(src);
        let anchor = toks
            .iter()
            .find(|t| t.kind == TokenKind::Anchor)
            .expect("anchor token");
        // The token covers the whole `[[…]]` line, not the body line.
        assert_eq!(&src[anchor.span.start..anchor.span.end], "[[pa,The Para]]");
    }

    #[test]
    fn block_attribute_list_line_is_one_token() {
        let src = "[source,ruby]\n----\nx\n----";
        let toks = tokens(src);
        let attr = toks
            .iter()
            .find(|t| t.kind == TokenKind::BlockAttributeList)
            .expect("block-attribute-list token");
        // One token over the whole `[…]` line, recognised like the parser consumes it.
        assert_eq!(&src[attr.span.start..attr.span.end], "[source,ruby]");
        // The shorthand form is the same token kind (it is the same grammar).
        let toks = tokens("[#id.role]\ntext");
        assert!(toks.iter().any(|t| t.kind == TokenKind::BlockAttributeList));
        // Ruby's recognition: `[]` is a list; a leading blank, another first
        // character or a non-anchor `[[` line is text.
        assert!(tokens("[]\ntext")
            .iter()
            .any(|t| t.kind == TokenKind::BlockAttributeList && t.span == Span::new(0, 2)));
        for src in ["[ quote]\ntext", "[*x]\ntext", "[[x y]]\ntext"] {
            assert!(
                !tokens(src)
                    .iter()
                    .any(|t| t.kind == TokenKind::BlockAttributeList),
                "{src}"
            );
        }
    }

    #[test]
    fn block_title_line_is_one_token() {
        let src = ".A *bold* caption\nBody *text*.";
        let toks = tokens(src);
        let title = toks
            .iter()
            .find(|t| t.kind == TokenKind::BlockTitle)
            .expect("block-title token");
        // One token over the whole `.Title` line (incl. the marker dot), recognised
        // exactly as the parser attaches the title to the next block. Like the block
        // anchor's reftext, its inline markup is *not* sub-tokenized — so the only
        // `Strong` token comes from the body line, which tokenizes as usual.
        assert_eq!(&src[title.span.start..title.span.end], ".A *bold* caption");
        let strong: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::Strong)
            .map(|t| &src[t.span.start..t.span.end])
            .collect();
        assert_eq!(strong, vec!["*text*"]);
        // `. item` is an ordered-list marker, never a block title.
        let toks = tokens(". item");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::BlockTitle));
        assert!(toks.iter().any(|t| t.kind == TokenKind::ListMarker));
    }

    #[test]
    fn inline_anchor_is_tokenized_with_global_span() {
        // Both spellings emit one `anchor` token spanning the whole construct,
        // mid-line, the same kind the whole-line block anchor uses.
        let src = "x [[point]]y and anchor:spot[ref] z";
        let toks = tokens(src);
        let spans: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::Anchor)
            .map(|t| &src[t.span.start..t.span.end])
            .collect();
        assert_eq!(spans, vec!["[[point]]", "anchor:spot[ref]"]);
    }

    #[test]
    fn literal_block_is_tokenized() {
        let kinds: Vec<_> = tokens("....\nx *y* z\n....")
            .iter()
            .map(|t| t.kind)
            .collect();
        // The delimiters bracket the verbatim text; no inline markup tokens leak
        // out of the literal body (the `*y*` stays literal text).
        assert_eq!(
            kinds,
            [
                TokenKind::LiteralDelimiter,
                TokenKind::LiteralText,
                TokenKind::LiteralDelimiter,
            ]
        );
    }

    #[test]
    fn passthrough_block_is_tokenized() {
        let kinds: Vec<_> = tokens("++++\n<div>*y* {z}</div>\n++++")
            .iter()
            .map(|t| t.kind)
            .collect();
        // Like `....`, the delimiters bracket the verbatim body; no inline markup
        // tokens leak out of it (`*y*` / `{z}` stay passthrough text).
        assert_eq!(
            kinds,
            [
                TokenKind::PassthroughDelimiter,
                TokenKind::PassthroughText,
                TokenKind::PassthroughDelimiter,
            ]
        );
    }

    #[test]
    fn passthrough_body_swallows_other_delimiters() {
        // Once `++++` is open, a `----`/`....` line inside is passthrough text, not
        // a nested fence — mirroring the parser's verbatim capture.
        let kinds: Vec<_> = tokens("++++\n----\n....\n++++")
            .iter()
            .map(|t| t.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                TokenKind::PassthroughDelimiter,
                TokenKind::PassthroughText,
                TokenKind::PassthroughText,
                TokenKind::PassthroughDelimiter,
            ]
        );
    }

    #[test]
    fn admonition_label_is_tokenized() {
        let toks = tokens("NOTE: be *bold*");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        // The label is its own token; inline markup in the body is still scanned.
        assert!(kinds.contains(&TokenKind::AdmonitionLabel));
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn admonition_label_span_covers_label_and_colon() {
        let src = "TIP: go";
        let t = tokens(src)
            .into_iter()
            .find(|t| t.kind == TokenKind::AdmonitionLabel)
            .unwrap();
        assert_eq!(&src[t.span.start..t.span.end], "TIP:");
    }

    #[test]
    fn note_mid_paragraph_is_not_a_label() {
        // `NOTE:` on a continuation line is plain text, matching the parser.
        let toks = tokens("intro\nNOTE: not here");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::AdmonitionLabel));
    }

    #[test]
    fn lowercase_label_is_not_tokenized() {
        let toks = tokens("note: nope");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::AdmonitionLabel));
    }

    #[test]
    fn example_delimiter_is_tokenized_and_body_is_not_verbatim() {
        // The `====` fences are delimiter tokens, but unlike `----`/`....` the body
        // is highlighted normally — `*bold*` inside stays a Strong token.
        let toks = tokens("====\ntext *bold*\n====");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == TokenKind::ExampleDelimiter)
                .count(),
            2
        );
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn delimited_admonition_keeps_example_delimiter_tokens() {
        // `[NOTE]` on a `====` block: the attribute line is a `block-attribute-list`
        // token and the fences stay `ExampleDelimiter` — the admonition is a
        // parse-time fixup, invisible to the tokenizer. The body tokenizes normally
        // (so `*x*` is a Strong token) and there is no `AdmonitionLabel`.
        let toks = tokens("[NOTE]\n====\n*x*\n====");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::BlockAttributeList));
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == TokenKind::ExampleDelimiter)
                .count(),
            2
        );
        assert!(kinds.contains(&TokenKind::Strong));
        assert!(!kinds.contains(&TokenKind::AdmonitionLabel));
    }

    #[test]
    fn quote_delimiter_is_tokenized_and_body_is_not_verbatim() {
        // The `____` fences are quote-delimiter tokens; like `====` the body is
        // highlighted normally, so `*bold*` inside stays a Strong token. A `[verse]`
        // attribute line is a `block-attribute-list` token — the verse vs quote
        // distinction is a parse-time concern, invisible to the tokenizer.
        let toks = tokens("[verse]\n____\ntext *bold*\n____");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == TokenKind::QuoteDelimiter)
                .count(),
            2
        );
        assert!(kinds.contains(&TokenKind::BlockAttributeList));
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn sidebar_delimiter_is_tokenized_and_body_is_not_verbatim() {
        // The `****` fences are sidebar-delimiter tokens; like `====` the body is
        // highlighted normally, so `*bold*` inside stays a Strong token.
        let toks = tokens("****\ntext *bold*\n****");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == TokenKind::SidebarDelimiter)
                .count(),
            2
        );
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn unordered_marker_run_is_not_a_sidebar_delimiter() {
        // `**** item` (content after the run) is a list marker, not a sidebar fence.
        let toks = tokens("**** item");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::SidebarDelimiter));
        assert!(toks.iter().any(|t| t.kind == TokenKind::ListMarker));
    }

    #[test]
    fn open_delimiter_is_tokenized_and_body_is_not_verbatim() {
        // The `--` fences are open-delimiter tokens; like `====` the body is
        // highlighted normally, so `*bold*` inside stays a Strong token.
        let toks = tokens("--\ntext *bold*\n--");
        let kinds: Vec<_> = toks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == TokenKind::OpenDelimiter)
                .count(),
            2
        );
        assert!(kinds.contains(&TokenKind::Strong));
    }

    #[test]
    fn inline_em_dash_is_not_an_open_delimiter() {
        // An inline `--` (em dash) inside paragraph text is not a *whole-line* `--`,
        // so no open-delimiter token is emitted for it.
        let toks = tokens("an em -- dash");
        assert!(!toks.iter().any(|t| t.kind == TokenKind::OpenDelimiter));
    }
}

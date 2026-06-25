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

use crate::ast::{Inline, Span};
use crate::inline::scan_inlines;
use crate::parser::{collect_header, parse_attribute_entry, parse_heading, parse_list_marker};
use crate::scanner::lines_with_offsets;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub span: Span,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Heading,
    AttributeEntry,
    ListingDelimiter,
    ListingText,
    Strong,
    Emphasis,
    Monospace,
    Mark,
    Superscript,
    Subscript,
    AttributeRef,
    ListMarker,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Heading => "heading",
            TokenKind::AttributeEntry => "attribute-entry",
            TokenKind::ListingDelimiter => "listing-delimiter",
            TokenKind::ListingText => "listing-text",
            TokenKind::Strong => "strong",
            TokenKind::Emphasis => "emphasis",
            TokenKind::Monospace => "monospace",
            TokenKind::Mark => "mark",
            TokenKind::Superscript => "superscript",
            TokenKind::Subscript => "subscript",
            TokenKind::AttributeRef => "attribute-ref",
            TokenKind::ListMarker => "list-marker",
        }
    }
}

pub fn tokens(source: &str) -> Vec<Token> {
    let lines = lines_with_offsets(source);
    // Resolve `{attr}` against the same header table the parser uses.
    let (attrs, _) = collect_header(&lines);
    let mut out = Vec::new();
    let mut in_listing = false;

    for (off, line) in lines {
        let line_span = Span::new(off, off + line.len());

        if line.trim() == "----" {
            out.push(Token {
                span: line_span,
                kind: TokenKind::ListingDelimiter,
            });
            in_listing = !in_listing;
            continue;
        }
        if in_listing {
            out.push(Token {
                span: line_span,
                kind: TokenKind::ListingText,
            });
            continue;
        }
        if parse_attribute_entry(line).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::AttributeEntry,
            });
            continue;
        }
        if parse_heading(line.trim_end()).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::Heading,
            });
            continue;
        }
        // List item: a marker token, then the item content flattened like any
        // other body line — same `scan_inlines` (and base offset) the parser
        // uses, so token spans coincide with the item's inline spans.
        if let Some((_, _, mlen)) = parse_list_marker(line.trim_end()) {
            out.push(Token {
                span: Span::new(off, off + mlen),
                kind: TokenKind::ListMarker,
            });
            flatten_inlines(&scan_inlines(&line[mlen..], off + mlen, &attrs), &mut out);
            continue;
        }
        // Body text line: flatten the shared inline scan to highlight tokens.
        flatten_inlines(&scan_inlines(line, off, &attrs), &mut out);
    }

    out
}

/// Project the inline AST onto highlight tokens (depth-first, document order).
fn flatten_inlines(inlines: &[Inline], out: &mut Vec<Token>) {
    for node in inlines {
        match node {
            Inline::Text { .. } => {}
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(&src[marker.span.start..marker.span.end], "* ");
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
        // A `**` marker token spans both stars plus the trailing space.
        let src = "* one\n** deep";
        let toks = tokens(src);
        let nested = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .find(|t| t.span.start == 6)
            .expect("no nested list marker token");
        assert_eq!(&src[nested.span.start..nested.span.end], "** ");
    }

    #[test]
    fn token_spans_match_ast_spans() {
        // The whole point of the refactor: tokens are a projection of the same
        // inline scan the parser uses, so their spans must coincide.
        let src = "lead *bold* and _it_ tail";
        let toks = tokens(src);
        let blocks = crate::parser::parse(src).document.blocks;
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
}

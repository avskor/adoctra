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
use crate::parser::{
    collect_header, parse_attribute_entry, parse_block_anchor, parse_block_attr_list,
    parse_heading, parse_list_marker,
};
use crate::scanner::lines_with_offsets;

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub span: Span,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Heading,
    Anchor,
    BlockAttributeList,
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
    Link,
    CrossReference,
    ListMarker,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Heading => "heading",
            TokenKind::Anchor => "anchor",
            TokenKind::BlockAttributeList => "block-attribute-list",
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
            TokenKind::Link => "link",
            TokenKind::CrossReference => "xref",
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
    // Whether we're inside a list (started by a column-0 marker, ended by a
    // blank line — mirroring the parser). While set, indented markers are
    // recognised; this is the tokenizer's share of "indented nested markers".
    let mut in_list = false;

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
        if line.trim().is_empty() {
            in_list = false; // a blank line ends a list (mirrors the parser)
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
        if parse_block_attr_list(line).is_some() {
            out.push(Token {
                span: line_span,
                kind: TokenKind::BlockAttributeList,
            });
            continue;
        }
        // List item: a marker token, then the item content flattened like any
        // other body line — same `scan_inlines` (and base offset) the parser
        // uses, so token spans coincide with the item's inline spans. A list
        // starts at column 0; once inside one, a deeper marker may be indented
        // (asciidoctor folds the indent and nests by marker identity), so we
        // recognise an indented marker only while `in_list` and keep the indent
        // out of the marker token.
        let t = line.trim_end();
        let indent = if in_list {
            t.len() - t.trim_start_matches(' ').len()
        } else {
            0
        };
        if let Some((_, _, mlen)) = parse_list_marker(&t[indent..]) {
            in_list = true;
            let mstart = off + indent;
            out.push(Token {
                span: Span::new(mstart, mstart + mlen),
                kind: TokenKind::ListMarker,
            });
            flatten_inlines(
                &scan_inlines(&line[indent + mlen..], off + indent + mlen, &attrs),
                &mut out,
            );
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
            // One `Anchor` token over the whole `[[id]]` / `anchor:id[…]`, then
            // highlight any markup in its reftext (the same kind the whole-line
            // block anchor uses, so inline and block anchors read alike).
            Inline::Anchor { reftext, span, .. } => {
                out.push(Token {
                    span: *span,
                    kind: TokenKind::Anchor,
                });
                if let Some(rt) = reftext {
                    flatten_inlines(rt, out);
                }
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

    #[test]
    fn indented_marker_is_tokenized_inside_a_list() {
        // Once a list has started at column 0, an indented marker is still a
        // marker; the marker token excludes the leading spaces.
        let src = "* one\n  ** deep";
        let toks = tokens(src);
        let nested = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .find(|t| t.span.start == 8) // "* one\n  " is 8 bytes
            .expect("no indented list marker token");
        assert_eq!(&src[nested.span.start..nested.span.end], "** ");
    }

    #[test]
    fn indented_marker_is_not_tokenized_outside_a_list() {
        // With no list in progress, a leading-space marker is ordinary text
        // (lists start at column 0) — no marker token at the indented position.
        let toks = tokens("  * not a list");
        assert!(toks.iter().all(|t| t.kind != TokenKind::ListMarker));
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
        // The marker token covers the digits, the dot and the trailing space;
        // multi-digit markers extend it accordingly.
        let src = "1. one\n10. ten";
        let toks = tokens(src);
        let markers: Vec<_> = toks
            .iter()
            .filter(|t| t.kind == TokenKind::ListMarker)
            .collect();
        assert_eq!(markers.len(), 2);
        assert_eq!(&src[markers[0].span.start..markers[0].span.end], "1. ");
        assert_eq!(&src[markers[1].span.start..markers[1].span.end], "10. ");
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
}

//! Tokenizer for syntax highlighting.
//!
//! Produces a flat list of [`Token`]s with byte spans, classifying the
//! "interesting" parts of the source (headings, attribute entries, listing
//! delimiters/content, inline `*strong*` / `_emphasis_`). Plain text is left
//! untokenized. Driving highlighting from here keeps the editor's coloring in
//! lockstep with the real parser instead of a duplicated JS grammar.

use crate::ast::Span;
use crate::parser::{parse_attribute_entry, parse_heading};
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
        }
    }
}

pub fn tokens(source: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let mut in_listing = false;

    for (off, line) in lines_with_offsets(source) {
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
        scan_marker(off, line, '*', TokenKind::Strong, &mut out);
        scan_marker(off, line, '_', TokenKind::Emphasis, &mut out);
    }

    out
}

fn scan_marker(off: usize, line: &str, marker: char, kind: TokenKind, out: &mut Vec<Token>) {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while let Some(rel_open) = line[idx..].find(marker) {
        let open = idx + rel_open;
        match line[open + 1..].find(marker) {
            Some(rel_close) => {
                let close = open + 1 + rel_close;
                if bytes.get(open + 1) != Some(&b' ') {
                    out.push(Token {
                        span: Span::new(off + open, off + close + 1),
                        kind,
                    });
                }
                idx = close + 1;
            }
            None => break,
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
}

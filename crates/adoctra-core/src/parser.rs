//! Parser: builds a [`ParseResult`] from source text.
//!
//! Error-tolerant by design: it never returns `Err` for malformed syntax.
//! Whatever can be parsed becomes blocks (with spans); problems become
//! [`Diagnostic`]s. This is what lets an editor parse on every keystroke while
//! the buffer is half-written.

use crate::ast::{Block, BlockKind, Document, Inline, Span};
use crate::diagnostic::{Diagnostic, ParseResult, Severity};
use crate::scanner::lines_with_offsets;

pub fn parse(source: &str) -> ParseResult {
    let lines = lines_with_offsets(source);
    let mut doc = Document::default();
    let mut diagnostics = Vec::new();
    let mut i = 0usize;

    // --- header: leading `:key: value` entries and blank lines ---
    while i < lines.len() {
        let (_, line) = lines[i];
        if let Some((key, value)) = parse_attribute_entry(line) {
            doc.attributes.set(key, value);
            i += 1;
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            break;
        }
    }

    // --- body blocks ---
    while i < lines.len() {
        let (off, line) = lines[i];
        let trimmed = line.trim_end();

        if trimmed.trim().is_empty() {
            i += 1;
            continue;
        }

        if let Some((level, title)) = parse_heading(trimmed) {
            doc.blocks.push(Block {
                span: Span::new(off, off + line.len()),
                kind: BlockKind::Heading {
                    level,
                    inlines: parse_inlines(title),
                },
            });
            i += 1;
            continue;
        }

        if trimmed.trim() == "----" {
            let start = off;
            let mut end = off + line.len();
            i += 1;
            let mut text = String::new();
            let mut closed = false;
            while i < lines.len() {
                let (loff, lline) = lines[i];
                if lline.trim() == "----" {
                    end = loff + lline.len();
                    i += 1;
                    closed = true;
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(lline);
                end = loff + lline.len();
                i += 1;
            }
            if !closed {
                diagnostics.push(Diagnostic {
                    span: Span::new(start, start + 4),
                    severity: Severity::Error,
                    message: "unterminated listing block (missing closing `----`)".to_string(),
                });
            }
            doc.blocks.push(Block {
                span: Span::new(start, end),
                kind: BlockKind::Listing { text },
            });
            continue;
        }

        // Paragraph: accumulate contiguous non-blank, non-heading lines.
        let start = off;
        let mut end = off + line.len();
        let mut paragraph = String::from(trimmed);
        i += 1;
        while i < lines.len() {
            let (loff, lline) = lines[i];
            if lline.trim().is_empty() || parse_heading(lline.trim_end()).is_some() {
                break;
            }
            paragraph.push(' ');
            paragraph.push_str(lline.trim());
            end = loff + lline.len();
            i += 1;
        }
        doc.blocks.push(Block {
            span: Span::new(start, end),
            kind: BlockKind::Paragraph {
                inlines: parse_inlines(&paragraph),
            },
        });
    }

    ParseResult {
        document: doc,
        diagnostics,
    }
}

pub(crate) fn parse_attribute_entry(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let rest = line.strip_prefix(':')?;
    let end = rest.find(':')?;
    let key = &rest[..end];
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let value = rest[end + 1..].trim().to_string();
    Some((key.to_string(), value))
}

pub(crate) fn parse_heading(line: &str) -> Option<(u8, &str)> {
    let line = line.trim_start();
    let eqs = line.chars().take_while(|&c| c == '=').count();
    if eqs == 0 || eqs > 6 {
        return None;
    }
    let rest = &line[eqs..];
    let title = rest.strip_prefix(' ')?;
    Some(((eqs - 1) as u8, title.trim()))
}

/// One-pass inline parse for `*strong*` and `_emphasis_`. Naive on purpose.
fn parse_inlines(text: &str) -> Vec<Inline> {
    fn parse_wrapped(
        text: &str,
        marker: char,
        wrap: fn(Vec<Inline>) -> Inline,
    ) -> Option<Vec<Inline>> {
        let bytes = text.as_bytes();
        let open = text.find(marker)?;
        let close = text[open + 1..].find(marker)? + open + 1;
        if bytes.get(open + 1) == Some(&b' ') {
            return None;
        }
        let mut out = Vec::new();
        if open > 0 {
            out.push(Inline::Text(text[..open].to_string()));
        }
        out.push(wrap(parse_inlines(&text[open + 1..close])));
        out.extend(parse_inlines(&text[close + 1..]));
        Some(out)
    }

    if let Some(v) = parse_wrapped(text, '*', Inline::Strong) {
        return v;
    }
    if let Some(v) = parse_wrapped(text, '_', Inline::Emphasis) {
        return v;
    }
    if text.is_empty() {
        Vec::new()
    } else {
        vec![Inline::Text(text.to_string())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_and_paragraph() {
        let r = parse("= Title\n\nHello world");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.document.blocks.len(), 2);
        assert!(matches!(
            r.document.blocks[0].kind,
            BlockKind::Heading { level: 0, .. }
        ));
    }

    #[test]
    fn heading_span_covers_the_line() {
        let r = parse("= Title");
        assert_eq!(r.document.blocks[0].span, Span::new(0, 7));
    }

    #[test]
    fn unterminated_listing_is_a_diagnostic_not_a_failure() {
        let r = parse("----\nlet x = 1;");
        assert_eq!(r.document.blocks.len(), 1);
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn parses_header_attributes() {
        let r = parse(":author: Alexey\n:lang: ru\n\n= Title");
        assert_eq!(r.document.attributes.get("author"), Some("Alexey"));
        assert_eq!(r.document.attributes.get("lang"), Some("ru"));
    }
}

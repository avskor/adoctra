//! Parser: builds a [`ParseResult`] from source text.
//!
//! Error-tolerant by design: it never returns `Err` for malformed syntax.
//! Whatever can be parsed becomes blocks (with spans); problems become
//! [`Diagnostic`]s. This is what lets an editor parse on every keystroke while
//! the buffer is half-written.

use crate::ast::{Block, BlockKind, Document, ListItem, Span};
use crate::attributes::Attributes;
use crate::diagnostic::{Diagnostic, ParseResult, Severity};
use crate::inline::scan_inlines;
use crate::scanner::lines_with_offsets;

pub fn parse(source: &str) -> ParseResult {
    let lines = lines_with_offsets(source);
    let mut doc = Document::default();
    let mut diagnostics = Vec::new();

    // --- header: leading `:key: value` entries and blank lines ---
    let (attributes, mut i) = collect_header(&lines);
    doc.attributes = attributes;

    // --- body blocks ---
    while i < lines.len() {
        let (off, line) = lines[i];
        let trimmed = line.trim_end();

        if trimmed.trim().is_empty() {
            i += 1;
            continue;
        }

        if let Some((level, title)) = parse_heading(trimmed) {
            // `title` is a subslice of `source`, so its absolute offset gives an
            // exact base for inline spans.
            let base = abs_offset(source, title);
            let inlines = scan_inlines(title, base, &doc.attributes);
            doc.blocks.push(Block {
                span: Span::new(off, off + line.len()),
                kind: BlockKind::Heading { level, inlines },
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

        // List: consecutive lines carrying the *same* marker char form one
        // flat list (`*`/`-` unordered, `.` ordered).
        if let Some((marker, ordered, _)) = parse_list_marker(trimmed) {
            let start = off;
            let mut end = off + line.len();
            let mut items = Vec::new();
            while i < lines.len() {
                let (loff, lline) = lines[i];
                match parse_list_marker(lline.trim_end()) {
                    Some((m, _, mlen)) if m == marker => {
                        let content = &source[loff + mlen..loff + lline.len()];
                        let inlines = scan_inlines(content, loff + mlen, &doc.attributes);
                        items.push(ListItem {
                            inlines,
                            span: Span::new(loff, loff + lline.len()),
                        });
                        end = loff + lline.len();
                        i += 1;
                    }
                    _ => break,
                }
            }
            doc.blocks.push(Block {
                span: Span::new(start, end),
                kind: BlockKind::List { ordered, items },
            });
            continue;
        }

        // Paragraph: accumulate contiguous non-blank, non-heading lines.
        let start = off;
        let mut end = off + line.len();
        i += 1;
        while i < lines.len() {
            let (loff, lline) = lines[i];
            if lline.trim().is_empty()
                || parse_heading(lline.trim_end()).is_some()
                || parse_list_marker(lline.trim_end()).is_some()
            {
                break;
            }
            end = loff + lline.len();
            i += 1;
        }
        // Scan inlines over the original source slice (newlines and all), so
        // every inline span maps back to real bytes — no re-joined string.
        let inlines = scan_inlines(&source[start..end], start, &doc.attributes);
        doc.blocks.push(Block {
            span: Span::new(start, end),
            kind: BlockKind::Paragraph { inlines },
        });
    }

    ParseResult {
        document: doc,
        diagnostics,
    }
}

/// Read leading `:key: value` entries and blank lines. Returns the collected
/// attributes and the index of the first body line. Shared by the parser and
/// the tokenizer so both resolve `{attr}` against the same table.
pub(crate) fn collect_header(lines: &[(usize, &str)]) -> (Attributes, usize) {
    let mut attributes = Attributes::new();
    let mut i = 0usize;
    while i < lines.len() {
        let (_, line) = lines[i];
        if let Some((key, value)) = parse_attribute_entry(line) {
            attributes.set(key, value);
            i += 1;
        } else if line.trim().is_empty() {
            i += 1;
        } else {
            break;
        }
    }
    (attributes, i)
}

/// Byte offset of subslice `sub` within `source`. `sub` must be a slice that
/// points into `source` (e.g. produced by `trim`/`strip_prefix` on a line).
fn abs_offset(source: &str, sub: &str) -> usize {
    sub.as_ptr() as usize - source.as_ptr() as usize
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

/// A list marker at the very start of `line`: exactly one of `*`/`-`/`.`
/// followed by a space. Returns `(marker_char, ordered, marker_len_bytes)`,
/// where `marker_len_bytes` is the byte offset at which item content begins.
/// Top-level only for now: leading whitespace is not accepted.
pub(crate) fn parse_list_marker(line: &str) -> Option<(char, bool, usize)> {
    let mut chars = line.chars();
    let marker = chars.next()?;
    let ordered = match marker {
        '*' | '-' => false,
        '.' => true,
        _ => return None,
    };
    if chars.next() != Some(' ') {
        return None;
    }
    Some((marker, ordered, marker.len_utf8() + 1))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Inline, Span};

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

    /// Helper: the first inline of the first block, whatever its kind.
    fn first_inline(src: &str) -> Inline {
        match &parse(src).document.blocks[0].kind {
            BlockKind::Paragraph { inlines } | BlockKind::Heading { inlines, .. } => {
                inlines[0].clone()
            }
            other => panic!("unexpected block: {other:?}"),
        }
    }

    #[test]
    fn inline_strong_span_points_at_real_source() {
        let src = "Hi *there*";
        let inlines = match &parse(src).document.blocks[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        let strong = inlines
            .iter()
            .find(|n| matches!(n, Inline::Strong { .. }))
            .unwrap();
        let s = strong.span();
        assert_eq!(&src[s.start..s.end], "*there*");
    }

    #[test]
    fn inline_span_is_accurate_across_a_line_break() {
        // The paragraph is scanned over the original source slice, so the
        // strong marker on the *second* line still gets real byte offsets.
        let src = "first line\nsecond *strong* line";
        let inlines = match &parse(src).document.blocks[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        let strong = inlines
            .iter()
            .find(|n| matches!(n, Inline::Strong { .. }))
            .unwrap();
        let s = strong.span();
        assert_eq!(&src[s.start..s.end], "*strong*");
    }

    #[test]
    fn attribute_reference_is_resolved_in_core() {
        let src = ":v: 9\n\nVersion {v}";
        let attr = match first_inline(src) {
            Inline::Text { .. } => {
                // {v} is the second inline; grab the block again
                let blocks = parse(src).document.blocks;
                match &blocks[0].kind {
                    BlockKind::Paragraph { inlines } => inlines[1].clone(),
                    _ => unreachable!(),
                }
            }
            other => other,
        };
        match attr {
            Inline::AttributeRef { name, value, span } => {
                assert_eq!(name, "v");
                assert_eq!(value, Some("9".to_string()));
                // span points at `{v}` in the body, not at the resolved value
                assert_eq!(&src[span.start..span.end], "{v}");
            }
            other => panic!("expected AttributeRef, got {other:?}"),
        }
    }

    #[test]
    fn missing_attribute_keeps_no_value() {
        let r = parse("Ref to {unknown} here");
        let inlines = match &r.document.blocks[0].kind {
            BlockKind::Paragraph { inlines } => inlines.clone(),
            _ => unreachable!(),
        };
        assert!(inlines
            .iter()
            .any(|n| matches!(n, Inline::AttributeRef { value: None, .. })));
    }

    /// The list block of `src`, or panic if the first block is not a list.
    fn list(src: &str) -> (bool, Vec<ListItem>) {
        match &parse(src).document.blocks[0].kind {
            BlockKind::List { ordered, items } => (*ordered, items.clone()),
            other => panic!("expected List, got {other:?}"),
        }
    }

    #[test]
    fn parses_unordered_star_list() {
        let r = parse("* one\n* two");
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.document.blocks.len(), 1);
        let (ordered, items) = list("* one\n* two");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
        // Item content is scanned with the right base offset.
        assert!(matches!(&items[0].inlines[0], Inline::Text { value, span }
            if value == "one" && *span == Span::new(2, 5)));
    }

    #[test]
    fn dash_is_an_unordered_marker() {
        let (ordered, items) = list("- a\n- b");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parses_ordered_dot_list() {
        let (ordered, items) = list(". first\n. second");
        assert!(ordered);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn list_block_and_item_spans_cover_their_lines() {
        let src = "* one\n* two";
        let block = &parse(src).document.blocks[0];
        assert_eq!(block.span, Span::new(0, 11));
        let items = match &block.kind {
            BlockKind::List { items, .. } => items,
            _ => unreachable!(),
        };
        assert_eq!(items[0].span, Span::new(0, 5));
        assert_eq!(items[1].span, Span::new(6, 11));
    }

    #[test]
    fn list_stops_at_blank_line() {
        let r = parse("* one\n\nA paragraph");
        assert_eq!(r.document.blocks.len(), 2);
        let (_, items) = list("* one\n\nA paragraph");
        assert_eq!(items.len(), 1);
        assert!(matches!(
            r.document.blocks[1].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn different_markers_make_separate_lists() {
        let r = parse("* a\n. b");
        assert_eq!(r.document.blocks.len(), 2);
        assert!(matches!(
            r.document.blocks[0].kind,
            BlockKind::List { ordered: false, .. }
        ));
        assert!(matches!(
            r.document.blocks[1].kind,
            BlockKind::List { ordered: true, .. }
        ));
    }

    #[test]
    fn star_without_space_is_not_a_list() {
        // `*bold*` has no space after the marker → it stays a paragraph.
        let r = parse("*bold*");
        assert!(matches!(
            r.document.blocks[0].kind,
            BlockKind::Paragraph { .. }
        ));
    }

    #[test]
    fn a_list_interrupts_a_paragraph_without_a_blank_line() {
        let r = parse("text\n* item");
        assert_eq!(r.document.blocks.len(), 2);
        assert!(matches!(
            r.document.blocks[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert!(matches!(r.document.blocks[1].kind, BlockKind::List { .. }));
    }
}

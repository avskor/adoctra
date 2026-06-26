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
        if lines[i].1.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(block) = parse_block(
            source,
            &lines,
            &mut i,
            &doc.attributes,
            &mut diagnostics,
            BlockCtx::TopLevel,
        ) {
            doc.blocks.push(block);
        }
    }

    ParseResult {
        document: doc,
        diagnostics,
    }
}

/// Where a block is being parsed. The only behavioural difference is how a
/// paragraph *ends*: at the top level it runs until a blank line, a heading or a
/// column-0 list marker; inside a list item (after a `+` continuation) it
/// additionally ends at a lone `+` line or an *indented* list marker, so an
/// attached paragraph does not swallow the next continuation or sibling item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockCtx {
    TopLevel,
    InListItem,
}

/// Parse exactly one block starting at `lines[*i]` (which the caller has ensured
/// is non-blank), advancing `*i` past every line it consumes. Returns `None`
/// only defensively, if a blank line slipped through. Shared by the top-level
/// loop and by list-item `+` continuations, so attached blocks reuse the very
/// same block grammar.
fn parse_block(
    source: &str,
    lines: &[(usize, &str)],
    i: &mut usize,
    attrs: &Attributes,
    diagnostics: &mut Vec<Diagnostic>,
    ctx: BlockCtx,
) -> Option<Block> {
    let (off, line) = lines[*i];
    let trimmed = line.trim_end();

    if trimmed.trim().is_empty() {
        *i += 1;
        return None;
    }

    // Heading.
    if let Some((level, title)) = parse_heading(trimmed) {
        // `title` is a subslice of `source`, so its absolute offset gives an
        // exact base for inline spans.
        let base = abs_offset(source, title);
        let inlines = scan_inlines(title, base, attrs);
        *i += 1;
        return Some(Block {
            span: Span::new(off, off + line.len()),
            kind: BlockKind::Heading { level, inlines },
        });
    }

    // Listing block (verbatim, fenced by `----`).
    if trimmed.trim() == "----" {
        let start = off;
        let mut end = off + line.len();
        *i += 1;
        let mut text = String::new();
        let mut closed = false;
        while *i < lines.len() {
            let (loff, lline) = lines[*i];
            if lline.trim() == "----" {
                end = loff + lline.len();
                *i += 1;
                closed = true;
                break;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(lline);
            end = loff + lline.len();
            *i += 1;
        }
        if !closed {
            diagnostics.push(Diagnostic {
                span: Span::new(start, start + 4),
                severity: Severity::Error,
                message: "unterminated listing block (missing closing `----`)".to_string(),
            });
        }
        return Some(Block {
            span: Span::new(start, end),
            kind: BlockKind::Listing { text },
        });
    }

    // List: a run of items that nests a level deeper whenever a *unique* marker
    // appears. A list only *starts* at column 0 (an indented marker elsewhere is
    // ordinary text); deeper levels may be indented, handled inside `parse_list`,
    // which consumes the whole (possibly nested) list.
    if parse_list_marker(trimmed).is_some() {
        return Some(parse_list(source, lines, i, attrs, diagnostics, &[]));
    }

    // Paragraph: accumulate contiguous lines until this context's terminator.
    let start = off;
    let mut end = off + line.len();
    *i += 1;
    while *i < lines.len() {
        let (loff, lline) = lines[*i];
        let t = lline.trim_end();
        let ends = t.trim().is_empty()
            || parse_heading(t).is_some()
            || match ctx {
                BlockCtx::TopLevel => parse_list_marker(t).is_some(),
                BlockCtx::InListItem => t.trim() == "+" || list_marker_indented(t).is_some(),
            };
        if ends {
            break;
        }
        end = loff + lline.len();
        *i += 1;
    }
    // Scan inlines over the original source slice (newlines and all), so every
    // inline span maps back to real bytes — no re-joined string.
    let inlines = scan_inlines(&source[start..end], start, attrs);
    Some(Block {
        span: Span::new(start, end),
        kind: BlockKind::Paragraph { inlines },
    })
}

/// The role of a source line *relative to a list level* (its current `marker`
/// and the `ancestors` stack). Computing it in one place lets `parse_list` read
/// as a small state machine: for the next line it decides whether the line
/// continues the item's text, attaches a block, opens a sublist, or ends the
/// list.
#[derive(Debug, PartialEq, Eq)]
enum LineRole {
    /// Blank line — ends the list (the current simplification).
    Blank,
    /// A lone `+` — a list continuation; the next block attaches to the item.
    Continuation,
    /// A marker identical to this level's — the next sibling item.
    Sibling,
    /// A marker belonging to an enclosing list — hands control back up.
    Ancestor,
    /// A unique, deeper marker — opens a sublist under the current item.
    Nested,
    /// Anything else — principal-text continuation of the current item.
    Text,
}

/// Classify `line` against the current list `marker` and `ancestors`. Marker
/// detection is indent-aware ([`list_marker_indented`]): asciidoctor decides
/// nesting by marker *identity*, not indentation, so an indented `*` under a
/// `*` list is a sibling, while `**` opens a deeper level regardless of indent.
fn classify(line: &str, marker: MarkerId, ancestors: &[MarkerId]) -> LineRole {
    let t = line.trim_end();
    if t.trim().is_empty() {
        return LineRole::Blank;
    }
    if t.trim() == "+" {
        return LineRole::Continuation;
    }
    match list_marker_indented(t) {
        Some((m, _, _)) if m == marker => LineRole::Sibling,
        Some((m, _, _)) if ancestors.contains(&m) => LineRole::Ancestor,
        Some(_) => LineRole::Nested,
        None => LineRole::Text,
    }
}

/// Parse a list — and any deeper sublists — starting at `lines[*i]`, which must
/// carry a list marker. Advances `*i` past every consumed line.
///
/// `ancestors` holds the marker identities of the enclosing lists. A line whose
/// marker matches an ancestor ends this list (returning control to that
/// ancestor); a *unique* marker opens a sublist under the current item — this is
/// asciidoctor's marker-stack nesting rule. Each item's principal text may wrap
/// over several plain lines, and a `+` continuation attaches following blocks
/// (paragraph, listing) to the item; both nested sublists and attached blocks
/// live in `item.blocks`, in source order.
fn parse_list(
    source: &str,
    lines: &[(usize, &str)],
    i: &mut usize,
    attrs: &Attributes,
    diagnostics: &mut Vec<Diagnostic>,
    ancestors: &[MarkerId],
) -> Block {
    let (marker, ordered, _) =
        list_marker_indented(lines[*i].1.trim_end()).expect("parse_list called on a non-list line");
    let start = lines[*i].0;
    let mut end = start;
    let mut items = Vec::new();

    // Only a sibling marker begins another item; everything else (a blank line,
    // an ancestor's marker) ends this list. Deeper markers and `+` lines are
    // folded into the item below, so they never reach this guard.
    while *i < lines.len() && classify(lines[*i].1, marker, ancestors) == LineRole::Sibling {
        let (loff, lline) = lines[*i];
        let content_off = list_marker_indented(lline.trim_end())
            .map(|(_, _, off)| off)
            .expect("a sibling line carries a marker");
        let item_start = loff;
        let content_start = loff + content_off;
        let mut principal_end = loff + lline.len();
        *i += 1;

        // Multi-line principal text: fold following plain lines into this item.
        while *i < lines.len() && classify(lines[*i].1, marker, ancestors) == LineRole::Text {
            let (toff, tline) = lines[*i];
            principal_end = toff + tline.len();
            *i += 1;
        }
        let inlines = scan_inlines(&source[content_start..principal_end], content_start, attrs);

        // Attached blocks (`+` continuations) and nested sublists, in source
        // order, all collected into `item.blocks`.
        let mut blocks = Vec::new();
        let mut item_end = principal_end;
        while *i < lines.len() {
            match classify(lines[*i].1, marker, ancestors) {
                LineRole::Continuation => {
                    *i += 1; // consume the `+`
                    if *i < lines.len() && !lines[*i].1.trim().is_empty() {
                        if let Some(b) =
                            parse_block(source, lines, i, attrs, diagnostics, BlockCtx::InListItem)
                        {
                            item_end = b.span.end;
                            blocks.push(b);
                        }
                    }
                }
                LineRole::Nested => {
                    let mut child_anc = ancestors.to_vec();
                    child_anc.push(marker);
                    let child = parse_list(source, lines, i, attrs, diagnostics, &child_anc);
                    item_end = child.span.end;
                    blocks.push(child);
                }
                _ => break,
            }
        }

        end = item_end;
        items.push(ListItem {
            inlines,
            blocks,
            span: Span::new(item_start, item_end),
        });
    }

    Block {
        span: Span::new(start, end),
        kind: BlockKind::List { ordered, items },
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

/// Identity of a list marker. Two markers open distinct nesting levels iff they
/// differ — this is asciidoctor's rule ("a new level for each unique marker"),
/// so nesting works across kinds (e.g. a `.` item inside a `*` list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerId {
    /// A run-marker: its leading byte and how many times that byte repeats. `-`
    /// is always width 1; `*` and `.` may repeat (`**`, `...`) to mark a deeper
    /// level.
    Run { byte: u8, width: usize },
    /// An explicit-numbered ordered marker (`1.`, `2.`, `10.`, …). All of them
    /// share one identity: the digit *value* and *count* don't affect nesting,
    /// and the numbers are rendered positionally — so this is distinct from the
    /// implicit ordered marker `Run { byte: b'.', .. }`, matching asciidoctor
    /// (a `1.` item after a `.` list nests rather than continuing it).
    OrderedExplicit,
}

/// A list marker at the very start of `line`: a run of `*` or `.`, a single
/// `-`, or an explicit-numbered `<digits>.` — each followed by a space. Returns
/// `(marker, ordered, marker_len_bytes)`, where `marker_len_bytes` is the byte
/// offset at which item content begins. Markers must be at column 0: leading
/// whitespace is not accepted. (All marker bytes are ASCII, so a run's width
/// equals its byte length.)
pub(crate) fn parse_list_marker(line: &str) -> Option<(MarkerId, bool, usize)> {
    let bytes = line.as_bytes();
    let first = *bytes.first()?;

    // Explicit-numbered ordered marker: a run of ASCII digits, then `.`, then a
    // space (e.g. `1. `, `10. `). Collapses to a single identity regardless of
    // the digits — `1.` and `10.` are siblings of one list.
    if first.is_ascii_digit() {
        let mut digits = 1;
        while matches!(bytes.get(digits), Some(b) if b.is_ascii_digit()) {
            digits += 1;
        }
        if bytes.get(digits) != Some(&b'.') || bytes.get(digits + 1) != Some(&b' ') {
            return None;
        }
        return Some((MarkerId::OrderedExplicit, true, digits + 2));
    }

    let (ordered, repeatable) = match first {
        b'*' => (false, true),
        b'.' => (true, true),
        b'-' => (false, false),
        _ => return None,
    };
    let mut width = 1;
    if repeatable {
        while bytes.get(width) == Some(&first) {
            width += 1;
        }
    }
    if bytes.get(width) != Some(&b' ') {
        return None;
    }
    Some((MarkerId::Run { byte: first, width }, ordered, width + 1))
}

/// Like [`parse_list_marker`] but tolerant of leading ASCII spaces. Indentation
/// is cosmetic in asciidoctor — nesting is decided by marker *identity*, not
/// indent — so we strip leading spaces before matching and fold their width into
/// the returned content offset. The [`MarkerId`] comes from the de-indented
/// marker, so an indented `*` is the *same* level as a column-0 `*` (a sibling),
/// while `**` opens a new level regardless of indent.
pub(crate) fn list_marker_indented(line: &str) -> Option<(MarkerId, bool, usize)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let (marker, ordered, mlen) = parse_list_marker(&line[indent..])?;
    Some((marker, ordered, indent + mlen))
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

    /// The single nested list held by `item`, or panic.
    fn sublist(item: &ListItem) -> (bool, Vec<ListItem>) {
        assert_eq!(item.blocks.len(), 1, "expected exactly one nested block");
        match &item.blocks[0].kind {
            BlockKind::List { ordered, items } => (*ordered, items.clone()),
            other => panic!("expected nested List, got {other:?}"),
        }
    }

    /// The principal text of `item` (its first inline, asserted to be text).
    fn item_text(item: &ListItem) -> String {
        match &item.inlines[0] {
            Inline::Text { value, .. } => value.clone(),
            other => panic!("expected Text, got {other:?}"),
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
    fn different_marker_nests_as_sublist() {
        // asciidoctor's rule: a *unique* marker opens a deeper level. So `. b`
        // (ordered) nests inside the `* a` (unordered) item — it does not start
        // a second top-level list. (Confirmed against asciidoctor 2.0.23.)
        let r = parse("* a\n. b");
        assert_eq!(r.document.blocks.len(), 1);
        let (ordered, items) = list("* a\n. b");
        assert!(!ordered);
        assert_eq!(items.len(), 1);
        assert_eq!(item_text(&items[0]), "a");
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(sub_items.len(), 1);
        assert_eq!(item_text(&sub_items[0]), "b");
    }

    #[test]
    fn doubled_marker_nests_under_its_item() {
        // `*` then `**` then back to `*`.
        let (ordered, items) = list("* one\n** a\n** b\n* two");
        assert!(!ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(!sub_ordered);
        assert_eq!(sub_items.len(), 2);
        assert_eq!(item_text(&sub_items[0]), "a");
        assert_eq!(item_text(&sub_items[1]), "b");
        // The second top-level item is a sibling with no nested content.
        assert_eq!(item_text(&items[1]), "two");
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn three_levels_nest_and_unwind() {
        // `*` / `**` / `***`, then `**` (back one level), then `*` (back to top).
        let (_, items) = list("* one\n** a\n*** x\n** b\n* two");
        assert_eq!(items.len(), 2);
        let (_, lvl2) = sublist(&items[0]);
        assert_eq!(lvl2.len(), 2); // a (with a deeper child), b
        assert_eq!(item_text(&lvl2[0]), "a");
        let (_, lvl3) = sublist(&lvl2[0]);
        assert_eq!(lvl3.len(), 1);
        assert_eq!(item_text(&lvl3[0]), "x");
        assert_eq!(item_text(&lvl2[1]), "b");
        assert!(lvl2[1].blocks.is_empty());
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn ordered_marker_run_nests() {
        let (ordered, items) = list(". one\n.. a\n.. b\n. two");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        let (sub_ordered, sub_items) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(sub_items.len(), 2);
    }

    #[test]
    fn mixed_ordered_unordered_from_docs() {
        // The hybrid-list example from the asciidoctor docs: an ordered list
        // whose items each hold a nested unordered list.
        let (ordered, items) = list(". Linux\n* Fedora\n* Ubuntu\n. BSD\n* FreeBSD");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "Linux");
        let (o0, distros0) = sublist(&items[0]);
        assert!(!o0);
        assert_eq!(distros0.len(), 2);
        assert_eq!(item_text(&distros0[0]), "Fedora");
        assert_eq!(item_text(&distros0[1]), "Ubuntu");
        assert_eq!(item_text(&items[1]), "BSD");
        let (o1, distros1) = sublist(&items[1]);
        assert!(!o1);
        assert_eq!(distros1.len(), 1);
        assert_eq!(item_text(&distros1[0]), "FreeBSD");
    }

    #[test]
    fn item_span_covers_its_nested_sublist() {
        // "* one\n** a": item "one" spans the whole construct (0..10); the
        // nested list block spans just its own line (6..10).
        let src = "* one\n** a";
        let (_, items) = list(src);
        assert_eq!(items[0].span, Span::new(0, 10));
        assert_eq!(&src[items[0].span.start..items[0].span.end], "* one\n** a");
        assert_eq!(items[0].blocks[0].span, Span::new(6, 10));
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

    /// `item_text` with interior whitespace collapsed — handy where a multi-line
    /// item folds its line break (and any indent) to spaces.
    fn item_text_squeezed(item: &ListItem) -> String {
        item_text(item)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The single nested/attached block of `item`, asserted to be a paragraph,
    /// returning its principal text.
    fn paragraph_text(block: &Block) -> String {
        match &block.kind {
            BlockKind::Paragraph { inlines } => match &inlines[0] {
                Inline::Text { value, .. } => value.clone(),
                other => panic!("expected Text, got {other:?}"),
            },
            other => panic!("expected Paragraph, got {other:?}"),
        }
    }

    #[test]
    fn item_text_wraps_over_several_lines() {
        // A plain line after the marker folds into the same item's text — its
        // line break (and indent) become spaces, but the item span covers both
        // source lines. (Confirmed against asciidoctor 2.0.23: one <li>.)
        let src = "* one\n  two\n* three";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text_squeezed(&items[0]), "one two");
        assert_eq!(item_text(&items[1]), "three");
        assert_eq!(&src[items[0].span.start..items[0].span.end], "* one\n  two");
    }

    #[test]
    fn indented_same_marker_is_a_sibling_not_nested() {
        // asciidoctor decides nesting by marker identity, not indent: an
        // indented `*` under a `*` list is a *sibling*, not a child.
        let src = "* one\n  * nested";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(item_text(&items[1]), "nested");
        assert!(items[0].blocks.is_empty());
    }

    #[test]
    fn indented_unique_marker_nests() {
        // A unique marker (`**`) opens a deeper level even when indented.
        let src = "* one\n  ** nested";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        let (_, sub) = sublist(&items[0]);
        assert_eq!(sub.len(), 1);
        assert_eq!(item_text(&sub[0]), "nested");
    }

    #[test]
    fn continuation_attaches_a_paragraph() {
        // `+` attaches the following paragraph as a block inside the item.
        let src = "* one\n+\nattached para\n* two";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(items[0].blocks.len(), 1);
        assert_eq!(paragraph_text(&items[0].blocks[0]), "attached para");
        assert_eq!(item_text(&items[1]), "two");
        assert!(items[1].blocks.is_empty());
    }

    #[test]
    fn continuation_attaches_a_listing() {
        let src = "* one\n+\n----\ncode\n----\n* two";
        let (_, items) = list(src);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].blocks.len(), 1);
        assert!(matches!(
            &items[0].blocks[0].kind,
            BlockKind::Listing { text } if text == "code"
        ));
        assert_eq!(item_text(&items[1]), "two");
    }

    #[test]
    fn double_continuation_attaches_two_blocks_in_order() {
        let src = "* one\n+\npara A\n+\npara B";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].blocks.len(), 2);
        assert_eq!(paragraph_text(&items[0].blocks[0]), "para A");
        assert_eq!(paragraph_text(&items[0].blocks[1]), "para B");
    }

    #[test]
    fn item_span_covers_an_attached_block() {
        let src = "* one\n+\nattached";
        let (_, items) = list(src);
        assert_eq!(items.len(), 1);
        assert_eq!(
            &src[items[0].span.start..items[0].span.end],
            "* one\n+\nattached"
        );
    }

    #[test]
    fn explicit_numbered_marker_is_an_ordered_list() {
        let src = "1. one\n2. two\n3. three";
        let (ordered, items) = list(src);
        assert!(ordered);
        assert_eq!(items.len(), 3);
        assert_eq!(item_text(&items[0]), "one");
        assert_eq!(item_text(&items[1]), "two");
        assert_eq!(item_text(&items[2]), "three");
        // Item content is scanned after the `1. ` marker (3 bytes).
        assert!(matches!(&items[0].inlines[0], Inline::Text { value, span }
            if value == "one" && *span == Span::new(3, 6)));
    }

    #[test]
    fn multi_digit_numbers_are_siblings_not_nested() {
        // `9.`, `10.`, `11.` collapse to one marker identity, so they form a
        // single flat list — the digit count must not leak into nesting.
        let (ordered, items) = list("9. nine\n10. ten\n11. eleven");
        assert!(ordered);
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|it| it.blocks.is_empty()));
        assert_eq!(item_text(&items[2]), "eleven");
    }

    #[test]
    fn explicit_number_nests_under_implicit_dot() {
        // A `1.` marker is a *unique* identity vs the implicit `.`, so it opens
        // a sublist under the preceding `.` item rather than continuing the
        // list. (Confirmed against asciidoctor 2.0.23.)
        let (ordered, items) = list(". a\n. b\n1. c");
        assert!(ordered);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "a");
        assert_eq!(item_text(&items[1]), "b");
        let (sub_ordered, sub) = sublist(&items[1]);
        assert!(sub_ordered);
        assert_eq!(sub.len(), 1);
        assert_eq!(item_text(&sub[0]), "c");
    }

    #[test]
    fn implicit_dot_nests_under_explicit_number() {
        // The symmetric case: a `.` after a `1.` item nests, since the two
        // markers are distinct identities. (Confirmed against asciidoctor 2.0.23.)
        let (ordered, items) = list("1. a\n. b");
        assert!(ordered);
        assert_eq!(items.len(), 1);
        assert_eq!(item_text(&items[0]), "a");
        let (sub_ordered, sub) = sublist(&items[0]);
        assert!(sub_ordered);
        assert_eq!(item_text(&sub[0]), "b");
    }

    #[test]
    fn number_without_space_or_dot_is_not_a_list() {
        // `1.text` (no space) and `1) item` (no dot) are not markers → paragraph.
        assert!(matches!(
            parse("1.text").document.blocks[0].kind,
            BlockKind::Paragraph { .. }
        ));
        assert!(matches!(
            parse("1) item").document.blocks[0].kind,
            BlockKind::Paragraph { .. }
        ));
    }
}

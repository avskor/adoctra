//! The single inline scanner.
//!
//! One pass over a slice of source produces spanned [`Inline`] nodes. Both the
//! parser (which builds the document model) and the tokenizer (which feeds
//! editor highlighting) drive off *this* function, so there is exactly one
//! place that understands `*strong*`, `_emphasis_` and `{attribute}` markup —
//! no second grammar that can drift out of sync.
//!
//! Offsets are absolute: `base` is the byte offset of `src[0]` in the original
//! document, so every node's [`Span`] maps straight back to real source bytes.
//! Newlines inside `src` (a paragraph spanning several source lines) are folded
//! to spaces in text *content*, while the spans keep pointing at the real
//! bytes.

use crate::ast::{Inline, Span};
use crate::attributes::Attributes;

/// Scan inline markup in `src`. `base` is the absolute byte offset of `src[0]`
/// in the original source; `attrs` resolves `{name}` references.
pub(crate) fn scan_inlines(src: &str, base: usize, attrs: &Attributes) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;

    while i < src.len() {
        if let Some((node, end)) = try_match(src, i, base, attrs) {
            flush_text(src, text_start, i, base, &mut out);
            out.push(node);
            i = end;
            text_start = end;
        } else {
            // advance one whole UTF-8 char
            i += utf8_step(src, i);
        }
    }
    flush_text(src, text_start, src.len(), base, &mut out);
    out
}

/// Try to recognise a construct starting at byte `i`. Returns the node and the
/// byte index (src-relative) just past it.
fn try_match(src: &str, i: usize, base: usize, attrs: &Attributes) -> Option<(Inline, usize)> {
    // Wrap a matcher's `(children, absolute_end)` into a quote node: the span
    // covers the markers, and `scan_inlines` wants the end as a src-relative
    // index (hence `end - base`).
    let quote = |make: fn(Vec<Inline>, Span) -> Inline, res: Option<(Vec<Inline>, usize)>| {
        res.map(|(children, end)| (make(children, Span::new(base + i, end)), end - base))
    };
    match src.as_bytes()[i] {
        // Constrained quotes: the marker must hug its content (no space just
        // inside the open marker), matching the parser's naive rule.
        b'*' => quote(
            |children, span| Inline::Strong { children, span },
            match_quoted(src, i, b'*', base, attrs),
        ),
        b'_' => quote(
            |children, span| Inline::Emphasis { children, span },
            match_quoted(src, i, b'_', base, attrs),
        ),
        b'`' => quote(
            |children, span| Inline::Monospace { children, span },
            match_quoted(src, i, b'`', base, attrs),
        ),
        b'#' => quote(
            |children, span| Inline::Mark { children, span },
            match_quoted(src, i, b'#', base, attrs),
        ),
        // Unconstrained quotes: match anywhere (incl. mid-word), but the content
        // may not be empty or contain whitespace.
        b'^' => quote(
            |children, span| Inline::Superscript { children, span },
            match_unconstrained(src, i, b'^', base, attrs),
        ),
        b'~' => quote(
            |children, span| Inline::Subscript { children, span },
            match_unconstrained(src, i, b'~', base, attrs),
        ),
        b'{' => match_attribute(src, i, base, attrs),
        _ => None,
    }
}

/// A constrained quote `*...*` / `_..._`: open marker at `i`, first matching
/// marker closes it, and the char right after the open must not be a space
/// (matching the current naive rule). Returns the parsed children and the
/// absolute end offset (one past the closing marker).
fn match_quoted(
    src: &str,
    i: usize,
    marker: u8,
    base: usize,
    attrs: &Attributes,
) -> Option<(Vec<Inline>, usize)> {
    let bytes = src.as_bytes();
    let inner_start = i + 1;
    if bytes.get(inner_start) == Some(&b' ') {
        return None;
    }
    let rel_close = src[inner_start..].find(marker as char)?;
    let close = inner_start + rel_close;
    let children = scan_inlines(&src[inner_start..close], base + inner_start, attrs);
    Some((children, base + close + 1))
}

/// An unconstrained quote `^...^` / `~...~`: open marker at `i`, first matching
/// marker closes it. Unlike [`match_quoted`] there is no word-boundary rule (it
/// matches mid-word, e.g. `E=mc^2^`), but the content must be non-empty and
/// contain no whitespace — otherwise the marker is just text. Returns the parsed
/// children and the absolute end offset (one past the closing marker).
fn match_unconstrained(
    src: &str,
    i: usize,
    marker: u8,
    base: usize,
    attrs: &Attributes,
) -> Option<(Vec<Inline>, usize)> {
    let inner_start = i + 1;
    let rel_close = src[inner_start..].find(marker as char)?;
    let close = inner_start + rel_close;
    let inner = &src[inner_start..close];
    if inner.is_empty() || inner.contains(char::is_whitespace) {
        return None;
    }
    let children = scan_inlines(inner, base + inner_start, attrs);
    Some((children, base + close + 1))
}

/// An attribute reference `{name}`. The name must be non-empty and contain no
/// whitespace (same shape as a header attribute key); otherwise `{` is just
/// text.
fn match_attribute(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    let rel_close = src[i + 1..].find('}')?;
    let name = &src[i + 1..i + 1 + rel_close];
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let end = i + 1 + rel_close + 1;
    let node = Inline::AttributeRef {
        name: name.to_string(),
        value: attrs.get(name).map(str::to_string),
        span: Span::new(base + i, base + end),
    };
    Some((node, end))
}

/// Emit a `Text` node for `src[a..b]` if non-empty, folding newlines to spaces
/// in the content while the span stays anchored to the real bytes.
fn flush_text(src: &str, a: usize, b: usize, base: usize, out: &mut Vec<Inline>) {
    if a >= b {
        return;
    }
    let raw = &src[a..b];
    let value = if raw.contains(['\n', '\r']) {
        raw.replace(['\n', '\r'], " ")
    } else {
        raw.to_string()
    };
    out.push(Inline::Text {
        value,
        span: Span::new(base + a, base + b),
    });
}

/// Byte width of the UTF-8 char starting at `i` (at least 1).
fn utf8_step(src: &str, i: usize) -> usize {
    let b = src.as_bytes()[i];
    if b < 0x80 {
        1
    } else if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else if b >= 0xC0 {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs() -> Attributes {
        Attributes::new()
    }

    #[test]
    fn plain_text_is_one_node() {
        let v = scan_inlines("hello", 0, &attrs());
        assert_eq!(
            v,
            vec![Inline::Text {
                value: "hello".into(),
                span: Span::new(0, 5)
            }]
        );
    }

    #[test]
    fn strong_span_covers_markers() {
        let src = "hi *there*";
        let v = scan_inlines(src, 0, &attrs());
        let span = v.iter().find_map(|n| match n {
            Inline::Strong { span, .. } => Some(*span),
            _ => None,
        });
        assert_eq!(&src[span.unwrap().start..span.unwrap().end], "*there*");
    }

    #[test]
    fn base_offset_is_applied() {
        // src starts at byte 100 of some larger document
        let v = scan_inlines("*x*", 100, &attrs());
        assert_eq!(v[0].span(), Span::new(100, 103));
    }

    #[test]
    fn attribute_reference_resolves() {
        let mut a = Attributes::new();
        a.set("v", "9");
        let v = scan_inlines("Version {v}", 0, &a);
        let attr = v.iter().find_map(|n| match n {
            Inline::AttributeRef { value, span, .. } => Some((value.clone(), *span)),
            _ => None,
        });
        let (value, span) = attr.unwrap();
        assert_eq!(value, Some("9".to_string()));
        // span points at the `{v}` token, not the resolved value
        assert_eq!(span, Span::new(8, 11));
    }

    #[test]
    fn missing_attribute_has_no_value() {
        let v = scan_inlines("{nope}", 0, &attrs());
        assert!(matches!(&v[0], Inline::AttributeRef { value: None, .. }));
    }

    #[test]
    fn emphasis_before_strong_both_parse() {
        // the old scanner missed the emphasis when a strong followed it
        let v = scan_inlines("_em_ *st*", 0, &attrs());
        assert!(v.iter().any(|n| matches!(n, Inline::Emphasis { .. })));
        assert!(v.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }

    #[test]
    fn newlines_fold_to_spaces_but_span_is_real() {
        let src = "a\nb";
        let v = scan_inlines(src, 0, &attrs());
        assert_eq!(
            v[0],
            Inline::Text {
                value: "a b".into(),
                span: Span::new(0, 3)
            }
        );
    }

    #[test]
    fn monospace_span_covers_markers() {
        let src = "use `code` here";
        let v = scan_inlines(src, 0, &attrs());
        let span = v.iter().find_map(|n| match n {
            Inline::Monospace { span, .. } => Some(*span),
            _ => None,
        });
        assert_eq!(&src[span.unwrap().start..span.unwrap().end], "`code`");
    }

    #[test]
    fn mark_span_covers_markers() {
        let src = "a #hot# word";
        let v = scan_inlines(src, 0, &attrs());
        let span = v.iter().find_map(|n| match n {
            Inline::Mark { span, .. } => Some(*span),
            _ => None,
        });
        assert_eq!(&src[span.unwrap().start..span.unwrap().end], "#hot#");
    }

    #[test]
    fn superscript_matches_mid_word() {
        let src = "E=mc^2^";
        let v = scan_inlines(src, 0, &attrs());
        let sup = v.iter().find_map(|n| match n {
            Inline::Superscript { children, span } => Some((children.clone(), *span)),
            _ => None,
        });
        let (children, span) = sup.expect("superscript not parsed");
        assert_eq!(&src[span.start..span.end], "^2^");
        assert_eq!(
            children,
            vec![Inline::Text {
                value: "2".into(),
                span: Span::new(5, 6)
            }]
        );
    }

    #[test]
    fn subscript_matches_mid_word() {
        let src = "H~2~O";
        let v = scan_inlines(src, 0, &attrs());
        let span = v.iter().find_map(|n| match n {
            Inline::Subscript { span, .. } => Some(*span),
            _ => None,
        });
        assert_eq!(&src[span.unwrap().start..span.unwrap().end], "~2~");
        // text right after the subscript still parses
        assert!(matches!(v.last(), Some(Inline::Text { value, .. }) if value == "O"));
    }

    #[test]
    fn unconstrained_with_space_stays_text() {
        // asciidoctor does not render `~a b~` — whitespace inside breaks the match
        let v = scan_inlines("x ~a b~ y", 0, &attrs());
        assert!(v.iter().all(|n| matches!(n, Inline::Text { .. })));
    }

    #[test]
    fn monospace_parses_nested_markup() {
        // `*bold* code` -> Monospace whose children include a Strong node
        let v = scan_inlines("`*bold* code`", 0, &attrs());
        let children = v
            .iter()
            .find_map(|n| match n {
                Inline::Monospace { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("monospace not parsed");
        assert!(children.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }
}

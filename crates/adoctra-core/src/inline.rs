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
        // Link macros (`link:`/`mailto:`) need brackets and — matching
        // asciidoctor — no left word boundary. A scheme (`https://` etc.) is a
        // bare autolink or, when followed by `[text]`, a URL macro; both want a
        // left boundary so a scheme glued to a word is not linked.
        b'l' => match_link_macro(src, i, base, attrs, "link:"),
        b'm' => match_link_macro(src, i, base, attrs, "mailto:"),
        b'h' | b'f' | b'i' => match_url(src, i, base, attrs),
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

/// Schemes that autolink as a bare URL or a URL macro. `mailto:` is handled by
/// [`match_link_macro`] instead (it has no `//` and is *not* bare-autolinked,
/// matching asciidoctor).
const URL_SCHEMES: [&str; 4] = ["https://", "http://", "ftp://", "irc://"];

/// Trailing punctuation stripped from a *bare* autolinked URL so a sentence's
/// period (etc.) stays outside the link. The set is pinned against asciidoctor
/// 2.0.23; a port colon like `:8080` is kept because only the trailing run is
/// trimmed.
fn trim_trailing_url_punct(url: &str) -> &str {
    url.trim_end_matches([',', '.', ';', ':', '!', '?', ')'])
}

/// Whether byte `i` may start a bare/URL-macro autolink. asciidoctor only
/// autolinks a scheme at the start of the text, after whitespace, or after one
/// of a small set of "opening" chars — *not* after a letter (`x_https://`) and
/// crucially *not* after `:` (so `link:url`/`word:url` without brackets stay
/// literal, with the `link:` macro form handled by [`match_link_macro`]).
fn at_left_boundary(src: &str, i: usize) -> bool {
    match src[..i].chars().next_back() {
        None => true,
        Some(prev) => {
            prev.is_whitespace()
                || matches!(prev, '<' | '>' | '(' | ')' | '[' | ']' | ';' | '"' | '\'')
        }
    }
}

/// The `link:target[text]` / `mailto:target[text]` macro. Brackets are
/// mandatory (a bare `link:url` is literal text); the target may not be empty
/// or contain whitespace. `link:` puts the bare target in the href, `mailto:`
/// keeps its scheme. Returns the node and the src-relative end index.
fn match_link_macro(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
    prefix: &str,
) -> Option<(Inline, usize)> {
    if !src[i..].starts_with(prefix) {
        return None;
    }
    let target_start = i + prefix.len();
    let rel_bracket = src[target_start..].find('[')?;
    let target = &src[target_start..target_start + rel_bracket];
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    let bracket = target_start + rel_bracket;
    let href = if prefix == "mailto:" {
        format!("mailto:{target}")
    } else {
        target.to_string()
    };
    finish_bracketed_link(src, i, bracket, &href, target, base, attrs)
}

/// A scheme-prefixed URL: either a URL macro `https://h[text]` or a bare
/// autolink `https://h`. Both require a left word boundary. Returns the node
/// and the src-relative end index.
fn match_url(src: &str, i: usize, base: usize, attrs: &Attributes) -> Option<(Inline, usize)> {
    if !at_left_boundary(src, i) {
        return None;
    }
    let scheme = URL_SCHEMES.iter().find(|s| src[i..].starts_with(**s))?;
    // The URL run ends at whitespace, a `[`, or end of slice.
    let rest = &src[i..];
    let run_len = rest
        .find(|c: char| c.is_whitespace() || c == '[')
        .unwrap_or(rest.len());
    // There must be something after the `scheme://`.
    if run_len <= scheme.len() {
        return None;
    }
    let run_end = i + run_len;
    // URL-macro form: the run is immediately followed by `[text]`. If the
    // bracket has no close it is not a macro — fall through to a bare autolink.
    if src.as_bytes().get(run_end) == Some(&b'[') {
        let target = &src[i..run_end];
        if let Some(node) = finish_bracketed_link(src, i, run_end, target, target, base, attrs) {
            return Some(node);
        }
    }
    // Bare autolink: trim a sentence's trailing punctuation off the run.
    let trimmed = trim_trailing_url_punct(&src[i..run_end]);
    if trimmed.len() <= scheme.len() {
        return None;
    }
    let end = i + trimmed.len();
    let span = Span::new(base + i, base + end);
    Some((
        Inline::Link {
            target: trimmed.to_string(),
            children: vec![Inline::Text {
                value: trimmed.to_string(),
                span,
            }],
            bare: true,
            span,
        },
        end,
    ))
}

/// Finish a link whose display text is in `[...]` starting at byte `bracket`.
/// `href` is the resolved target; `bare_text` is the text used when the
/// brackets are empty (asciidoctor renders an empty-text link `class="bare"`).
/// The text inside the brackets is scanned recursively so markup inside a link
/// (`link:url[*bold*]`) still parses. Returns `None` if the bracket is not
/// closed. The end index is src-relative (one past the `]`).
fn finish_bracketed_link(
    src: &str,
    start: usize,
    bracket: usize,
    href: &str,
    bare_text: &str,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    let text_start = bracket + 1;
    let rel_close = src[text_start..].find(']')?;
    let close = text_start + rel_close;
    let text = &src[text_start..close];
    let end = close + 1;
    let (children, bare) = if text.is_empty() {
        (
            vec![Inline::Text {
                value: bare_text.to_string(),
                span: Span::new(base + start, base + bracket),
            }],
            true,
        )
    } else {
        (scan_inlines(text, base + text_start, attrs), false)
    };
    Some((
        Inline::Link {
            target: href.to_string(),
            children,
            bare,
            span: Span::new(base + start, base + end),
        },
        end,
    ))
}

/// Emit a `Text` node for `src[a..b]` if non-empty, folding newlines to spaces
/// and applying typographic replacements (`(C)`, `--`, `...`, `'`, arrows) in
/// the content while the span stays anchored to the real bytes — so a `Text`'s
/// value is what the source *means* and its span is where it *came from*.
fn flush_text(src: &str, a: usize, b: usize, base: usize, out: &mut Vec<Inline>) {
    if a >= b {
        return;
    }
    let raw = &src[a..b];
    let folded = if raw.contains(['\n', '\r']) {
        raw.replace(['\n', '\r'], " ")
    } else {
        raw.to_string()
    };
    let value = match crate::replacements::apply(&folded) {
        std::borrow::Cow::Borrowed(_) => folded,
        std::borrow::Cow::Owned(s) => s,
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

    /// The single link node in `v`, or `None`.
    fn first_link(v: &[Inline]) -> Option<(String, Vec<Inline>, bool, Span)> {
        v.iter().find_map(|n| match n {
            Inline::Link {
                target,
                children,
                bare,
                span,
            } => Some((target.clone(), children.clone(), *bare, *span)),
            _ => None,
        })
    }

    #[test]
    fn link_macro_span_target_and_text() {
        let src = "See link:https://example.org[the site] now.";
        let v = scan_inlines(src, 0, &attrs());
        let (target, children, bare, span) = first_link(&v).expect("no link");
        assert_eq!(
            &src[span.start..span.end],
            "link:https://example.org[the site]"
        );
        assert_eq!(target, "https://example.org");
        assert!(!bare);
        assert_eq!(
            children,
            vec![Inline::Text {
                value: "the site".into(),
                span: Span::new(29, 37),
            }]
        );
    }

    #[test]
    fn mailto_macro_keeps_its_scheme_in_href() {
        let v = scan_inlines("Mail mailto:me@x.io[me] now.", 0, &attrs());
        let (target, _, bare, _) = first_link(&v).expect("no link");
        assert_eq!(target, "mailto:me@x.io");
        assert!(!bare);
    }

    #[test]
    fn url_macro_without_link_prefix() {
        let src = "Go https://example.org[here].";
        let v = scan_inlines(src, 0, &attrs());
        let (target, _, _, span) = first_link(&v).expect("no link");
        assert_eq!(&src[span.start..span.end], "https://example.org[here]");
        assert_eq!(target, "https://example.org");
    }

    #[test]
    fn bare_autolink_trims_trailing_period() {
        let src = "End https://example.org.";
        let v = scan_inlines(src, 0, &attrs());
        let (target, _, bare, span) = first_link(&v).expect("no link");
        // The span (and href) stop before the sentence's period.
        assert_eq!(&src[span.start..span.end], "https://example.org");
        assert_eq!(target, "https://example.org");
        assert!(bare);
        // The trailing period survives as its own text node.
        assert!(matches!(v.last(), Some(Inline::Text { value, .. }) if value == "."));
    }

    #[test]
    fn link_text_parses_inline_markup() {
        let v = scan_inlines("link:https://h[*bold* text]", 0, &attrs());
        let (_, children, _, _) = first_link(&v).expect("no link");
        assert!(children.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }

    #[test]
    fn empty_brackets_make_a_bare_link() {
        let v = scan_inlines("link:https://h[]", 0, &attrs());
        let (target, children, bare, _) = first_link(&v).expect("no link");
        assert!(bare);
        assert_eq!(target, "https://h");
        assert!(matches!(&children[0], Inline::Text { value, .. } if value == "https://h"));
    }

    #[test]
    fn scheme_glued_to_a_word_is_not_autolinked() {
        // Left-boundary rule: `x_https://...` is not a link (matches asciidoctor).
        let v = scan_inlines("x_https://example.org here", 0, &attrs());
        assert!(first_link(&v).is_none());
    }

    #[test]
    fn link_macro_without_brackets_is_literal() {
        // `link:` needs `[...]`; without it the text is plain.
        let v = scan_inlines("link:https://example.org plain", 0, &attrs());
        assert!(first_link(&v).is_none());
    }

    #[test]
    fn bare_mailto_is_not_autolinked() {
        // asciidoctor does not autolink a bare `mailto:` (no brackets).
        let v = scan_inlines("Mail mailto:me@x.io now", 0, &attrs());
        assert!(first_link(&v).is_none());
    }

    #[test]
    fn ftp_scheme_autolinks() {
        let v = scan_inlines("Get ftp://ftp.example.org/f now", 0, &attrs());
        let (target, _, bare, _) = first_link(&v).expect("no link");
        assert_eq!(target, "ftp://ftp.example.org/f");
        assert!(bare);
    }

    #[test]
    fn bare_link_inside_strong_still_links() {
        // The recursive scanner handles `*https://h*` for free: Strong wraps a
        // bare Link.
        let v = scan_inlines("*https://h*", 0, &attrs());
        let children = v
            .iter()
            .find_map(|n| match n {
                Inline::Strong { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no strong");
        assert!(children.iter().any(|n| matches!(n, Inline::Link { .. })));
    }

    #[test]
    fn replacement_resolves_value_but_span_stays_on_source() {
        // `(C)` (3 source bytes) resolves to a single © char; the Text span still
        // covers the original bytes, so value length and span length differ.
        let v = scan_inlines("(C) 2026", 0, &attrs());
        assert_eq!(
            v,
            vec![Inline::Text {
                value: "\u{a9} 2026".into(),
                span: Span::new(0, 8),
            }]
        );
    }

    #[test]
    fn replacements_apply_inside_formatting() {
        // asciidoctor applies replacements within formatted text too.
        let v = scan_inlines("*it's*", 0, &attrs());
        let children = v
            .iter()
            .find_map(|n| match n {
                Inline::Strong { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no strong");
        assert_eq!(
            children,
            vec![Inline::Text {
                value: "it\u{2019}s".into(),
                span: Span::new(1, 5),
            }]
        );
    }
}

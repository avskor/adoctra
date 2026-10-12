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
use crate::parser::is_valid_anchor_id;

/// Scan inline markup in `src`. `base` is the absolute byte offset of `src[0]`
/// in the original source; `attrs` resolves `{name}` references. Newlines in text
/// content fold to spaces (asciidoctor's soft wrap); use [`scan_inlines_with`] to
/// keep them.
pub(crate) fn scan_inlines(src: &str, base: usize, attrs: &Attributes) -> Vec<Inline> {
    scan_inlines_with(src, base, attrs, true)
}

/// Like [`scan_inlines`], but `fold_newlines` chooses whether newlines in *text*
/// content collapse to spaces. A paragraph folds (a soft line wrap is just a
/// space); a verse block does **not** — its line breaks are significant and render
/// inside a `<pre>`. A newline *inside* an inline-markup span still folds (markup
/// children are scanned through the regular folding path), but markup straddling a
/// line break is vanishingly rare in verse.
pub(crate) fn scan_inlines_with(
    src: &str,
    base: usize,
    attrs: &Attributes,
    fold_newlines: bool,
) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut text_start = 0usize;
    let mut i = 0usize;

    while i < src.len() {
        if let Some((node, end)) = try_match(src, i, base, attrs) {
            flush_text(src, text_start, i, base, fold_newlines, &mut out);
            out.push(node);
            i = end;
            text_start = end;
        } else {
            // advance one whole UTF-8 char
            i += utf8_step(src, i);
        }
    }
    flush_text(src, text_start, src.len(), base, fold_newlines, &mut out);
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
        // Cross-references: the `<<id>>` / `<<id,text>>` angle form, and the
        // `xref:id[text]` macro (brackets mandatory, like `link:`).
        b'<' => match_xref_angle(src, i, base, attrs),
        b'x' => match_xref_macro(src, i, base, attrs),
        // Inline anchors: the `[[id]]` / `[[id,reftext]]` shorthand mid-text, and
        // the `anchor:id[reftext]` macro (brackets mandatory, like `xref:`).
        b'[' => match_inline_anchor(src, i, base, attrs),
        b'a' => match_anchor_macro(src, i, base, attrs),
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
        parts: attrs.raw_parts(name).unwrap_or_default().to_vec(),
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

/// A cross-reference in angle-bracket form: `<<id>>` or `<<id,text>>`. Requires
/// the doubled `<<` … `>>` delimiters; the content is split on the *first* comma
/// into the raw `target` id and an optional reftext. The reftext is trimmed and
/// scanned for inline markup (`<<id,*bold*>>` works). With no reftext (or an
/// empty one) the display text is the core-generated `[id]`, matching
/// asciidoctor's rendering of an unresolved reference. An empty `target`, or a
/// missing `>>`, is not a cross-reference (the `<` stays literal text). Returns
/// the node and the src-relative end index (one past the closing `>>`).
fn match_xref_angle(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    if !src[i..].starts_with("<<") {
        return None;
    }
    let inner_start = i + 2;
    let rel_close = src[inner_start..].find(">>")?;
    let inner = &src[inner_start..inner_start + rel_close];
    let end = inner_start + rel_close + 2;
    // Split target from reftext on the first comma; keep the reftext's absolute
    // offset so its scanned children carry real source spans.
    let (target, reftext, reftext_off) = match inner.find(',') {
        Some(comma) => {
            let after = &inner[comma + 1..];
            let lead = after.len() - after.trim_start().len();
            (
                &inner[..comma],
                after.trim(),
                inner_start + comma + 1 + lead,
            )
        }
        None => (inner, "", inner_start),
    };
    if target.is_empty() {
        return None;
    }
    let children = xref_children(reftext, reftext_off, base, attrs);
    Some((
        Inline::CrossReference {
            target: target.to_string(),
            children,
            span: Span::new(base + i, base + end),
        },
        end,
    ))
}

/// The `xref:id[text]` macro. Brackets are mandatory (like `link:`); `target` is
/// the id up to `[` (non-empty, no whitespace). Empty brackets leave the display
/// text empty for the resolve pass to fill in, like the textless angle form.
/// Returns the node and the src-relative end index (one past the `]`).
fn match_xref_macro(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    const PREFIX: &str = "xref:";
    if !src[i..].starts_with(PREFIX) {
        return None;
    }
    let target_start = i + PREFIX.len();
    let rel_bracket = src[target_start..].find('[')?;
    let target = &src[target_start..target_start + rel_bracket];
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    let text_start = target_start + rel_bracket + 1;
    let rel_close = src[text_start..].find(']')?;
    let close = text_start + rel_close;
    let end = close + 1;
    let children = xref_children(&src[text_start..close], text_start, base, attrs);
    Some((
        Inline::CrossReference {
            target: target.to_string(),
            children,
            span: Span::new(base + i, base + end),
        },
        end,
    ))
}

/// The display children of a cross-reference: the scanned `reftext` (at absolute
/// offset `base + reftext_off`) when non-empty, otherwise **empty** — a textless
/// reference has no display text until the resolve pass fills it in (with the
/// section title, or the `[id]` fallback for an unknown id).
fn xref_children(
    reftext: &str,
    reftext_off: usize,
    base: usize,
    attrs: &Attributes,
) -> Vec<Inline> {
    if reftext.is_empty() {
        Vec::new()
    } else {
        scan_inlines(reftext, base + reftext_off, attrs)
    }
}

/// An inline anchor in shorthand form: `[[id]]` or `[[id,reftext]]` appearing
/// mid-text. Requires the doubled `[[` … `]]` delimiters; the content is split on
/// the first comma into the raw `id` and an optional reftext (the reftext is
/// trimmed, like the angle xref form). The id must be well-formed (see
/// [`is_valid_anchor_id`]) — otherwise the `[` stays literal text, matching
/// asciidoctor, which treats only a well-formed anchor specially. The reftext is
/// scanned for inline markup (`[[id,*bold*]]` works) but is **not** shown at the
/// anchor site: the resolve pass uses it as the display text of a textless
/// `<<id>>` to this anchor. Returns the node and the src-relative end index (one
/// past the closing `]]`).
fn match_inline_anchor(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    if !src[i..].starts_with("[[") {
        return None;
    }
    let inner_start = i + 2;
    let rel_close = src[inner_start..].find("]]")?;
    let inner = &src[inner_start..inner_start + rel_close];
    let end = inner_start + rel_close + 2;
    // Split id from reftext on the first comma; keep the reftext's absolute offset
    // so its scanned children carry real source spans.
    let (id, reftext, reftext_off) = match inner.find(',') {
        Some(comma) => {
            let after = &inner[comma + 1..];
            let lead = after.len() - after.trim_start().len();
            (
                &inner[..comma],
                after.trim(),
                inner_start + comma + 1 + lead,
            )
        }
        None => (inner, "", inner_start),
    };
    if !is_valid_anchor_id(id) {
        return None;
    }
    Some((
        Inline::Anchor {
            id: id.to_string(),
            reftext: anchor_reftext(reftext, reftext_off, base, attrs),
            span: Span::new(base + i, base + end),
        },
        end,
    ))
}

/// The `anchor:id[reftext]` macro — an alternative spelling of the `[[id]]`
/// inline anchor. Brackets are mandatory (like `xref:`); `id` is the text up to
/// `[` and must be well-formed. The bracket content is the reftext (used as-is,
/// like the xref macro), and may be empty. Renders identically to the shorthand.
/// Returns the node and the src-relative end index (one past the `]`).
fn match_anchor_macro(
    src: &str,
    i: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<(Inline, usize)> {
    const PREFIX: &str = "anchor:";
    if !src[i..].starts_with(PREFIX) {
        return None;
    }
    let id_start = i + PREFIX.len();
    let rel_bracket = src[id_start..].find('[')?;
    let id = &src[id_start..id_start + rel_bracket];
    if !is_valid_anchor_id(id) {
        return None;
    }
    let text_start = id_start + rel_bracket + 1;
    let rel_close = src[text_start..].find(']')?;
    let close = text_start + rel_close;
    let end = close + 1;
    Some((
        Inline::Anchor {
            id: id.to_string(),
            reftext: anchor_reftext(&src[text_start..close], text_start, base, attrs),
            span: Span::new(base + i, base + end),
        },
        end,
    ))
}

/// The reftext children of an inline anchor: the scanned `reftext` (at absolute
/// offset `base + reftext_off`) when non-empty, otherwise `None`. Unlike a
/// cross-reference, an anchor never renders this text at its own site — it only
/// feeds the resolve pass — so an absent reftext is `None`, not an empty `Vec`.
fn anchor_reftext(
    reftext: &str,
    reftext_off: usize,
    base: usize,
    attrs: &Attributes,
) -> Option<Vec<Inline>> {
    if reftext.is_empty() {
        None
    } else {
        Some(scan_inlines(reftext, base + reftext_off, attrs))
    }
}

/// Emit a `Text` node for `src[a..b]` if non-empty, folding newlines to spaces
/// (when `fold_newlines`) and applying typographic replacements (`(C)`, `--`,
/// `...`, `'`, arrows) in the content while the span stays anchored to the real
/// bytes — so a `Text`'s value is what the source *means* and its span is where it
/// *came from*.
fn flush_text(
    src: &str,
    a: usize,
    b: usize,
    base: usize,
    fold_newlines: bool,
    out: &mut Vec<Inline>,
) {
    if a >= b {
        return;
    }
    let raw = &src[a..b];
    let folded = if fold_newlines && raw.contains(['\n', '\r']) {
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

    /// The single cross-reference node in `v`, or `None`.
    fn first_xref(v: &[Inline]) -> Option<(String, Vec<Inline>, Span)> {
        v.iter().find_map(|n| match n {
            Inline::CrossReference {
                target,
                children,
                span,
            } => Some((target.clone(), children.clone(), *span)),
            _ => None,
        })
    }

    #[test]
    fn xref_angle_with_text_span_target_and_children() {
        let src = "See <<sec,Section Two>> now.";
        let v = scan_inlines(src, 0, &attrs());
        let (target, children, span) = first_xref(&v).expect("no xref");
        assert_eq!(&src[span.start..span.end], "<<sec,Section Two>>");
        assert_eq!(target, "sec");
        // The reftext is scanned at its real offset (spans point at real bytes).
        assert_eq!(
            children,
            vec![Inline::Text {
                value: "Section Two".into(),
                span: Span::new(10, 21),
            }]
        );
    }

    #[test]
    fn xref_angle_without_text_has_empty_children() {
        let src = "<<sec>>";
        let v = scan_inlines(src, 0, &attrs());
        let (target, children, span) = first_xref(&v).expect("no xref");
        assert_eq!(target, "sec");
        assert_eq!(&src[span.start..span.end], "<<sec>>");
        // Scan-level: a textless reference has no display text yet; the resolve
        // pass fills it in (section title, or the `[sec]` unresolved fallback).
        assert!(children.is_empty());
    }

    #[test]
    fn xref_macro_form_span_and_target() {
        let src = "Go xref:sec[Mac].";
        let v = scan_inlines(src, 0, &attrs());
        let (target, children, span) = first_xref(&v).expect("no xref");
        assert_eq!(&src[span.start..span.end], "xref:sec[Mac]");
        assert_eq!(target, "sec");
        assert!(matches!(&children[0], Inline::Text { value, .. } if value == "Mac"));
    }

    #[test]
    fn xref_macro_empty_brackets_have_empty_children() {
        let v = scan_inlines("xref:sec[]", 0, &attrs());
        let (target, children, _) = first_xref(&v).expect("no xref");
        assert_eq!(target, "sec");
        // Like the textless angle form: display text is filled in by resolve.
        assert!(children.is_empty());
    }

    #[test]
    fn xref_reftext_parses_inline_markup() {
        // The reftext is scanned recursively, so markup inside it still parses.
        let v = scan_inlines("<<id,*bold* ref>>", 0, &attrs());
        let (_, children, _) = first_xref(&v).expect("no xref");
        assert!(children.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }

    #[test]
    fn unterminated_angle_xref_stays_text() {
        // `<<` with no closing `>>` is not a cross-reference.
        let v = scan_inlines("a << b c", 0, &attrs());
        assert!(first_xref(&v).is_none());
    }

    #[test]
    fn xref_macro_without_brackets_is_literal() {
        // `xref:` needs `[...]`; without it the text is plain.
        let v = scan_inlines("xref:sec plain", 0, &attrs());
        assert!(first_xref(&v).is_none());
    }

    #[test]
    fn empty_target_angle_is_literal() {
        // `<<>>` and `<<,text>>` have no id — not a cross-reference.
        assert!(first_xref(&scan_inlines("<<>>", 0, &attrs())).is_none());
        assert!(first_xref(&scan_inlines("<<,text>>", 0, &attrs())).is_none());
    }

    #[test]
    fn lone_angle_does_not_break_text() {
        // A single `<` (e.g. before the arrow replacement) is just text.
        let v = scan_inlines("a < b", 0, &attrs());
        assert!(first_xref(&v).is_none());
        assert!(v.iter().all(|n| matches!(n, Inline::Text { .. })));
    }

    #[test]
    fn x_word_that_is_not_xref_stays_text() {
        // The `b'x'` dispatch arm must not split ordinary words starting with x.
        let v = scan_inlines("extra example", 0, &attrs());
        assert_eq!(
            v,
            vec![Inline::Text {
                value: "extra example".into(),
                span: Span::new(0, 13),
            }]
        );
    }

    fn first_anchor(v: &[Inline]) -> Option<(String, Option<Vec<Inline>>, Span)> {
        v.iter().find_map(|n| match n {
            Inline::Anchor { id, reftext, span } => Some((id.clone(), reftext.clone(), *span)),
            _ => None,
        })
    }

    #[test]
    fn inline_anchor_span_covers_construct_and_has_no_reftext() {
        let src = "see [[point]]here";
        let v = scan_inlines(src, 0, &attrs());
        let (id, reftext, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "point");
        assert_eq!(reftext, None);
        assert_eq!(&src[span.start..span.end], "[[point]]");
        // The text on either side stays put (the anchor renders empty in place).
        assert!(matches!(&v[0], Inline::Text { value, .. } if value == "see "));
        assert!(matches!(v.last(), Some(Inline::Text { value, .. }) if value == "here"));
    }

    #[test]
    fn inline_anchor_reftext_parses_inline_markup() {
        // The reftext after the comma is scanned recursively (markup applies) and
        // its spans point at real bytes.
        let src = "[[id,*bold* ref]]x";
        let v = scan_inlines(src, 0, &attrs());
        let (id, reftext, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "id");
        assert_eq!(&src[span.start..span.end], "[[id,*bold* ref]]");
        let rt = reftext.expect("reftext");
        assert!(rt.iter().any(|n| matches!(n, Inline::Strong { .. })));
    }

    #[test]
    fn anchor_macro_form_span_and_id() {
        let src = "go anchor:point[the part] now";
        let v = scan_inlines(src, 0, &attrs());
        let (id, reftext, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "point");
        assert_eq!(&src[span.start..span.end], "anchor:point[the part]");
        assert!(matches!(&reftext.unwrap()[0], Inline::Text { value, .. } if value == "the part"));
    }

    #[test]
    fn anchor_macro_empty_brackets_have_no_reftext() {
        let v = scan_inlines("anchor:point[]", 0, &attrs());
        let (id, reftext, _) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "point");
        assert_eq!(reftext, None);
    }

    #[test]
    fn malformed_inline_anchor_stays_text() {
        // A space in the id, empty brackets, or no closing `]]` is not an anchor.
        assert!(first_anchor(&scan_inlines("a [[a b]] c", 0, &attrs())).is_none());
        assert!(first_anchor(&scan_inlines("a [[]] c", 0, &attrs())).is_none());
        assert!(first_anchor(&scan_inlines("a [[unclosed c", 0, &attrs())).is_none());
    }

    #[test]
    fn anchor_word_that_is_not_a_macro_stays_text() {
        // The `b'a'` dispatch arm must not disturb ordinary words starting with a.
        let v = scan_inlines("an anchor without brackets", 0, &attrs());
        assert!(first_anchor(&v).is_none());
        assert!(v.iter().all(|n| matches!(n, Inline::Text { .. })));
    }
}

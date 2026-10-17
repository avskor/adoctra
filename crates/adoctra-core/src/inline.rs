//! The single inline scanner.
//!
//! One pass over a slice of source produces spanned [`Inline`] nodes. Both the
//! parser (which builds the document model) and the tokenizer (which feeds
//! editor highlighting) drive off *this* function, so there is exactly one
//! place that understands `*strong*`, `_emphasis_`, `{attribute}` and macro
//! markup — no second grammar that can drift out of sync.
//!
//! Offsets are absolute: `base` is the byte offset of `src[0]` in the original
//! document, so every node's [`Span`] maps straight back to real source bytes.
//! Newlines inside `src` (a paragraph spanning several source lines) are folded
//! to spaces in text *content*, while the spans keep pointing at the real
//! bytes.

use crate::ast::{Inline, Span};
use crate::attributes::Attributes;
use crate::inline_quotes::Quotes;

/// Scan inline markup with the block's effective attributes.
#[cfg(test)]
pub(crate) fn scan_inlines(src: &str, base: usize, attrs: &Attributes) -> Vec<Inline> {
    scan_inlines_with(src, base, attrs, true)
}

/// `fold_newlines` preserves verse line breaks while keeping physical spans.
#[cfg(test)]
pub(crate) fn scan_inlines_with(
    src: &str,
    base: usize,
    attrs: &Attributes,
    fold_newlines: bool,
) -> Vec<Inline> {
    scan_inlines_report(src, base, attrs, fold_newlines, &mut Vec::new())
}

pub(crate) fn scan_inlines_report(
    src: &str,
    base: usize,
    attrs: &Attributes,
    fold_newlines: bool,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Vec<Inline> {
    let mut evaluation = crate::substitution_context::Evaluation::read_only(attrs);
    scan_inlines_evaluated(src, base, &mut evaluation, fold_newlines, diagnostics)
}

pub(crate) fn scan_inlines_evaluated(
    src: &str,
    base: usize,
    evaluation: &mut crate::substitution_context::Evaluation<'_>,
    fold_newlines: bool,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Vec<Inline> {
    let compat = evaluation.get("compat-mode").is_some();
    let mut quotes = Quotes::new(src, base, evaluation, compat);
    let mut out = scan_unprocessed(src, base, &quotes);
    crate::attribute_macros::apply_inlines(&mut out, evaluation, fold_newlines, &mut quotes.passes);
    quotes.passes.restore(&mut out, evaluation, compat);
    diagnostics.extend(quotes.passes.diagnostics);
    out
}

/// Preserve source syntax for highlighting even when a later macro consumes it.
/// Links are reported by the same late matcher that produces semantic parts.
pub(crate) fn scan_for_tokens(src: &str, base: usize, attrs: &Attributes) -> Vec<crate::Token> {
    let mut evaluation = crate::substitution_context::Evaluation::read_only(attrs);
    let mut quotes = Quotes::new(
        src,
        base,
        &mut evaluation,
        attrs.get("compat-mode").is_some(),
    );
    let mut syntax = scan_unprocessed(src, base, &quotes);
    let mut processed = syntax.clone();
    let matches = crate::attribute_macros::apply_inlines(
        &mut processed,
        &mut evaluation,
        true,
        &mut quotes.passes,
    );
    for span in matches.links {
        syntax.push(Inline::Link {
            target: String::new(),
            children: Vec::new(),
            bare: false,
            span,
        });
    }
    for span in matches.ui {
        syntax.push(Inline::Ui {
            ui: crate::UiMacro::Button { label: Vec::new() },
            span,
        });
    }
    for span in matches.images {
        syntax.push(Inline::Image {
            image: Default::default(),
            span,
        });
    }
    for span in matches.anchors {
        syntax.push(Inline::Anchor {
            id: String::new(),
            span,
        });
    }
    for span in matches.xrefs {
        syntax.push(Inline::CrossReference {
            xref: Default::default(),
            children: Vec::new(),
            span,
        });
    }
    syntax.extend(quotes.syntax());
    let mut tokens = Vec::new();
    crate::tokenizer::flatten_inlines(&syntax, &mut tokens);
    tokens.extend(quotes.passes.tokens());
    tokens
}

fn scan_unprocessed(src: &str, base: usize, quotes: &Quotes) -> Vec<Inline> {
    // Every macro, including cross-references and inline anchors, is
    // recognized later by the macros stage over the expanded text (see
    // `attribute_macros`), so references can complete or break them. Only
    // the quote/attribute edits are applied to the source text here.
    let mut out = Vec::new();
    let mut text_start = 0usize;
    for (&at, edit) in quotes.edits.range(base..base + src.len()) {
        let i = at - base;
        if i < text_start {
            continue;
        }
        flush_text(src, text_start, i, base, &mut out);
        out.extend(edit.nodes.clone());
        text_start = edit.end - base;
    }
    flush_text(src, text_start, src.len(), base, &mut out);
    quotes.rebuild(out, base..base + src.len())
}

/// Preserve source text until attribute expansion and the ordered replacements
/// pass have the complete neighboring inline run available.
fn flush_text(src: &str, a: usize, b: usize, base: usize, out: &mut Vec<Inline>) {
    if a < b {
        out.push(Inline::Text {
            value: src[a..b].into(),
            span: Span::new(base + a, base + b),
        });
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

    /// The first semantic link and its display parts and source coverage.
    fn first_link(v: &[Inline]) -> Option<(String, Vec<crate::AttributePart>, bool, Span)> {
        v.iter().find_map(|n| {
            let Inline::Substituted { parts, span, .. } = n else { return None };
            let [crate::AttributePart::OpenLink(link), label @ .., crate::AttributePart::CloseLink] = parts.as_slice() else { return None };
            Some((link.target.clone(), label.to_vec(), link.role.as_deref() == Some("bare"), *span))
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
            vec![crate::AttributePart::Text("the site".into())]
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
        assert_eq!(
            children,
            vec![
                crate::AttributePart::Open(crate::AttributeFormat::Strong),
                crate::AttributePart::Text("bold".into()),
                crate::AttributePart::Close(crate::AttributeFormat::Strong),
                crate::AttributePart::Text(" text".into()),
            ]
        );
    }

    #[test]
    fn empty_brackets_make_a_bare_link() {
        let v = scan_inlines("link:https://h[]", 0, &attrs());
        let (target, children, bare, _) = first_link(&v).expect("no link");
        assert!(bare);
        assert_eq!(target, "https://h");
        assert!(matches!(&children[0], crate::AttributePart::Text(value) if value == "https://h"));
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
        // Quote boundaries survive around a generated bare link.
        let v = scan_inlines("*https://h*", 0, &attrs());
        let children = v
            .iter()
            .find_map(|n| match n {
                Inline::Strong { children, .. } => Some(children.clone()),
                _ => None,
            })
            .expect("no strong");
        assert_eq!(first_link(&children).unwrap().0, "https://h");
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
                xref,
                children,
                span,
            } => Some((
                xref.refid.clone().unwrap_or_default(),
                children.clone(),
                *span,
            )),
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
        // Quotes run before the macros stage; the consumed label keeps the
        // formatting as semantic parts over the label's physical range.
        let src = "<<id,*bold* ref>>";
        let v = scan_inlines(src, 0, &attrs());
        let (_, children, _) = first_xref(&v).expect("no xref");
        let [Inline::Substituted { parts, span, .. }] = children.as_slice() else {
            panic!("{children:?}")
        };
        assert_eq!(&src[span.start..span.end], "*bold* ref");
        assert_eq!(
            parts,
            &vec![
                crate::AttributePart::Open(crate::AttributeFormat::Strong),
                crate::AttributePart::Text("bold".into()),
                crate::AttributePart::Close(crate::AttributeFormat::Strong),
                crate::AttributePart::Text(" ref".into()),
            ]
        );
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

    fn first_anchor(v: &[Inline]) -> Option<(String, Span)> {
        v.iter().find_map(|n| match n {
            Inline::Anchor { id, span } => Some((id.clone(), *span)),
            _ => None,
        })
    }

    #[test]
    fn inline_anchor_span_covers_construct() {
        let src = "see [[point]]here";
        let v = scan_inlines(src, 0, &attrs());
        let (id, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "point");
        assert_eq!(&src[span.start..span.end], "[[point]]");
        // The text on either side stays put (the anchor renders empty in place).
        assert!(matches!(&v[0], Inline::Text { value, .. } if value == "see "));
        assert!(matches!(v.last(), Some(Inline::Text { value, .. }) if value == "here"));
    }

    #[test]
    fn inline_anchor_consumes_formatted_reftext() {
        // Quotes run first; the anchor then consumes the formatted reftext
        // (registry metadata only) together with its boundaries.
        let src = "[[id,*bold* ref]]x";
        let v = scan_inlines(src, 0, &attrs());
        let (id, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "id");
        assert_eq!(&src[span.start..span.end], "[[id,*bold* ref]]");
        assert!(
            !v.iter().any(|n| matches!(n, Inline::Strong { .. })),
            "{v:?}"
        );
        assert!(matches!(v.last(), Some(Inline::Text { value, .. }) if value == "x"));
    }

    #[test]
    fn anchor_macro_form_span_and_id() {
        let src = "go anchor:point[the part] now";
        let v = scan_inlines(src, 0, &attrs());
        let (id, span) = first_anchor(&v).expect("no anchor");
        assert_eq!(id, "point");
        assert_eq!(&src[span.start..span.end], "anchor:point[the part]");
        let (id, _) = first_anchor(&scan_inlines("anchor:point[]", 0, &attrs())).unwrap();
        assert_eq!(id, "point");
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

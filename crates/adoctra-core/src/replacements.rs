//! Ordered replacements over the lexical view after attribute expansion.
//! The order and escape rules follow Asciidoctor 2.0.23 REPLACEMENTS.
use crate::substitution_text::{self, Text};
use crate::{AttributePart, Inline};
use std::borrow::Cow;

/// Formatting and macros bound runs; adjacent Text and AttributeRef values share
/// a stream. Children have markup on both sides, not artificial line boundaries.
pub(crate) fn apply_inlines(nodes: &mut Vec<Inline>, fold_newlines: bool) {
    process(nodes, fold_newlines, true);
}

/// Attribute entries may apply specialcharacters after replacements. Keep the
/// generated entity spelling raw so that a later stage can protect it, instead
/// of decoding it to the Unicode form used by ordinary inline replacements.
pub(crate) fn apply_parts(parts: Vec<AttributePart>) -> Vec<AttributePart> {
    let mut text = Text::from_parts(&parts);
    for stage in 0..13 {
        text = replace_stage(text, stage, true, true, false);
    }
    text.into_parts()
}

fn process(nodes: &mut Vec<Inline>, fold_newlines: bool, edges: bool) {
    let old = std::mem::take(nodes);
    let mut pending = Vec::new();
    let mut left_edge = edges;
    for mut node in old {
        if matches!(
            &node,
            Inline::Text { .. } | Inline::AttributeRef { value: Some(_), .. }
        ) {
            pending.push(node);
            continue;
        }
        flush(&mut pending, nodes, fold_newlines, left_edge, false);
        match &mut node {
            Inline::Strong { children, .. }
            | Inline::Emphasis { children, .. }
            | Inline::Monospace { children, .. }
            | Inline::Mark { children, .. }
            | Inline::Superscript { children, .. }
            | Inline::Subscript { children, .. }
            | Inline::CrossReference { children, .. }
            | Inline::Link {
                children,
                bare: false,
                ..
            } => process(children, true, false),
            Inline::Anchor {
                reftext: Some(children),
                ..
            } => process(children, true, false),
            _ => {}
        }
        nodes.push(node);
        left_edge = false;
    }
    flush(&mut pending, nodes, fold_newlines, left_edge, edges);
}

fn flush(pending: &mut Vec<Inline>, output: &mut Vec<Inline>, fold: bool, left: bool, right: bool) {
    if pending.is_empty() {
        return;
    }
    let mut text = Text::default();
    for (owner, node) in pending.iter().enumerate() {
        match node {
            Inline::Text { value, span } => text.append(value, false, owner, *span, true),
            Inline::AttributeRef {
                value: Some(value),
                parts,
                span,
                ..
            } => {
                if parts.is_empty() {
                    text.append(value, false, owner, *span, false);
                } else {
                    for part in parts {
                        text.append_part(part, owner, *span);
                    }
                }
            }
            _ => unreachable!(),
        }
    }
    for stage in 0..13 {
        text = replace_stage(text, stage, left, right, true);
    }
    output.extend(substitution_text::write_back(
        std::mem::take(pending),
        text,
        fold,
    ));
}

struct Match<'a> {
    end: usize,
    escape: Option<usize>,
    lexical: Cow<'a, str>,
    decoded: Option<&'a str>,
}

fn replace_stage(text: Text, stage: usize, left: bool, right: bool, decode: bool) -> Text {
    const TRIGGERS: &[u8; 13] = b"(((--.`'-=&&&";
    if !text.text.as_bytes().contains(&TRIGGERS[stage]) {
        return text;
    }
    let mut output = Text::default();
    let mut copied = 0;
    let mut offset = 0;
    while offset < text.text.len() {
        if let Some(m) = match_at(&text.text, offset, stage, left, right) {
            text.copy(copied..offset, &mut output);
            if let Some(escape) = m.escape {
                text.copy(offset..escape, &mut output);
                text.copy(escape + 1..m.end, &mut output);
            } else {
                text.replacement(
                    offset..m.end,
                    &m.lexical,
                    if decode { m.decoded } else { None },
                    &mut output,
                );
            }
            copied = m.end;
            offset = m.end;
        } else {
            offset += text.text[offset..].chars().next().unwrap().len_utf8();
        }
    }
    if copied == 0 {
        return text;
    }
    text.copy(copied..text.text.len(), &mut output);
    output
}

fn match_at(src: &str, at: usize, stage: usize, left: bool, right: bool) -> Option<Match<'_>> {
    let bytes = src.as_bytes();
    let escaped = bytes[at] == b'\\';
    let start = at + usize::from(escaped);
    let rest = &src[start..];
    let fixed = |pattern: &str, lexical, decoded| {
        rest.starts_with(pattern).then_some(Match {
            end: start + pattern.len(),
            escape: escaped.then_some(at),
            lexical: Cow::Borrowed(lexical),
            decoded: Some(decoded),
        })
    };
    match stage {
        0 => fixed("(C)", "&#169;", "©"),
        1 => fixed("(R)", "&#174;", "®"),
        2 => fixed("(TM)", "&#8482;", "™"),
        3 => {
            // The spaced dash consumes exactly one space/newline on each side.
            // Tabs do not qualify. Consuming the trailing blank matters for
            // adjacent dashes, as Ruby's gsub does not revisit that blank.
            let dash = if matches!(bytes[at], b' ' | b'\n' | b'\\') {
                at + 1
            } else if at == 0 && left {
                at
            } else {
                return None;
            };
            if !src[dash..].starts_with("--") {
                return None;
            }
            let end = dash + 2;
            let end = match bytes.get(end) {
                Some(b' ' | b'\n') => end + 1,
                None if right => end,
                _ => return None,
            };
            Some(Match {
                end,
                escape: escaped.then_some(at),
                lexical: Cow::Borrowed("&#8201;&#8212;&#8201;"),
                decoded: Some("\u{2009}—\u{2009}"),
            })
        }
        4 => {
            if !src[..at].chars().next_back().is_some_and(is_word)
                || !rest.starts_with("--")
                || !rest[2..].chars().next().is_some_and(is_word)
            {
                return None;
            }
            fixed("--", "&#8212;&#8203;", "—\u{200b}")
        }
        5 => fixed("...", "&#8230;&#8203;", "…\u{200b}"),
        6 => fixed("`'", "&#8217;", "’"),
        7 => {
            if !src[..at]
                .chars()
                .next_back()
                .is_some_and(crate::unicode_word::is_alphanumeric)
                || !rest.starts_with("'")
                || !rest[1..].chars().next().is_some_and(char::is_alphabetic)
            {
                return None;
            }
            fixed("'", "&#8217;", "’")
        }
        8 => fixed("-&gt;", "&#8594;", "→"),
        9 => fixed("=&gt;", "&#8658;", "⇒"),
        10 => fixed("&lt;-", "&#8592;", "←"),
        11 => fixed("&lt;=", "&#8656;", "⇐"),
        12 => {
            let end = crate::entity_references::entity_end(bytes, start)?;
            Some(Match {
                end,
                escape: escaped.then_some(at),
                lexical: Cow::Owned(format!("&{}", &src[start + 5..end])),
                decoded: None,
            })
        }
        _ => unreachable!(),
    }
}

fn is_word(ch: char) -> bool {
    crate::unicode_word::is_alphanumeric(ch) || crate::unicode_word::is_mark_or_connector(ch)
}

#[cfg(test)]
fn apply(src: &str) -> String {
    let mut nodes = vec![Inline::Text {
        value: src.into(),
        span: crate::Span::new(0, src.len()),
    }];
    apply_inlines(&mut nodes, false);
    nodes
        .into_iter()
        .map(|n| match n {
            Inline::Text { value, .. } | Inline::Raw { value, .. } => value,
            _ => unreachable!(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        apply(s)
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(apply("nothing to do here"), "nothing to do here");
    }

    #[test]
    fn registered_symbols() {
        assert_eq!(r("(C) (R) (TM)"), "\u{a9} \u{ae} \u{2122}");
        // adjacency
        assert_eq!(r("(C)(R)(TM)"), "\u{a9}\u{ae}\u{2122}");
    }

    #[test]
    fn spaced_em_dash_consumes_one_blank_each_side() {
        assert_eq!(r("a -- b"), "a\u{2009}\u{2014}\u{2009}b");
        // extra spaces are preserved beyond the one consumed
        assert_eq!(r("a  --  b"), "a \u{2009}\u{2014}\u{2009} b");
    }

    #[test]
    fn spaced_em_dash_at_line_boundaries() {
        assert_eq!(r("-- start"), "\u{2009}\u{2014}\u{2009}start");
        assert_eq!(r("end --"), "end\u{2009}\u{2014}\u{2009}");
    }

    #[test]
    fn word_em_dash_keeps_words_and_adds_zwsp() {
        assert_eq!(r("word--word"), "word\u{2014}\u{200b}word");
    }

    #[test]
    fn dash_without_a_full_boundary_stays_literal() {
        assert_eq!(r("a-- b"), "a-- b");
        assert_eq!(r("a --b"), "a --b");
        assert_eq!(r("three --- dashes"), "three --- dashes");
    }

    #[test]
    fn ellipsis_takes_three_dots_only() {
        assert_eq!(r("Wait..."), "Wait\u{2026}\u{200b}");
        assert_eq!(r("dots.... four"), "dots\u{2026}\u{200b}. four");
        assert_eq!(r("two.. dots"), "two.. dots");
    }

    #[test]
    fn apostrophe_only_between_word_chars() {
        assert_eq!(r("it's"), "it\u{2019}s");
        // not between two word chars -> literal
        assert_eq!(r("rock 'n' roll"), "rock 'n' roll");
        assert_eq!(r("kids' toys"), "kids' toys");
    }

    #[test]
    fn arrows() {
        assert_eq!(r("-> => <- <="), "\u{2192} \u{21d2} \u{2190} \u{21d0}");
        // no boundary needed
        assert_eq!(r("a->b"), "a\u{2192}b");
    }

    #[test]
    fn right_arrow_wins_over_left_in_ambiguous_runs() {
        assert_eq!(r("<=>"), "<\u{21d2}");
        assert_eq!(r("<->"), "<\u{2192}");
    }

    #[test]
    fn backslash_escapes_each_construct() {
        assert_eq!(r("\\(C)"), "(C)");
        assert_eq!(r("\\--"), "--");
        assert_eq!(r("\\..."), "...");
        assert_eq!(r("\\->"), "->");
        assert_eq!(r("it\\'s"), "it's");
        // a stray backslash survives
        assert_eq!(r("a\\b"), "a\\b");
    }

    #[test]
    fn replacements_compose_in_reference_order() {
        assert_eq!(
            r("(C) 2026 -- it's a test..."),
            "\u{a9} 2026\u{2009}\u{2014}\u{2009}it\u{2019}s a test\u{2026}\u{200b}"
        );
    }

    #[test]
    fn non_ascii_text_passes_through() {
        // a trigger byte forces the slow path; multibyte chars must survive it
        assert_eq!(r("café -- thé"), "café\u{2009}\u{2014}\u{2009}thé");
    }
}

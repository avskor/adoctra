//! Inline anchors are cataloged while reading, as in the pinned Ruby parser:
//! `catalog_inline_anchors` over ordinary paragraph text and
//! `catalog_inline_anchor` for an anchor leading a list item. Only cataloged
//! anchors join the reference registry; anchors elsewhere still render. Their
//! reftext runs the early a-stage with reading state; like block-anchor
//! reftext, each reference formats it with c/q/r during conversion.
use crate::context::ParseContext;
use crate::ruby_strip::RubyStrip;
use crate::unicode_word::is_word;
use crate::Span;
use std::ops::Range;

/// A cataloged anchor: its processed-source span (from `[[` or `anchor:` to
/// the closing bracket) and its early-expanded reftext.
pub(crate) struct Entry {
    pub span: Span,
    pub reftext: Option<crate::reftext::Read>,
    pub bibliography: bool,
    /// The id of a paragraph folded into a list item's text: Ruby registered
    /// it with the block (and names a block in a duplicate's warning).
    pub block: bool,
    /// Where reading cataloged it: a duplicate id is reported there.
    pub stamp: crate::context::Stamp,
}

pub(crate) struct Found {
    pub start: usize,
    pub end: usize,
    pub id: Range<usize>,
    pub reftext: Option<Range<usize>>,
    pub shorthand: bool,
}

/// Ruby's `InlineAnchorScanRx`, scanned like `String#scan`: the character
/// before `[[` (or `anchor:`) is consumed by a match, so it must not belong to
/// the previous match unless it starts a line.
fn scan(text: &str) -> Vec<Found> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut consumed = 0;
    let mut at = 0;
    while at < text.len() {
        let shorthand = text[at..].starts_with("[[");
        if !shorthand && !text[at..].starts_with("anchor:") {
            at += text[at..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        let line_start = at == 0 || bytes[at - 1] == b'\n';
        let preceded = at > consumed
            && at > 0
            && bytes[at - 1] != b'\\'
            && (!shorthand || bytes[at - 1] != b'[');
        let matched = (line_start || preceded)
            .then(|| {
                if shorthand {
                    shorthand_at(text, at)
                } else {
                    macro_at(text, at)
                }
            })
            .flatten();
        match matched {
            Some(m) => {
                at = m.end;
                consumed = m.end;
                found.push(m);
            }
            None => at += 1,
        }
    }
    found
}

/// `[#{CC_ALPHA}_:][#{CC_WORD}\-:.]*`
fn id_end(text: &str, start: usize) -> Option<usize> {
    let mut chars = text[start..].char_indices();
    match chars.next() {
        Some((_, c)) if c.is_alphabetic() || matches!(c, '_' | ':') => {}
        _ => return None,
    }
    Some(
        chars
            .find(|(_, c)| !is_word(*c) && !matches!(c, '-' | ':' | '.'))
            .map_or(text.len(), |(i, _)| start + i),
    )
}

/// Ruby's `InlineBiblioAnchorRx`. Only a triple closing delimiter ends it.
pub(crate) fn bibliography_at(text: &str, at: usize) -> Option<Found> {
    if !text[at..].starts_with("[[[") {
        return None;
    }
    let start = at + 3;
    let end = id_end(text, start)?;
    let rest = &text[end..];
    let (close, reftext) = if rest.starts_with("]]]") {
        (end, None)
    } else {
        let after = rest.strip_prefix(',')?;
        let line = &after[..after.find('\n').unwrap_or(after.len())];
        let spaces = line.len() - line.trim_start_matches(' ').len();
        let (from, close) = line[spaces..]
            .chars()
            .next()
            .and_then(|c| {
                let first = spaces + c.len_utf8();
                Some((spaces, first + line[first..].find("]]]")?))
            })
            .or_else(|| {
                (spaces > 0 && line[spaces..].starts_with("]]]")).then(|| (spaces - 1, spaces))
            })?;
        (end + 1 + close, Some(end + 1 + from..end + 1 + close))
    };
    Some(Found {
        start: at,
        end: close + 3,
        id: start..end,
        reftext,
        shorthand: true,
    })
}

/// `\[\[(id)(?:, *(.+?))?\]\]` within one line.
pub(crate) fn shorthand_at(text: &str, at: usize) -> Option<Found> {
    let id_start = at + 2;
    let id_end = id_end(text, id_start)?;
    let rest = &text[id_end..];
    if rest.starts_with("]]") {
        return Some(Found {
            start: at,
            end: id_end + 2,
            id: id_start..id_end,
            reftext: None,
            shorthand: true,
        });
    }
    let after = rest.strip_prefix(',')?;
    let line = &after[..after.find('\n').unwrap_or(after.len())];
    let spaces = line.len() - line.trim_start_matches(' ').len();
    // ` *` is greedy and `.+?` needs one character; only when no `]]` follows
    // that character does the regex take back the last skipped space.
    let (from, close) = line[spaces..]
        .chars()
        .next()
        .and_then(|c| {
            let first = spaces + c.len_utf8();
            Some((spaces, first + line[first..].find("]]")?))
        })
        .or_else(|| {
            (spaces > 0 && line[spaces..].starts_with("]]")).then(|| (spaces - 1, spaces))
        })?;
    let reftext = id_end + 1 + from..id_end + 1 + close;
    Some(Found {
        start: at,
        end: reftext.end + 2,
        id: id_start..id_end,
        reftext: Some(reftext),
        shorthand: true,
    })
}

/// `anchor:(id)\[(?:\]|(.*?[^\\])\])` within one line.
pub(crate) fn macro_at(text: &str, at: usize) -> Option<Found> {
    let id_start = at + "anchor:".len();
    let id_end = id_end(text, id_start)?;
    let rest = text[id_end..].strip_prefix('[')?;
    let open = id_end + 1;
    if rest.starts_with(']') {
        return Some(Found {
            start: at,
            end: open + 1,
            id: id_start..id_end,
            reftext: None,
            shorthand: false,
        });
    }
    let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let bytes = line.as_bytes();
    let close = (1..bytes.len()).find(|&i| bytes[i] == b']' && bytes[i - 1] != b'\\')?;
    Some(Found {
        start: at,
        end: open + close + 1,
        id: id_start..id_end,
        reftext: Some(open..open + close),
        shorthand: false,
    })
}

impl ParseContext<'_> {
    /// Bibliographic reftext deliberately skips the reading-time a-stage.
    pub(crate) fn catalog_bibliography(&mut self, text: &str, map: impl Fn(Range<usize>) -> Span) {
        let Some(found) = bibliography_at(text, 0) else {
            return;
        };
        let span = map(found.start..found.end);
        let reftext = found.reftext.map(|range| {
            let mut expanded = crate::substitution_text::Text::default();
            expanded.append_source(&format!("[{}]", &text[range]), true, 0, span);
            crate::reftext::Read::expanded(expanded, Vec::new())
        });
        let stamp = self.stamp();
        self.anchors.push((
            text[found.id].to_owned(),
            Entry {
                span,
                reftext,
                bibliography: true,
                block: false,
                stamp,
            },
        ));
    }

    /// Ruby's `catalog_inline_anchors` for paragraph text. `map` turns a range
    /// of `text` into processed-source coordinates.
    pub(crate) fn catalog_inline_anchors(
        &mut self,
        text: &str,
        map: impl Fn(Range<usize>) -> Span,
    ) {
        if !text.contains("[[") && !text.contains("or:") {
            return;
        }
        for found in scan(text) {
            let span = map(found.start..found.end);
            let id = text[found.id.clone()].to_owned();
            let Some(range) = found.reftext.clone() else {
                self.register_anchor(id, span, None);
                continue;
            };
            let base = map(range.clone()).start;
            let raw = &text[range];
            let reftext = if found.shorthand {
                // An anchor whose expanded reftext is empty is not cataloged.
                let value = self.block_reftext(raw, base);
                if raw.contains('{') && value.value.is_empty() {
                    continue;
                }
                Some(value)
            } else if raw.contains(']') {
                Some(self.block_reftext(&raw.replace("\\]", "]"), base))
            } else {
                Some(self.block_reftext(raw, base)).filter(|value| !value.value.is_empty())
            };
            self.register_anchor(id, span, reftext);
        }
    }

    /// Ruby's `catalog_inline_anchor` for `[[id, reftext]]` leading a list item.
    pub(crate) fn catalog_leading_anchor(
        &mut self,
        text: &str,
        map: impl Fn(Range<usize>) -> Span,
    ) {
        let Some(found) = text
            .starts_with("[[")
            .then(|| shorthand_at(text, 0))
            .flatten()
        else {
            return;
        };
        let reftext = found.reftext.clone().map(|range| {
            let base = map(range.clone()).start;
            self.block_reftext(&text[range], base)
        });
        let id = text[found.id].to_owned();
        self.register_anchor(id, map(found.start..found.end), reftext);
    }

    /// Ruby's term anchor: `[[id]]` without reftext takes the rest of the term.
    pub(crate) fn catalog_term_anchor(&mut self, text: &str, map: impl Fn(Range<usize>) -> Span) {
        let Some(found) = text
            .starts_with("[[")
            .then(|| shorthand_at(text, 0))
            .flatten()
        else {
            return;
        };
        let range = found.reftext.clone().unwrap_or_else(|| {
            let rest = &text[found.end..];
            found.end + (rest.len() - rest.ruby_lstrip().len())..text.len()
        });
        let reftext = (!range.is_empty()).then(|| {
            let base = map(range.clone()).start;
            self.block_reftext(&text[range], base)
        });
        let id = text[found.id].to_owned();
        self.register_anchor(id, map(found.start..found.end), reftext);
    }

    fn register_anchor(&mut self, id: String, span: Span, reftext: Option<crate::reftext::Read>) {
        let stamp = self.stamp();
        self.anchors.push((
            id,
            Entry {
                span,
                reftext,
                bibliography: false,
                block: false,
                stamp,
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(text: &str) -> Vec<(&str, Option<&str>)> {
        scan(text)
            .into_iter()
            .map(|f| (&text[f.id], f.reftext.map(|r| &text[r])))
            .collect()
    }

    #[test]
    fn scans_like_ruby_inline_anchor_scan_rx() {
        assert_eq!(ids("A [[a,R]] and [[b]]"), [("a", Some("R")), ("b", None)]);
        // Escaped, bibliography-like and adjacent anchors are not matched: the
        // preceding character was consumed by the previous match.
        assert_eq!(ids("\\[[a,R]] [[[b,S]]]"), []);
        assert_eq!(ids("[[a]][[b]]"), [("a", None)]);
        assert_eq!(ids("[[a]]\n[[b]]"), [("a", None), ("b", None)]);
        // Minimal reftext up to the first `]]`, spaces after the comma skipped,
        // and the last space kept when nothing else remains.
        assert_eq!(ids("[[a,  R]] x]]"), [("a", Some("R"))]);
        assert_eq!(ids("[[a, ]]"), [("a", Some(" "))]);
        assert_eq!(ids("[[a,]]"), []);
        // A closing bracket can be the one-character reftext.
        assert_eq!(ids("[[a,]]]"), [("a", Some("]"))]);
        assert_eq!(ids("[[a, ]]]"), [("a", Some("]"))]);
        assert_eq!(ids("[[1a]] [[Ünï:x.y-z]]"), [("Ünï:x.y-z", None)]);
        // Macro form: escaped closing brackets stay inside the reftext.
        assert_eq!(
            ids("x anchor:a[R \\] y] anchor:b[] \\anchor:c[]"),
            [("a", Some("R \\] y")), ("b", None)]
        );
        // Neither form crosses a line.
        assert_eq!(ids("[[a,R\n]]"), []);
        // Reftext boundaries are characters, including at the end of input.
        assert_eq!(ids("[[a,Ё]]"), [("a", Some("Ё"))]);
        assert_eq!(ids("[[a,"), []);
    }
}

//! Inline anchors are cataloged while reading, as in the pinned Ruby parser:
//! `catalog_inline_anchors` over ordinary paragraph text and
//! `catalog_inline_anchor` for an anchor leading a list item. Only cataloged
//! anchors join the reference registry; anchors elsewhere still render. Their
//! reftext runs the early a-stage with reading state and is formatted with
//! c/q/r like block-anchor reftext.
use crate::context::ParseContext;
use crate::unicode_word::is_word;
use crate::{Inline, Span};
use std::ops::Range;

/// A cataloged anchor: its processed-source span (from `[[` or `anchor:` to
/// the closing bracket) and formatted reftext.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub span: Span,
    pub reftext: Option<Vec<Inline>>,
}

struct Found {
    start: usize,
    end: usize,
    id: Range<usize>,
    reftext: Option<Range<usize>>,
    shorthand: bool,
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

/// `\[\[(id)(?:, *(.+?))?\]\]` within one line.
fn shorthand_at(text: &str, at: usize) -> Option<Found> {
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
    // `.+?` needs one character: it may take back the last skipped space.
    let from = if line[spaces..].starts_with("]]") {
        spaces.checked_sub(1)?
    } else {
        spaces
    };
    let first = from + line[from..].chars().next()?.len_utf8();
    let close = first + line[first..].find("]]")?;
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
fn macro_at(text: &str, at: usize) -> Option<Found> {
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

impl ParseContext {
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

    fn register_anchor(&mut self, id: String, span: Span, reftext: Option<crate::reftext::Read>) {
        let reftext = reftext.map(|value| {
            let attributes = std::sync::Arc::clone(&self.attributes);
            value.finish(&attributes, &mut self.inline_diagnostics)
        });
        self.anchors.push((id, Entry { span, reftext }));
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

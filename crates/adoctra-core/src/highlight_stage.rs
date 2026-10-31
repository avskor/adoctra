//! Ruby's `highlight` substitution (`highlight_source`): a `source` listing
//! whose custom `subs` keep `specialcharacters` is highlighted by the
//! converter in that stage's place, after `extract_callouts`; the later
//! stages run over the highlighted HTML and `restore_callouts` appends the
//! marks to their lines again.
//!
//! The HTML becomes semantic parts with Ruby's lexical spelling: tags are
//! `Raw`, the text between them `Text` with Rouge's `&amp;`/`&lt;`/`&gt;`
//! decoded, so later stages see the same string as Ruby's.

use crate::attributes::AttributePart as P;
use crate::backend::Highlighted;
use crate::substitution_context::Evaluation;
use crate::Attributes;

/// The converter's highlighter for one listing.
pub(crate) type Hook<'h> = &'h mut dyn FnMut(&str, &Attributes) -> Option<Highlighted>;

/// A callout mark taken out of the source: its 1-based line, guard, XML
/// form and number.
struct Mark {
    line: usize,
    guard: Option<String>,
    xml: bool,
    number: String,
}

/// `extract_callouts`: the source without its marks (a later mark of a
/// chain keeps the space before it; an escaped mark loses its backslash),
/// the marks by line, and a LF when the last line had a mark.
fn extract_callouts(source: &str, line_comment: Option<&str>) -> (String, Vec<Mark>) {
    let mut out = String::new();
    let mut marks = Vec::new();
    let mut autonum = 0u64;
    let lines: Vec<&str> = source.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut copied = 0;
        for mark in crate::callouts::scan_line(line, line_comment) {
            out.push_str(&line[copied..mark.start]);
            copied = mark.end;
            if mark.escaped {
                // `$&.sub RS, ''`: the first backslash of the match.
                let text = &line[mark.start..mark.end];
                out.push_str(&text.replacen('\\', "", 1));
                continue;
            }
            let number = if mark.number == "." {
                autonum += 1;
                autonum.to_string()
            } else {
                mark.number
            };
            marks.push(Mark {
                line: index + 1,
                guard: mark.guard,
                xml: mark.xml,
                number,
            });
        }
        out.push_str(&line[copied..]);
    }
    if marks.last().is_some_and(|mark| mark.line == lines.len()) {
        out.push('\n');
    }
    (out, marks)
}

/// The highlighted text as parts: tags `Raw`, text `Text` (decoded).
fn html_parts(html: &str, out: &mut Vec<P>) {
    let mut rest = html;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            let end = rest.find('>').map_or(rest.len(), |at| at + 1);
            out.push(P::Raw(rest[..end].to_owned()));
            rest = &rest[end..];
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            let text = rest[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&");
            out.push(P::Text(text));
            rest = &rest[end..];
        }
    }
}

/// `HighlightedPassSlotRx`: a passthrough slot the highlighter wrapped in
/// spans (`<span …>START</span>…N…<span …>END</span>`) becomes a slot again.
fn fix_pass_slots(html: &str, prefix: &str) -> String {
    let span_around = |text: &str, inner: &str| -> Option<usize> {
        // `<span\b[^>]*>inner</span>` at the start of `text`.
        let rest = text.strip_prefix("<span")?;
        if rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            return None;
        }
        let close = rest.find('>')?;
        let rest = rest[close + 1..].strip_prefix(inner)?;
        rest.strip_prefix("</span>")?;
        Some(text.len() - rest.len() + "</span>".len())
    };
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find("<span") {
        out.push_str(&rest[..at]);
        let candidate = &rest[at..];
        let fixed = span_around(candidate, prefix).and_then(|open_end| {
            let after = &candidate[open_end..];
            // `[^\d]*(\d+)[^\d]*` then the end span.
            let digits_at = after.find(|c: char| c.is_ascii_digit())?;
            let digits_len = after[digits_at..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(after.len() - digits_at);
            let number = &after[digits_at..digits_at + digits_len];
            let tail = &after[digits_at + digits_len..];
            // The first end span with no digit before it.
            let mut search = 0;
            let (end_at, end_len) = loop {
                let at = search + tail[search..].find("<span")?;
                if tail[..at].contains(|c: char| c.is_ascii_digit()) {
                    return None;
                }
                if let Some(len) = span_around(&tail[at..], "\u{97}") {
                    break (at, len);
                }
                search = at + 1;
            };
            let consumed = open_end + digits_at + digits_len + end_at + end_len;
            Some((format!("{prefix}{number}\u{97}"), consumed))
        });
        match fixed {
            Some((slot, consumed)) => {
                out.push_str(&slot);
                rest = &candidate[consumed..];
            }
            None => {
                out.push_str("<span");
                rest = &candidate["<span".len()..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// How the listing's substitutions frame the stage.
pub(crate) struct Listing<'a> {
    /// Ruby's `subs.include? :callouts`.
    pub process_callouts: bool,
    pub line_comment: Option<&'a str>,
    /// The passthrough slot prefix (Ruby's `PASS_START`).
    pub slot_prefix: &'a str,
}

/// The stage: None when the converter does not highlight (Ruby's
/// `sub_source`: the caller substitutes special characters instead).
pub(crate) fn apply(
    parts: &[P],
    hook: Hook<'_>,
    listing: Listing<'_>,
    attrs: &Evaluation<'_>,
    span: crate::Span,
    diagnostics: &mut Vec<crate::Diagnostic>,
) -> Option<Vec<P>> {
    let Listing {
        process_callouts,
        line_comment,
        slot_prefix,
    } = listing;
    let text = crate::attributes::reference_text(parts);
    let (source, marks) = if process_callouts {
        extract_callouts(&text, line_comment)
    } else {
        (text, Vec::new())
    };
    let Highlighted {
        html,
        source_offset,
    } = hook(&source, attrs)?;
    let html = if html.contains(slot_prefix) {
        fix_pass_slots(&html, slot_prefix)
    } else {
        html
    };
    let mut out = Vec::new();
    if marks.is_empty() {
        html_parts(&html, &mut out);
        return Some(out);
    }
    // `restore_callouts`.
    let offset = source_offset.unwrap_or(0).min(html.len());
    let (preamble, code) = html.split_at(offset);
    html_parts(preamble, &mut out);
    for (index, line) in code.split('\n').enumerate() {
        if index > 0 {
            out.push(P::Text("\n".into()));
        }
        html_parts(line, &mut out);
        let mut first = true;
        for mark in marks.iter().filter(|mark| mark.line == index + 1) {
            if !first {
                out.push(P::Text(" ".into()));
            }
            first = false;
            let (icons, target) = crate::callouts::icon(attrs, &mark.number);
            let src = target.map_or_else(String::new, |target| {
                attrs.image_uri(&target, attrs.get("iconsdir"), span, diagnostics)
            });
            out.push(P::Callout(Box::new(crate::InlineCallout {
                number: mark.number.clone(),
                guard: mark.guard.clone(),
                xml: mark.xml,
                id: None,
                icons,
                src,
            })));
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_callouts_like_ruby() {
        let (source, marks) = extract_callouts("a // <1> <2>\nb \\<3>\nc <.>", None);
        // The chain's second mark keeps its space; the escape loses its
        // backslash; a mark on the last line adds a LF.
        assert_eq!(source, "a  \nb <3>\nc \n");
        let numbers: Vec<_> = marks.iter().map(|m| (m.line, m.number.as_str())).collect();
        assert_eq!(numbers, [(1, "1"), (1, "2"), (3, "1")]);
        assert_eq!(marks[0].guard.as_deref(), Some("// "));
    }

    #[test]
    fn splits_html_into_raw_tags_and_decoded_text() {
        let mut parts = Vec::new();
        html_parts("<span class=\"o\">&lt;&amp;</span>x", &mut parts);
        assert_eq!(
            parts,
            [
                P::Raw("<span class=\"o\">".into()),
                P::Text("<&".into()),
                P::Raw("</span>".into()),
                P::Text("x".into()),
            ]
        );
    }

    #[test]
    fn fixes_highlighted_pass_slots() {
        let html = "a<span class=\"err\">\u{96}</span>12<span class=\"err\">\u{97}</span>b";
        assert_eq!(fix_pass_slots(html, "\u{96}"), "a\u{96}12\u{97}b");
        assert_eq!(fix_pass_slots("<span>x</span>", "\u{96}"), "<span>x</span>");
    }
}

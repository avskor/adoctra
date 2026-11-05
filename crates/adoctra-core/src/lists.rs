//! List grammar ported from Ruby's `Parser`: the unordered, ordered and
//! description list item expressions (`UnorderedListRx`, `OrderedListRx`,
//! `DescriptionListRx` and its siblings, `AnyListRx`), ordered marker
//! resolution and sibling detection.

/// A list context (Ruby's `:ulist`, `:olist`, `:dlist`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ListType {
    Unordered,
    Ordered,
    Description,
    Callout,
}

/// A list item line: the raw marker (or description delimiter) and byte
/// ranges of the marker and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ItemMatch {
    pub marker: String,
    /// The marker (or delimiter) range.
    pub marker_range: std::ops::Range<usize>,
    /// The item text after the marker (the description text for a term),
    /// absent for a term without inline description.
    pub text: Option<std::ops::Range<usize>>,
    /// The term of a description list item.
    pub term: Option<std::ops::Range<usize>>,
}

fn blank(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

fn indent(line: &str) -> usize {
    line.bytes().take_while(|&b| blank(b)).count()
}

/// The text after a marker ending at `end`: `[ \t]+(.*)$` with at least one
/// character of text (lines are right-stripped).
fn text_after(line: &str, end: usize) -> Option<std::ops::Range<usize>> {
    let ws = line[end..].bytes().take_while(|&b| blank(b)).count();
    (ws > 0 && end + ws < line.len()).then_some(end + ws..line.len())
}

/// Ruby's `UnorderedListRx`: `^[ \t]*(-|\*\**|•)[ \t]+(.*)$`.
pub(crate) fn unordered(line: &str) -> Option<ItemMatch> {
    let start = indent(line);
    let rest = &line[start..];
    let len = if rest.starts_with('-') {
        1
    } else if rest.starts_with('*') {
        rest.bytes().take_while(|&b| b == b'*').count()
    } else if rest.starts_with('\u{2022}') {
        '\u{2022}'.len_utf8()
    } else {
        return None;
    };
    let text = text_after(line, start + len)?;
    Some(ItemMatch {
        marker: rest[..len].to_owned(),
        marker_range: start..start + len,
        text: Some(text),
        term: None,
    })
}

/// Ruby's `OrderedListRx`:
/// `^[ \t]*(\.\.*|\d+\.|[a-zA-Z]\.|[IVXivx]+\))[ \t]+(.*)$`.
pub(crate) fn ordered(line: &str) -> Option<ItemMatch> {
    let start = indent(line);
    let rest = &line.as_bytes()[start..];
    let len = match rest.first()? {
        b'.' => rest.iter().take_while(|&&b| b == b'.').count(),
        b'0'..=b'9' => {
            let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
            (rest.get(digits) == Some(&b'.')).then_some(digits + 1)?
        }
        c if c.is_ascii_alphabetic() && rest.get(1) == Some(&b'.') => 2,
        _ => {
            let roman = rest
                .iter()
                .take_while(|b| matches!(b, b'I' | b'V' | b'X' | b'i' | b'v' | b'x'))
                .count();
            (roman > 0 && rest.get(roman) == Some(&b')')).then_some(roman + 1)?
        }
    };
    // A letter followed by `.` may also be a roman numeral with `)`; the
    // alternatives are tried in Ruby's order, all ending at their own marker.
    let text = text_after(line, start + len)?;
    Some(ItemMatch {
        marker: line[start..start + len].to_owned(),
        marker_range: start..start + len,
        text: Some(text),
        term: None,
    })
}

/// Ruby's `DescriptionListRx`:
/// `^(?!//[^/])[ \t]*([^ \t].*?)(:::{0,2}|;;)(?:$|[ \t]+(.*)$)`.
pub(crate) fn description(line: &str) -> Option<ItemMatch> {
    description_with(line, None)
}

/// Ruby's `DescriptionListSiblingRx` for `delimiter` (a colon delimiter
/// requires a term not ending in `:`).
pub(crate) fn description_sibling(line: &str, delimiter: &str) -> Option<ItemMatch> {
    description_with(line, Some(delimiter))
}

fn description_with(line: &str, sibling: Option<&str>) -> Option<ItemMatch> {
    if line.starts_with("//") && !line[2..].starts_with('/') && line.len() > 2 {
        return None;
    }
    let start = indent(line);
    let bytes = line.as_bytes();
    if start >= line.len() {
        return None;
    }
    let first = line[start..].chars().next()?.len_utf8();
    let mut at = start + first;
    while at < line.len() {
        // Delimiters start with `:` or `;`.
        at = crate::candidates::next(bytes, at, |b| matches!(b, b':' | b';'));
        if at == line.len() {
            break;
        }
        if !line.is_char_boundary(at) {
            at += 1;
            continue;
        }
        let candidates: Vec<usize> = if line[at..].starts_with(";;") {
            vec![2]
        } else if line[at..].starts_with("::") {
            let run = bytes[at..].iter().take_while(|&&b| b == b':').count();
            (2..=run.min(4)).rev().collect()
        } else {
            Vec::new()
        };
        for len in candidates {
            let delimiter = &line[at..at + len];
            if let Some(wanted) = sibling {
                if delimiter != wanted || (wanted != ";;" && bytes[at - 1] == b':') {
                    continue;
                }
            }
            let end = at + len;
            let text = if end == line.len() {
                None
            } else if blank(bytes[end]) {
                Some(text_after(line, end).unwrap_or(line.len()..line.len()))
            } else {
                continue;
            };
            return Some(ItemMatch {
                marker: delimiter.to_owned(),
                marker_range: at..end,
                text: text.filter(|r| !r.is_empty()),
                term: Some(start..at),
            });
        }
        at += 1;
    }
    None
}

/// Ruby's `AnyListRx`: the start of any list item (including callouts).
pub(crate) fn any(line: &str) -> bool {
    if unordered(line).is_some() || ordered(line).is_some() || description(line).is_some() {
        return true;
    }
    // `<(?:\d+|\.)>[ \t]` callout list items.
    let rest = line.as_bytes();
    if rest.first() == Some(&b'<') {
        let digits = rest[1..].iter().take_while(|b| b.is_ascii_digit()).count();
        let len = if digits > 0 {
            digits
        } else {
            usize::from(rest.get(1) == Some(&b'.'))
        };
        return len > 0
            && rest.get(1 + len) == Some(&b'>')
            && rest.get(2 + len).is_some_and(|&b| blank(b));
    }
    false
}

/// The item expression of a list type.
pub(crate) fn item(line: &str, list_type: ListType) -> Option<ItemMatch> {
    match list_type {
        ListType::Unordered => unordered(line),
        ListType::Ordered => ordered(line),
        ListType::Description => description(line),
        ListType::Callout => crate::callouts::list_item(line),
    }
}

/// Ruby's `ORDERED_LIST_STYLES` and the marker of their first item.
pub(crate) fn ordered_style(marker: &str) -> Option<(&'static str, &'static str)> {
    let bytes = marker.as_bytes();
    let (body, last) = bytes.split_at(bytes.len().checked_sub(1)?);
    Some(match last {
        [b'.'] if !body.is_empty() && body.iter().all(u8::is_ascii_digit) => ("arabic", "1."),
        [b'.'] if body.len() == 1 && body[0].is_ascii_lowercase() => ("loweralpha", "a."),
        // Ruby's unanchored `[ivx]+\)` / `[IVX]+\)`: the numeral's last letter.
        [b')'] if matches!(body.last(), Some(b'i' | b'v' | b'x')) => ("lowerroman", "i)"),
        [b'.'] if body.len() == 1 && body[0].is_ascii_uppercase() => ("upperalpha", "A."),
        [b')'] if matches!(body.last(), Some(b'I' | b'V' | b'X')) => ("upperroman", "I)"),
        _ => return None,
    })
}

/// Ruby's `resolve_ordered_list_marker`: the first marker of the series (a
/// `.` run is its own) and the implicit style.
pub(crate) fn resolve_ordered(marker: &str) -> (String, Option<&'static str>) {
    if marker.starts_with('.') {
        return (marker.to_owned(), None);
    }
    match ordered_style(marker) {
        Some((style, first)) => (first.to_owned(), Some(style)),
        None => (marker.to_owned(), None),
    }
}

/// The marker an item at `ordinal` should have, for Ruby's validation
/// warning (`list item index: expected …, got …`).
pub(crate) fn expected_ordinal(marker: &str, ordinal: usize) -> Option<(String, String)> {
    let (style, _) = ordered_style(marker)?;
    let actual = &marker[..marker.len() - 1];
    let expected = match style {
        "arabic" => {
            return Some((
                (ordinal + 1).to_string(),
                crate::table::to_i(actual).to_string(),
            ))
        }
        "loweralpha" => char::from(b'a'.wrapping_add(ordinal as u8)).to_string(),
        "upperalpha" => char::from(b'A'.wrapping_add(ordinal as u8)).to_string(),
        "lowerroman" => roman(ordinal + 1).to_lowercase(),
        _ => roman(ordinal + 1),
    };
    Some((expected, actual.to_owned()))
}

/// Ruby's `Helpers.int_to_roman`.
pub(crate) fn roman(mut value: usize) -> String {
    let mut out = String::new();
    for (n, s) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= n {
            out.push_str(s);
            value -= n;
        }
    }
    out
}

/// What makes a line a sibling of the current item: the resolved marker of
/// an ordered or unordered list, or a description delimiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sibling {
    Marker(ListType, String),
    Delimiter(String),
}

/// Ruby's `is_sibling_list_item?`.
pub(crate) fn is_sibling(line: &str, sibling: &Sibling) -> bool {
    match sibling {
        Sibling::Delimiter(delimiter) => description_sibling(line, delimiter).is_some(),
        Sibling::Marker(ListType::Unordered, marker) => {
            unordered(line).is_some_and(|m| m.marker == *marker)
        }
        Sibling::Marker(ListType::Ordered, marker) => {
            ordered(line).is_some_and(|m| resolve_ordered(&m.marker).0 == *marker)
        }
        Sibling::Marker(ListType::Callout, _) => crate::callouts::list_item(line).is_some(),
        Sibling::Marker(ListType::Description, _) => false,
    }
}

/// Ruby's `is_delimited_block?`: the closing line of a delimited block.
pub(crate) fn delimited(line: &str) -> Option<String> {
    delimiter(line).map(|d| d.terminator)
}

/// Ruby's `BlockMatchData`: the `DELIMITED_BLOCKS` key of a delimiter line
/// and the line that closes the block (the opening line itself, except for
/// fenced code).
pub(crate) struct Delimiter {
    pub tip: &'static str,
    pub terminator: String,
}

/// Ruby's `is_delimited_block?` with match data.
pub(crate) fn delimiter(line: &str) -> Option<Delimiter> {
    const HEADS: [&str; 12] = [
        "--", "..", "==", "**", "__", "++", "|=", ",=", ":=", "!=", "//", "``",
    ];
    // Every head is two ASCII characters, so a matching line has at least two
    // characters; most lines stop here without collecting their characters.
    if !HEADS.iter().any(|h| line.starts_with(h)) {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if len == 2 {
        return (line == "--").then(|| Delimiter {
            tip: "--",
            terminator: line.to_owned(),
        });
    }
    let mut tip: String = chars.iter().take(4).collect();
    let mut tip_len = len.min(4);
    let mut line_len = len;
    let mut terminator = line.to_owned();
    if tip.starts_with('`') {
        if tip_len == 4 {
            // `tip` holds four characters, not bytes: compare by prefix.
            if tip == "````" || !tip.starts_with("```") {
                return None;
            }
            tip.truncate(3);
            terminator = tip.clone();
            line_len = 3;
            tip_len = 3;
        } else if tip != "```" {
            return None;
        }
    } else if tip_len == 3 {
        return None;
    }
    const KEYS: [&str; 13] = [
        "--", "----", "....", "====", "****", "____", "++++", "|===", ",===", ":===", "!===",
        "////", "```",
    ];
    let tip = KEYS.into_iter().find(|key| *key == tip)?;
    let tail = tip.chars().last().unwrap();
    (line_len == tip_len || (tip_len == 4 && chars[1..].iter().all(|&c| c == tail)))
        .then_some(Delimiter { tip, terminator })
}

/// Ruby's `BlockAttributeLineRx`: an attribute list or block anchor line.
pub(crate) fn block_attribute_line(line: &str) -> bool {
    crate::block_attributes::attr_list_inner(line).is_some()
        || crate::parser::parse_block_anchor(line).is_some()
}

/// Ruby's `LiteralParagraphRx`: a line starting with a space or tab.
pub(crate) fn literal_paragraph(line: &str) -> bool {
    line.starts_with([' ', '\t'])
}

/// Ruby's `AttributeEntryRx` shape: `^:(!?\w[^:]*):(?:[ \t]+(.*))?$`.
pub(crate) fn attribute_entry(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(':') else {
        return false;
    };
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    let Some(first) = rest.chars().next() else {
        return false;
    };
    if !crate::unicode_word::is_word(first) {
        return false;
    }
    let Some(close) = rest.find(':') else {
        return false;
    };
    let after = &rest[close + 1..];
    after.is_empty() || after.starts_with([' ', '\t'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delimiter_lines_need_a_two_character_head() {
        for line in [
            "",
            "-",
            "Ж",
            "Ж-",
            "-Ж",
            "a--",
            " ----",
            "\u{2014}\u{2014}",
            "---",
            "````",
        ] {
            assert_eq!(delimited(line), None, "{line:?}");
        }
        for (line, terminator) in [
            ("--", "--"),
            ("----", "----"),
            ("====", "===="),
            ("|===", "|==="),
            ("```", "```"),
            ("```ruby", "```"),
        ] {
            assert_eq!(delimited(line).as_deref(), Some(terminator), "{line:?}");
        }
    }

    #[test]
    fn description_delimiter_skipping_matches_a_scan_of_every_offset() {
        use crate::candidates::tests::{every_offset, inputs};
        let tokens = ["a", ":", "::", ";", ";;", " ", "\t", "\u{416}", "//", "/"];
        for input in inputs(&tokens, 5) {
            for sibling in [None, Some("::"), Some(":::"), Some("::::"), Some(";;")] {
                assert_eq!(
                    description_with(&input, sibling),
                    every_offset(|| description_with(&input, sibling)),
                    "{input:?} {sibling:?}"
                );
            }
        }
    }

    #[test]
    fn markers_follow_ruby_expressions() {
        assert_eq!(unordered("  ** item").unwrap().marker, "**");
        assert_eq!(unordered("- x").unwrap().text, Some(2..3));
        assert!(unordered("*bold*").is_none());
        assert!(unordered("* ").is_none());
        for (line, marker) in [
            (". a", "."),
            ("10. a", "10."),
            ("b. a", "b."),
            ("iv) a", "iv)"),
            ("C. a", "C."),
        ] {
            assert_eq!(ordered(line).unwrap().marker, marker, "{line}");
        }
        assert!(ordered("1.5 x").is_none());
        assert!(ordered("ab. x").is_none());
    }

    #[test]
    fn description_delimiters_follow_ruby_expressions() {
        let m = description("CPU:: The brain").unwrap();
        assert_eq!(
            (m.marker.as_str(), m.term, m.text),
            ("::", Some(0..3), Some(6..15))
        );
        let m = description("a::::: b").unwrap();
        assert_eq!((m.marker.as_str(), m.term), ("::::", Some(0..2)));
        let m = description("term;;").unwrap();
        assert_eq!((m.marker.as_str(), m.text), (";;", None));
        assert!(description("// x:: y").is_none());
        assert!(description("http://x").is_none());
        assert!(description(":: x").is_none());
        assert!(description_sibling("a::: b", "::").is_none());
        assert!(description_sibling("a:: b", "::").is_some());
        assert!(description_sibling("a: b:: c", "::").is_some());
    }

    #[test]
    fn ordered_markers_resolve_and_validate() {
        assert_eq!(resolve_ordered("3."), ("1.".into(), Some("arabic")));
        assert_eq!(resolve_ordered("c."), ("a.".into(), Some("loweralpha")));
        assert_eq!(resolve_ordered("iv)"), ("i)".into(), Some("lowerroman")));
        assert_eq!(resolve_ordered(".."), ("..".into(), None));
        assert_eq!(expected_ordinal("3.", 1), Some(("2".into(), "3".into())));
        assert_eq!(expected_ordinal("iv)", 3), Some(("iv".into(), "iv".into())));
        assert_eq!(roman(1994), "MCMXCIV");
    }

    #[test]
    fn any_list_and_attribute_entries() {
        assert!(any("<1> callout"));
        assert!(any("<.> auto"));
        assert!(!any("<x> no"));
        assert!(attribute_entry(":name: value"));
        assert!(attribute_entry(":!name:"));
        assert!(!attribute_entry(":: x"));
    }
}

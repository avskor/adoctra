//! Ruby's `Parser.adjust_indentation!` for verbatim content (listing, literal
//! and verse blocks, indented literal paragraphs): tab expansion, then removal
//! of the common block indent and an optional new indent. It transforms a
//! [`FilteredSource`], so converted inlines keep their physical spans;
//! generated spaces pin to the tab or the line start they stand for.

use crate::attributes::Attributes;
use crate::scanner::{FilteredSource, Piece};
use std::collections::BTreeMap;

/// Ruby has no bound; a larger value is clamped rather than allocating
/// arbitrarily long lines.
const MAX_WIDTH: usize = 1 << 12;

/// How a verbatim block adjusts its lines.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Indentation {
    /// The `indent` attribute: `None` keeps the indentation, a negative value
    /// only expands tabs, otherwise the block indent becomes this many spaces.
    pub indent: Option<i64>,
    /// `tabsize`: the tab stop width, `0` keeps tabs.
    pub tab_size: usize,
}

impl Indentation {
    /// An indented literal paragraph: the common indent is removed.
    pub fn strip() -> Self {
        Self {
            indent: Some(0),
            tab_size: 0,
        }
    }

    /// Ruby's `build_block` for verbatim content: the block's `indent` and its
    /// (else the document's) `tabsize`.
    pub fn of_block(named: &BTreeMap<String, String>, document: &Attributes) -> Self {
        let tab_size = named
            .get("tabsize")
            .map(String::as_str)
            .or_else(|| document.get("tabsize"))
            .map_or(0, to_i);
        Self {
            indent: named.get("indent").map(|value| to_i(value)),
            tab_size: usize::try_from(tab_size).unwrap_or(0).min(MAX_WIDTH),
        }
    }

    pub fn is_noop(self) -> bool {
        self.indent.is_none() && self.tab_size == 0
    }

    /// Adjust a plain text (Ruby's `adjust_indentation!` on its lines).
    pub fn apply_text(self, text: &str) -> String {
        if self.is_noop() {
            return text.to_owned();
        }
        self.apply(&FilteredSource::from_fragments([(0, text)]))
            .text
    }

    /// Adjust the lines of `source`, keeping the map to physical bytes.
    pub fn apply(self, source: &FilteredSource) -> FilteredSource {
        if self.is_noop() {
            return source.clone();
        }
        let text = source.text.as_str();
        // Each line's pieces, then its line ending (kept as is).
        let mut lines: Vec<(Vec<Run>, std::ops::Range<usize>)> = Vec::new();
        let mut start = 0;
        while start <= text.len() {
            let newline = text[start..].find('\n').map(|i| start + i);
            let eol_end = newline.map_or(text.len(), |i| i + 1);
            let mut end = newline.unwrap_or(text.len());
            if text[start..end].ends_with('\r') {
                end -= 1;
            }
            let runs = if start == end {
                Vec::new()
            } else {
                vec![Run::Copy(start..end)]
            };
            lines.push((runs, end..eol_end));
            if newline.is_none() {
                break;
            }
            start = eol_end;
        }

        if self.tab_size > 0 && text.contains('\t') {
            for (runs, _) in &mut lines {
                *runs = expand_tabs(text, runs, self.tab_size);
            }
        }

        if let Some(indent) = self.indent.filter(|&indent| indent >= 0) {
            // The common indent of the non-empty lines; any unindented line
            // disables its removal.
            let mut block_indent: Option<usize> = None;
            for (runs, _) in lines.iter().filter(|(runs, _)| !runs.is_empty()) {
                let width = leading_whitespace(text, runs);
                if width == 0 {
                    block_indent = None;
                    break;
                }
                block_indent = Some(block_indent.map_or(width, |b| b.min(width)));
            }
            let padding = " ".repeat(usize::try_from(indent).unwrap_or(0).min(MAX_WIDTH));
            for (runs, _) in lines.iter_mut().filter(|(runs, _)| !runs.is_empty()) {
                if let Some(width) = block_indent {
                    remove_leading(runs, width);
                }
                if !padding.is_empty() {
                    let at = runs.first().map_or(0, Run::source_start);
                    runs.insert(0, Run::Generated(padding.clone(), at, 0));
                }
            }
        }

        let mut pieces = Vec::new();
        for (runs, eol) in lines {
            for run in runs {
                pieces.push(match run {
                    Run::Copy(range) => Piece::Copy(range),
                    Run::Generated(text, at, width) => Piece::Generated { text, at, width },
                });
            }
            pieces.push(Piece::Copy(eol));
        }
        source.remap(&pieces)
    }
}

/// A run of an adjusted line: source text, or spaces standing for the source
/// range `at..at + width`.
#[derive(Clone)]
enum Run {
    Copy(std::ops::Range<usize>),
    Generated(String, usize, usize),
}

impl Run {
    fn source_start(&self) -> usize {
        match self {
            Run::Copy(range) => range.start,
            Run::Generated(_, at, _) => *at,
        }
    }
}

/// Replace every tab with spaces up to the next tab stop, counting columns in
/// characters like Ruby.
fn expand_tabs(text: &str, runs: &[Run], tab_size: usize) -> Vec<Run> {
    let mut out = Vec::new();
    let mut column = 0;
    for run in runs {
        match run {
            Run::Copy(range) => {
                let mut copied = range.start;
                for (i, c) in text[range.clone()].char_indices() {
                    let at = range.start + i;
                    if c == '\t' {
                        if at > copied {
                            out.push(Run::Copy(copied..at));
                        }
                        let spaces = tab_size - column % tab_size;
                        out.push(Run::Generated(" ".repeat(spaces), at, 1));
                        column += spaces;
                        copied = at + 1;
                    } else {
                        column += 1;
                    }
                }
                if range.end > copied {
                    out.push(Run::Copy(copied..range.end));
                }
            }
            Run::Generated(spaces, ..) => {
                column += spaces.chars().count();
                out.push(run.clone());
            }
        }
    }
    out
}

/// Ruby's `String#lstrip` whitespace.
fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\0')
}

/// The number of leading whitespace characters (all ASCII, so also bytes).
fn leading_whitespace(text: &str, runs: &[Run]) -> usize {
    let mut width = 0;
    for run in runs {
        let (chunk, len) = match run {
            Run::Copy(range) => (&text[range.clone()], range.len()),
            Run::Generated(spaces, ..) => (spaces.as_str(), spaces.len()),
        };
        let leading = chunk.len() - chunk.trim_start_matches(is_whitespace).len();
        width += leading;
        if leading < len {
            break;
        }
    }
    width
}

/// Drop the first `width` bytes of a line.
fn remove_leading(runs: &mut Vec<Run>, mut width: usize) {
    while width > 0 && !runs.is_empty() {
        match &mut runs[0] {
            Run::Copy(range) => {
                let take = width.min(range.len());
                range.start += take;
                width -= take;
                if range.start == range.end {
                    runs.remove(0);
                }
            }
            Run::Generated(spaces, ..) => {
                let take = width.min(spaces.len());
                spaces.drain(..take);
                width -= take;
                if spaces.is_empty() {
                    runs.remove(0);
                }
            }
        }
    }
}

/// Ruby's `String#to_i`: optional leading whitespace and sign, then digits
/// (single underscores may separate them); anything else gives `0`.
pub(crate) fn to_i(value: &str) -> i64 {
    let value = value.trim_start_matches(is_whitespace);
    let (negative, digits) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let mut number: i64 = 0;
    let mut previous_digit = false;
    let bytes = digits.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'0'..=b'9' => {
                number = number
                    .saturating_mul(10)
                    .saturating_add(i64::from(b - b'0'));
                previous_digit = true;
            }
            b'_' if previous_digit && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                previous_digit = false;
            }
            _ => break,
        }
    }
    if negative {
        -number
    } else {
        number
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adjust(text: &str, indent: Option<i64>, tab_size: usize) -> String {
        Indentation { indent, tab_size }.apply_text(text)
    }

    #[test]
    fn expands_tabs_to_character_tab_stops() {
        assert_eq!(adjust("\tЯ\tX\n  \tab", None, 4), "    Я   X\n    ab");
        assert_eq!(adjust("\t\tx", Some(-1), 2), "    x");
        assert_eq!(adjust("a\tb", None, 0), "a\tb");
    }

    #[test]
    fn removes_the_common_indent_and_adds_a_new_one() {
        assert_eq!(adjust("   *x*\n     y", Some(1), 0), " *x*\n   y");
        assert_eq!(adjust("\t a\n\n    b", Some(2), 4), "   a\n\n  b");
        assert_eq!(adjust("x\n  y", Some(2), 0), "  x\n    y");
        assert_eq!(adjust("  x\n y", Some(0), 0), " x\ny");
        // Without tab expansion a tab is one indent character.
        assert_eq!(adjust("\tx\n\t\ty", Some(0), 0), "x\n\ty");
        assert_eq!(adjust("a\r\n  b\r\n", Some(0), 0), "a\r\n  b\r\n");
    }

    #[test]
    fn keeps_spans_of_copied_text_and_pins_generated_spaces() {
        let source = FilteredSource::from_fragments([(10, "\tab\n  cd")]);
        let adjusted = Indentation {
            indent: Some(1),
            tab_size: 4,
        }
        .apply(&source);
        assert_eq!(adjusted.text, "   ab\n cd");
        let span = |s, e| adjusted.original_span(crate::Span::new(s, e));
        // `ab` and `cd` keep their physical bytes.
        assert_eq!(span(3, 5), crate::Span::new(11, 13));
        assert_eq!(span(7, 9), crate::Span::new(16, 18));
        // The new indent is zero-width before the line's retained text; the
        // rest of an expanded tab stands for the tab byte.
        assert_eq!(span(0, 1), crate::Span::new(10, 10));
        assert_eq!(span(1, 3), crate::Span::new(10, 11));
        assert_eq!(span(0, 5), crate::Span::new(10, 13));
        assert_eq!(span(6, 7), crate::Span::new(16, 16));
    }

    #[test]
    fn parses_integers_like_ruby() {
        assert_eq!(to_i("4"), 4);
        assert_eq!(to_i(" -3x"), -3);
        assert_eq!(to_i("1_0"), 10);
        assert_eq!(to_i("abc"), 0);
        assert_eq!(to_i(""), 0);
        assert_eq!(to_i("_1"), 0);
    }
}

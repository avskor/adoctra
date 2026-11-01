//! The pinned Ruby AttributeList grammar, shared by block attribute lines,
//! include parameters and link macro labels. The lexer only reports field
//! boundaries in its input; callers decide which substitutions a field gets and
//! map ranges back to their own source representation.
use crate::ruby_strip::RubyStrip;
use crate::unicode_word::is_word;
use std::ops::Range;

/// One parsed field. `index` is Ruby's zero-based field index: positional
/// values are stored under `index + 1`, so named and skipped fields still
/// occupy a slot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Field {
    pub index: usize,
    /// The attribute name of a `name=value` field.
    pub name: Option<Range<usize>>,
    /// The value, excluding a closing quote pair. `None` is Ruby's nil
    /// positional (an empty unquoted field).
    pub value: Option<Range<usize>>,
    /// The quote that delimited a closed value; its escaped form is removed.
    pub quote: Option<char>,
    /// Ruby's `single_quoted`: a closed single-quoted value that does not
    /// itself begin with an apostrophe requests `sub_result` substitutions.
    pub single_quoted: bool,
    /// Blanks between a leading name and the rest of an unquoted value; Ruby
    /// rebuilds them as the same number of spaces.
    pub gap: Option<Range<usize>>,
}

impl Field {
    /// The value as Ruby stores it: escaped delimiter quotes removed and a
    /// name gap normalized to spaces.
    pub fn text(&self, src: &str) -> Option<String> {
        let range = self.value.clone()?;
        let mut out = String::with_capacity(range.len());
        let mut chars = src[range.clone()].char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let at = range.start + i;
            if self.gap.as_ref().is_some_and(|gap| gap.contains(&at)) {
                out.push(' ');
            } else if c == '\\' && self.quote.is_some() && chars.peek().map(|p| p.1) == self.quote {
                out.push(chars.next().unwrap().1);
            } else {
                out.push(c);
            }
        }
        Some(out)
    }

    pub fn name<'a>(&self, src: &'a str) -> Option<&'a str> {
        self.name.clone().map(|range| &src[range])
    }
}

/// A named field whose unquoted value is `None` is skipped, but still
/// consumes a field index.
pub(crate) fn parse(src: &str) -> Vec<Field> {
    let mut scanner = Scanner { src, at: 0 };
    let mut fields = Vec::new();
    let mut index = 0;
    while let Some((field, more)) = scanner.attribute(index) {
        fields.extend(field);
        if !more || scanner.at >= src.len() {
            break;
        }
        scanner.skip_delimiter();
        index += 1;
    }
    fields
}

struct Scanner<'a> {
    src: &'a str,
    at: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<char> {
        self.src[self.at..].chars().next()
    }

    fn skip_blank(&mut self) -> usize {
        let start = self.at;
        while matches!(self.peek(), Some(' ' | '\t')) {
            self.at += 1;
        }
        self.at - start
    }

    /// `[ \t]*(,|$)`: blanks are consumed only together with a delimiter.
    fn skip_delimiter(&mut self) {
        let start = self.at;
        self.skip_blank();
        match self.peek() {
            Some(',') => self.at += 1,
            None | Some('\n') => {}
            _ => self.at = start,
        }
    }

    fn scan_name(&mut self) -> Option<Range<usize>> {
        let start = self.at;
        let mut chars = self.src[start..].char_indices();
        match chars.next() {
            Some((_, c)) if is_word(c) => {}
            _ => return None,
        }
        let end = chars
            .find(|(_, c)| !is_word(*c) && !matches!(c, '-' | '.'))
            .map_or(self.src.len(), |(i, _)| start + i);
        self.at = end;
        Some(start..end)
    }

    /// `.*?(?=[ \t]*(,|$))`
    fn scan_to_delimiter(&mut self) -> usize {
        let rest = &self.src[self.at..];
        let stop = rest.find([',', '\n']).unwrap_or(rest.len());
        self.at += rest[..stop].trim_end_matches([' ', '\t']).len();
        self.at
    }

    /// The opening quote has been consumed. Returns the value range and
    /// whether the quote closed.
    fn value(&mut self, quote: char) -> (Range<usize>, bool) {
        let start = self.at;
        if self.peek() == Some(quote) {
            self.at += 1;
            return (start..start, true);
        }
        // `.*?[^\\](?=quote)` on a single line.
        let bytes = self.src.as_bytes();
        let mut i = start;
        while i < bytes.len() && bytes[i] != b'\n' {
            if bytes[i] == quote as u8 && i > start && bytes[i - 1] != b'\\' {
                self.at = i + 1;
                return (start..i, true);
            }
            i += 1;
        }
        // A leading quote only: the quote is literal and commas delimit.
        let end = self.scan_to_delimiter();
        (start - 1..end, false)
    }

    fn quoted(&mut self, index: usize, name: Option<Range<usize>>) -> Field {
        let quote = self.peek().unwrap();
        self.at += 1;
        let (value, closed) = self.value(quote);
        Field {
            index,
            name,
            // Ruby tests the value after removing escaped quotes, so a
            // leading `\'` is an apostrophe too.
            single_quoted: closed && quote == '\'' && {
                let raw = &self.src[value.clone()];
                !(raw.starts_with('\'') || raw.starts_with("\\'"))
            },
            quote: closed.then_some(quote),
            value: Some(value),
            gap: None,
        }
    }

    /// Ruby's `parse_attribute`. `None` ends parsing without a field; the flag
    /// is Ruby's `continue`.
    fn attribute(&mut self, index: usize) -> Option<(Option<Field>, bool)> {
        self.skip_blank();
        if matches!(self.peek(), Some('"' | '\'')) {
            return Some((Some(self.quoted(index, None)), true));
        }
        let name = self.scan_name();
        let skipped = if name.is_some() { self.skip_blank() } else { 0 };
        let positional = |value| Field {
            index,
            name: None,
            value,
            quote: None,
            single_quoted: false,
            gap: None,
        };
        let Some(c) = self.peek().filter(|c| *c != '\n') else {
            if name.is_none() && !self.src.ruby_rstrip().ends_with(',') {
                return None;
            }
            return Some((Some(positional(name)), false));
        };
        if c == ',' {
            return Some((Some(positional(name)), true));
        }
        let Some(name) = name else {
            let start = self.at;
            self.at += c.len_utf8();
            let end = self.scan_to_delimiter();
            return Some((Some(positional(Some(start..end))), true));
        };
        if c != '=' {
            let gap = name.end..name.end + skipped;
            let end = self.scan_to_delimiter();
            let mut field = positional(Some(name.start..end));
            field.gap = (skipped > 0).then_some(gap);
            return Some((Some(field), true));
        }
        self.at += 1;
        self.skip_blank();
        let named = |value| Field {
            index,
            name: Some(name.clone()),
            value: Some(value),
            quote: None,
            single_quoted: false,
            gap: None,
        };
        let field = match self.peek() {
            Some('"' | '\'') => self.quoted(index, Some(name.clone())),
            Some(',') | Some('\n') | None => named(self.at..self.at),
            Some(c) => {
                let start = self.at;
                self.at += c.len_utf8();
                let end = self.scan_to_delimiter();
                if &self.src[start..end] == "None" {
                    return Some((None, true));
                }
                named(start..end)
            }
        };
        Some((Some(field), true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(src: &str) -> Vec<(usize, Option<String>, Option<String>, bool)> {
        parse(src)
            .iter()
            .map(|f| {
                (
                    f.index,
                    f.name(src).map(str::to_owned),
                    f.text(src),
                    f.single_quoted,
                )
            })
            .collect()
    }

    #[test]
    fn positional_indexes_count_named_and_skipped_fields() {
        let v = values("quote, role=x, Author");
        assert_eq!(v[0], (0, None, Some("quote".into()), false));
        assert_eq!(v[1], (1, Some("role".into()), Some("x".into()), false));
        assert_eq!(v[2], (2, None, Some("Author".into()), false));
        // `None` consumes an index without producing a field.
        let v = values("quote, role=None, A");
        assert_eq!(v.len(), 2);
        assert_eq!(v[1], (2, None, Some("A".into()), false));
    }

    #[test]
    fn empty_fields_are_nil_and_quoted_empties_are_strings() {
        let v = values("quote, , W");
        assert_eq!(v[1], (1, None, None, false));
        assert_eq!(v[2].2.as_deref(), Some("W"));
        let v = values("quote,\"\"");
        assert_eq!(v[1].2.as_deref(), Some(""));
        // A trailing delimiter yields a final nil positional.
        let v = values("quote, ");
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].2, None);
        assert_eq!(values("quote").len(), 1);
    }

    #[test]
    fn quotes_escape_and_recover() {
        let v = values(r#"quote, "A\"q", W"#);
        assert_eq!(v[1].2.as_deref(), Some("A\"q"));
        // An unclosed quote is literal, and commas delimit again.
        let v = values("quote, \"A, B");
        assert_eq!(v[1].2.as_deref(), Some("\"A"));
        assert_eq!(v[2].2.as_deref(), Some("B"));
        // Single quotes request substitutions unless the value starts with one.
        // `''` is an empty value; the following `'b'` is an adjacent field.
        let v = values("'a', '''b', 'c");
        assert!(v[0].3);
        assert_eq!(v[1].2.as_deref(), Some(""));
        assert!(v[1].3);
        assert_eq!((v[2].0, v[2].2.as_deref(), v[2].3), (2, Some("b"), true));
        assert_eq!(v[3].2.as_deref(), Some("'c"));
        assert!(!v[3].3);
        // A closed single-quoted value that begins with an apostrophe.
        let v = values("x, ''''");
        assert_eq!(v[1].2.as_deref(), Some(""));
        assert_eq!(values("x ''y'")[0].2.as_deref(), Some("x ''y'"));
    }

    #[test]
    fn adjacent_fields_and_gaps_follow_ruby() {
        // A closing quote without a delimiter starts the next field.
        let v = values("\"quote\" A,B");
        assert_eq!(
            v.iter().map(|f| f.2.clone().unwrap()).collect::<Vec<_>>(),
            ["quote", "A", "B"]
        );
        assert_eq!(v[1].0, 1);
        // Blanks after a leading name token become spaces.
        assert_eq!(values("A\tB, C")[0].2.as_deref(), Some("A B"));
        assert_eq!(values("#a.b, x")[0].2.as_deref(), Some("#a.b"));
        assert_eq!(values("a=, b=")[0].2.as_deref(), Some(""));
        assert_eq!(values("a=, b=")[1].2.as_deref(), Some(""));
        assert_eq!(values("Ünï=ß")[0].1.as_deref(), Some("Ünï"));
    }
}

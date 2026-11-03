//! The parts of Asciidoctor's rouge adapter (`SyntaxHighlighter::RougeAdapter`)
//! every backend shares: choosing and running the lexer of a listing's
//! `language`, and the `highlight` line spec.

use crate::guess::{self, Guess};
use crate::lexer::{self, Lexer, OptionValue, Options};
use crate::token::Token;

/// `create_lexer` and the lexing of `source` for a listing's `language`
/// (`lang?options` through `find_fancy`, php's `start_inline` unless
/// `mixed`); plain text when the lexer is not ported or fails, where Rouge
/// would raise.
pub fn lex(lang: Option<&str>, mixed: bool, source: &str) -> Vec<(Token, String)> {
    let lexer = lang.and_then(|lang| {
        if lang.contains('?') {
            // `find_fancy` without code: the query string becomes the
            // lexer's options; `guess?mimetype=…` guesses by mimetype (an
            // ambiguous guess raises in Ruby: plain text here).
            let (found, mut options) = guess::lookup_fancy(lang, None, &Options::new());
            let def = match found? {
                Guess::Found(def) => def,
                Guess::Ambiguous(_) => return None,
            };
            let def = Some(def).filter(|def| def.missing.is_empty())?;
            if def.tag == "php" && !mixed && !options.contains_key("start_inline") {
                options.insert("start_inline".into(), OptionValue::Bool(true));
            }
            Lexer::new(def, options).ok()
        } else {
            let def = lexer::find_def(lang).filter(|def| def.missing.is_empty())?;
            let mut options = Options::new();
            if def.tag == "php" && !mixed {
                options.insert("start_inline".into(), OptionValue::Bool(true));
            }
            Lexer::new(def, options).ok()
        }
    });
    if let Some(tokens) = lexer.and_then(|mut lexer| lexer.lex(source).ok()) {
        return tokens;
    }
    Lexer::new(lexer::plaintext(), Options::new())
        .and_then(|mut lexer| lexer.lex(source))
        .unwrap_or_default()
}

/// Ruby's `String#to_i`: leading whitespace, a sign and digits (with
/// single underscores between them); 0 without digits.
pub fn ruby_to_i(text: &str) -> i64 {
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: i64 = 0;
    let mut previous_underscore = false;
    for (i, c) in digits.char_indices() {
        match c {
            '0'..='9' => {
                value = value
                    .saturating_mul(10)
                    .saturating_add(i64::from(c as u8 - b'0'));
                previous_underscore = false;
            }
            '_' if i > 0 && !previous_underscore => previous_underscore = true,
            _ => break,
        }
    }
    if negative {
        -value
    } else {
        value
    }
}

/// `resolve_lines_to_highlight`: a `1-5, !2, 10` (or `1..5;!2;10`) spec as
/// sorted 1-based lines, relative to `start` when lines are numbered.
pub fn lines_to_highlight(source: &str, spec: &str, start: Option<i64>) -> Vec<i64> {
    let spec: String = spec.chars().filter(|&c| c != ' ').collect();
    let separator = if spec.contains(',') { ',' } else { ';' };
    // Ruby's split drops trailing empty fields.
    let mut entries: Vec<&str> = spec.split(separator).collect();
    while entries.last() == Some(&"") {
        entries.pop();
    }
    let line_count = source.matches('\n').count() as i64 + 1;
    let mut lines: Vec<i64> = Vec::new();
    for entry in entries {
        let (negate, entry) = match entry.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, entry),
        };
        let delimiter = if entry.contains("..") {
            Some("..")
        } else if entry.contains('-') {
            Some("-")
        } else {
            None
        };
        if let Some(delimiter) = delimiter {
            let (from, to) = entry.split_once(delimiter).unwrap_or((entry, ""));
            let to = if to.is_empty() || ruby_to_i(to) < 0 {
                line_count
            } else {
                ruby_to_i(to)
            };
            let range = ruby_to_i(from)..=to;
            if negate {
                lines.retain(|line| !range.contains(line));
            } else {
                for line in range {
                    if !lines.contains(&line) {
                        lines.push(line);
                    }
                }
            }
        } else if negate {
            let line = ruby_to_i(entry);
            lines.retain(|&l| l != line);
        } else {
            let line = ruby_to_i(entry);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    let shift = start.map_or(0, |start| start - 1);
    let mut lines: Vec<i64> = lines.into_iter().map(|line| line - shift).collect();
    lines.sort_unstable();
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruby_to_i_reads_like_ruby() {
        assert_eq!(ruby_to_i(" 12abc"), 12);
        assert_eq!(ruby_to_i("-3"), -3);
        assert_eq!(ruby_to_i("1_000"), 1000);
        assert_eq!(ruby_to_i("x"), 0);
    }

    #[test]
    fn highlight_specs_resolve_like_ruby() {
        let source = "a\nb\nc\nd\ne";
        assert_eq!(lines_to_highlight(source, "1-3, !2, 5", None), [1, 3, 5]);
        assert_eq!(lines_to_highlight(source, "2..", None), [2, 3, 4, 5]);
        assert_eq!(lines_to_highlight(source, "11;12", Some(10)), [2, 3]);
    }

    #[test]
    fn unknown_languages_lex_as_plain_text() {
        let tokens = lex(Some("no-such-language"), false, "x = 1");
        assert_eq!(
            tokens.iter().map(|(_, t)| t.as_str()).collect::<String>(),
            "x = 1"
        );
        let rust = lex(Some("rust"), false, "fn main() {}");
        assert!(rust.len() > 1);
    }
}

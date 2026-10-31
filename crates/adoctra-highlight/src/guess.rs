//! Rouge's language guessing (`Lexer.guess`, `Lexer.lookup_fancy`): the
//! Mimetype, Modeline and Source guessers over `Lexer.all` in Rouge's order.
//! Each lexer's `detect?` is generated from Rouge (`GuessEntry.detect`).

use crate::lexer::{self, LexerDef, OptionValue, Options};
use std::sync::OnceLock;

/// One lexer of `Lexer.all`: its index in the lexer table, aliases,
/// mimetypes and `detect?` (None when the lexer is not `detectable?`).
pub struct GuessEntry {
    pub lexer: usize,
    pub aliases: &'static [&'static str],
    pub mimetypes: &'static [&'static str],
    pub detect: Option<fn(&TextAnalyzer) -> bool>,
}

/// `Rouge::TextAnalyzer`: the normalized source with its shebang and
/// doctype.
pub struct TextAnalyzer {
    text: String,
    shebang: OnceLock<Option<String>>,
    doctype: OnceLock<Option<String>>,
}

/// The first group of a generated regexp's first match.
fn capture(index: usize, text: &str) -> Option<String> {
    let captures = lexer::regex(index).search(text, 0)?;
    captures
        .get(1)
        .map(|(start, end)| text[start..end].to_string())
}

impl TextAnalyzer {
    /// `SourceNormalizer.normalize`: without a leading UTF-8 BOM, CRLF as LF.
    pub fn new(source: &str) -> TextAnalyzer {
        TextAnalyzer {
            text: normalize(source),
            shebang: OnceLock::new(),
            doctype: OnceLock::new(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// `shebang`: what follows `#!` on the first non-blank line.
    pub fn shebang(&self) -> Option<&str> {
        self.shebang
            .get_or_init(|| capture(lexer::tables().shebang, &self.text))
            .as_deref()
    }

    /// `shebang?(match)` with its generated `/\b#{match}(\s|$)/`.
    pub fn shebang_is(&self, index: usize) -> bool {
        self.shebang()
            .is_some_and(|shebang| lexer::search(index, shebang))
    }

    /// `doctype`: the name of a leading `<!DOCTYPE …>`.
    pub fn doctype(&self) -> Option<&str> {
        self.doctype
            .get_or_init(|| capture(lexer::tables().doctype, &self.text))
            .as_deref()
    }

    /// `text =~ re`.
    pub fn search(&self, index: usize) -> bool {
        lexer::search(index, &self.text)
    }
}

fn normalize(source: &str) -> String {
    source
        .strip_prefix('\u{feff}')
        .unwrap_or(source)
        .replace("\r\n", "\n")
}

/// `Lexer.guesses(mimetype:, source:)`: the lexers every guesser narrowed
/// the list to, empty when none narrowed it.
pub fn guesses(source: Option<&str>, mimetype: Option<&str>) -> Vec<&'static LexerDef> {
    let tables = lexer::tables();
    let all: Vec<&GuessEntry> = tables.guess.iter().collect();
    let original = all.len();
    let mut lexers = all;
    let apply = |lexers: &mut Vec<&GuessEntry>, filtered: Vec<&'static GuessEntry>| {
        if !filtered.is_empty() {
            *lexers = filtered;
        }
    };
    if let Some(mimetype) = mimetype {
        let filtered = lexers
            .iter()
            .copied()
            .filter(|entry| entry.mimetypes.contains(&mimetype))
            .collect();
        apply(&mut lexers, filtered);
    }
    if let Some(source) = source {
        let analyzer = TextAnalyzer::new(source);
        let filtered = modeline(&lexers, analyzer.text());
        apply(&mut lexers, filtered);
        let filtered = detect(&lexers, &analyzer);
        apply(&mut lexers, filtered);
    }
    if lexers.len() < original {
        lexers
            .into_iter()
            .map(|entry| tables.lexers[entry.lexer])
            .collect()
    } else {
        Vec::new()
    }
}

/// `Guessers::Modeline#filter`: an Emacs or Vim modeline in the first or
/// last five lines names the language.
fn modeline(lexers: &[&'static GuessEntry], text: &str) -> Vec<&'static GuessEntry> {
    if lexers.len() == 1 {
        return lexers.to_vec();
    }
    // `split(/\n/)` drops trailing empty fields.
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let first = &lines[..lines.len().min(5)];
    let last = &lines[lines.len().saturating_sub(5)..];
    let space = [first, last].concat().join("\n");
    let names: Vec<String> = lexer::tables()
        .modelines
        .iter()
        .filter_map(|&index| capture(index, &space))
        .collect();
    if names.is_empty() {
        return lexers.to_vec();
    }
    lexers
        .iter()
        .copied()
        .filter(|entry| {
            let tag = lexer::tables().lexers[entry.lexer].tag;
            names
                .iter()
                .any(|name| name == tag || entry.aliases.contains(&name.as_str()))
        })
        .collect()
}

/// `Guessers::Source#filter`: the detectable lexers whose `detect?` holds.
fn detect(lexers: &[&'static GuessEntry], text: &TextAnalyzer) -> Vec<&'static GuessEntry> {
    if lexers.len() == 1 {
        return lexers.to_vec();
    }
    lexers
        .iter()
        .copied()
        .filter(|entry| entry.detect.is_some_and(|detect| detect(text)))
        .collect()
}

/// The result of `Lexer.guess`.
pub enum Guess {
    /// One lexer (`PlainText` when no guesser narrowed the list).
    Found(&'static LexerDef),
    /// `Guesser::Ambiguous` and its alternatives.
    Ambiguous(Vec<&'static LexerDef>),
}

/// `Lexer.guess(source:, mimetype:)`.
pub fn guess(source: Option<&str>, mimetype: Option<&str>) -> Guess {
    let mut lexers = guesses(source, mimetype);
    match lexers.len() {
        0 => Guess::Found(lexer::plaintext()),
        1 => Guess::Found(lexers.remove(0)),
        _ => Guess::Ambiguous(lexers),
    }
}

/// `Lexer.lookup_fancy(str, code, default_options)`: a tag (`ruby`), CGI
/// options (`erb?parent=tex`) or `guess`; the lexer (None when not found)
/// and its options.
pub fn lookup_fancy(
    name: &str,
    code: Option<&str>,
    defaults: &Options,
) -> (Option<Guess>, Options) {
    if !name.contains('?') && name != "guess" {
        return (lexer::find_def(name).map(Guess::Found), defaults.clone());
    }
    let (name, query) = name.split_once('?').unwrap_or((name, ""));
    let mut options = defaults.clone();
    options.extend(decode_www_form(query));
    let found = if name == "guess" {
        let mimetype = match options.get("mimetype") {
            Some(OptionValue::Str(mimetype)) => Some(mimetype.clone()),
            _ => None,
        };
        Some(guess(code, mimetype.as_deref()))
    } else {
        lexer::find_def(name).map(Guess::Found)
    };
    (found, options)
}

/// `URI.decode_www_form` grouped like `lookup_fancy`: one value is a string,
/// a repeated key a list.
pub fn decode_www_form(query: &str) -> Options {
    let mut values: Vec<(String, Vec<String>)> = Vec::new();
    if !query.is_empty() {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let (key, value) = (decode_component(key), decode_component(value));
            match values.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push(value),
                None => values.push((key, vec![value])),
            }
        }
    }
    values
        .into_iter()
        .map(|(key, mut list)| {
            let value = if list.len() == 1 {
                OptionValue::Str(list.remove(0))
            } else {
                OptionValue::List(list)
            };
            (key, value)
        })
        .collect()
}

/// `URI.decode_www_form_component`: `+` is a space, `%XX` a byte.
fn decode_component(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit() =>
            {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(lexers: Vec<&LexerDef>) -> Vec<&'static str> {
        lexers.iter().map(|def| def.tag).collect()
    }

    #[test]
    fn shebangs_doctypes_and_modelines_guess_like_rouge() {
        assert_eq!(
            tags(guesses(Some("#!/usr/bin/env ruby\nputs 1\n"), None)),
            ["ruby"]
        );
        assert_eq!(tags(guesses(Some("#!/bin/bash\necho\n"), None)), ["shell"]);
        assert_eq!(
            tags(guesses(Some("<!DOCTYPE html>\n<html></html>\n"), None)),
            ["html"]
        );
        assert_eq!(
            tags(guesses(Some("x = 1\n# vim: set ft=python :\n"), None)),
            ["python"]
        );
        // Nothing narrows the list: no guess.
        assert!(guesses(Some("hello\n"), None).is_empty());
        assert!(
            matches!(guess(Some("hello\n"), None), Guess::Found(def) if def.tag == "plaintext")
        );
        assert_eq!(tags(guesses(None, Some("application/json"))), ["json"]);
    }

    #[test]
    fn lookup_fancy_parses_options_and_guesses() {
        let (found, options) = lookup_fancy("ruby?a=1&a=2&b=x+y", None, &Options::new());
        assert!(matches!(found, Some(Guess::Found(def)) if def.tag == "ruby"));
        assert_eq!(
            options.get("a"),
            Some(&OptionValue::List(vec!["1".into(), "2".into()]))
        );
        assert_eq!(options.get("b"), Some(&OptionValue::Str("x y".into())));
        let (found, _) = lookup_fancy("guess", Some("#!/usr/bin/perl\n"), &Options::new());
        assert!(matches!(found, Some(Guess::Found(def)) if def.tag == "perl"));
        let (found, _) = lookup_fancy("nope", None, &Options::new());
        assert!(found.is_none());
    }

    #[test]
    fn decodes_lexer_options() {
        let options = decode_www_form("token=Name+Builtin&a=%41&a=b&flag");
        assert_eq!(
            options.get("token"),
            Some(&OptionValue::Str("Name Builtin".into()))
        );
        assert_eq!(
            options.get("a"),
            Some(&OptionValue::List(vec!["A".into(), "b".into()]))
        );
        assert_eq!(options.get("flag"), Some(&OptionValue::Str(String::new())));
    }
}

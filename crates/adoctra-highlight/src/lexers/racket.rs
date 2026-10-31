//! Hand-ported `Rouge::Lexers::Racket.detect?`: a `#lang` line naming
//! racket or scribble.

use crate::guess::TextAnalyzer;
use crate::regex::Regex;
use std::sync::OnceLock;

pub(crate) fn detect(text: &TextAnalyzer) -> bool {
    static LANG: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    let lang = LANG.get_or_init(|| Regex::new(r"\A#lang\s*(.*?)$", 0).expect("racket regexp"));
    let name = NAME.get_or_init(|| Regex::new("racket|scribble", 0).expect("racket regexp"));
    let text = text.text();
    lang.search(text, 0)
        .and_then(|captures| captures.get(1))
        .is_some_and(|(start, end)| name.is_match(&text[start..end]))
}

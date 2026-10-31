//! Hand-ported block of `Rouge::Lexers::Vue`: a `lang` attribute chooses
//! the lexer class of the block (`lookup_lang`).

use crate::lexer::{LexError, Match, Scan};
use crate::token::{LITERAL_STRING, NAME_ATTRIBUTE, TEXT};

/// `lookup_lang(lang)`: downcased, without quotes.
fn lookup_lang(lang: &str) -> &'static str {
    let lang: String = lang
        .to_lowercase()
        .chars()
        .filter(|&c| c != '"' && c != '\'')
        .collect();
    match lang.as_str() {
        "html" => "html",
        "css" => "css",
        "javascript" => "javascript",
        "sass" => "sass",
        "scss" => "scss",
        "coffee" => "coffeescript",
        _ => "plaintext",
    }
}

/// `lang="…"` on a block's tag.
pub(crate) fn rule_86(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[NAME_ATTRIBUTE, TEXT, LITERAL_STRING]);
    s.lexer.set_class("lang", lookup_lang(m.group(3)))
}

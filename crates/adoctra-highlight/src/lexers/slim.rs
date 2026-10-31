//! Hand-ported blocks of `Rouge::Lexers::Slim`: filters (`ruby:`,
//! `javascript:` …) lex their block with a lexer from the memoized
//! `filters` Hash (`ruby` is the shared `@ruby`).
//!
//! Delegates (slim): ruby, erb, javascript, css, coffeescript, markdown, scss, sass.

use crate::lexer::{LexError, Match, Scan};
use crate::token::NAME_DECORATOR;

#[derive(Default)]
struct Vars {
    /// `@filter_lexer`: the delegate's key, None for nil.
    filter: Option<&'static str>,
}

/// `filters[name]`: the delegate key and lexer tag.
fn filter(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "ruby" => ("ruby", "ruby"),
        "erb" => ("filter_erb", "erb"),
        "javascript" => ("filter_javascript", "javascript"),
        "css" => ("filter_css", "css"),
        "coffee" => ("filter_coffee", "coffeescript"),
        "markdown" => ("filter_markdown", "markdown"),
        "scss" => ("filter_scss", "scss"),
        "sass" => ("filter_sass", "sass"),
        _ => return None,
    })
}

/// A filter line `name:`: the block below goes to the filter's lexer.
pub(crate) fn rule_63(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_DECORATOR, m.text());
    s.pop(1)?;
    crate::lexers::util::starts_block(s, "filter_block");
    let found = filter(m.group(1));
    s.lexer.vars::<Vars>().filter = found.map(|(key, _)| key);
    if let Some((key, tag)) = found {
        if !s.lexer.subs.contains_key(key) {
            let sub = s.lexer.sub_for(tag)?;
            s.lexer.subs.insert(key, sub);
        }
        s.lexer.reset_sub(key, None)?;
    }
    Ok(())
}

/// Filter content: the filter's lexer, else a decorator.
pub(crate) fn rule_181(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    match s.lexer.vars::<Vars>().filter {
        Some(key) => s.delegate_existing(key, m.text()),
        None => {
            s.token(NAME_DECORATOR, m.text());
            Ok(())
        }
    }
}

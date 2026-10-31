//! Hand-ported blocks of `Rouge::Lexers::Wollok`: the entity names a lexer
//! has seen (`@entities`, kept across `reset!` like Ruby's instance
//! variable) are classes.

use crate::lexer::{LexError, Match, Scan};
use crate::token::{KEYWORD_VARIABLE, NAME_CLASS};

#[derive(Default)]
struct Vars {
    entities: Vec<String>,
}

fn add_entity(s: &mut Scan<'_, '_>, m: &Match<'_>) {
    s.token(NAME_CLASS, m.text());
    s.lexer.vars::<Vars>().entities.push(m.text().to_string());
}

/// An entity name before `{`.
pub(crate) fn rule_42(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    add_entity(s, m);
    s.pop(1)
}

/// An entity name.
pub(crate) fn rule_47(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    add_entity(s, m);
    Ok(())
}

/// A variable: a known entity or a capitalized name is a class
/// (`('A'..'Z').cover?(variable[0])`).
pub(crate) fn rule_66(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let variable = m.text();
    let known = s
        .lexer
        .vars::<Vars>()
        .entities
        .iter()
        .any(|entity| entity == variable);
    let capital = variable
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase());
    let token = if known || capital {
        NAME_CLASS
    } else {
        KEYWORD_VARIABLE
    };
    s.token(token, variable);
    Ok(())
}

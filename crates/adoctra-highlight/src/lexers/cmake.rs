//! Hand-ported blocks of `Rouge::Lexers::CMake`: text takes the token of
//! the current state (`STATES_MAP`), and bracket arguments/comments close
//! with a bracket of their opening's length (`@bracket_len`).

use crate::lexer::{LexError, Match, Scan};
use crate::token::{Token, COMMENT_MULTILINE, LITERAL_STRING_DOUBLE, NAME_VARIABLE, TEXT};

#[derive(Default)]
struct Vars {
    /// `@bracket_len`: None is nil.
    bracket_len: Option<usize>,
}

/// `STATES_MAP[state.name.to_sym]`; nil (no token) raises in Rouge.
fn state_token(s: &Scan<'_, '_>) -> Result<Token, LexError> {
    let name = s.lexer.stack.last().map_or("", |state| state.name);
    match name {
        "root" => Ok(TEXT),
        "bracket_string" | "quoted_argument" => Ok(LITERAL_STRING_DOUBLE),
        "bracket_comment" => Ok(COMMENT_MULTILINE),
        "variable_reference" => Ok(NAME_VARIABLE),
        _ => Err(LexError(format!("no token for state {name}"))),
    }
}

/// A line break in the current state.
pub(crate) fn rule_138(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let token = state_token(s)?;
    s.token(token, m.text());
    Ok(())
}

/// Any other character in the current state.
pub(crate) fn rule_141(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    rule_138(s, m)
}

/// `]=*]`: the end of the bracket when its length matches the opening's.
pub(crate) fn rule_154(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    rule_138(s, m)?;
    if Some(m.text().chars().count()) == s.lexer.vars::<Vars>().bracket_len {
        s.goto("root")?;
    }
    Ok(())
}

/// `#[==[`: a bracket comment.
pub(crate) fn rule_166(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(COMMENT_MULTILINE, m.text());
    s.lexer.vars::<Vars>().bracket_len = Some(m.text().chars().count() - 1);
    s.goto("bracket_comment")
}

/// `[==[`: a bracket argument.
pub(crate) fn rule_171(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(LITERAL_STRING_DOUBLE, m.text());
    s.lexer.vars::<Vars>().bracket_len = Some(m.text().chars().count());
    s.goto("bracket_string")
}

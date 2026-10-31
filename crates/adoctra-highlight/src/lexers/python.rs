//! Hand-ported blocks of `Rouge::Lexers::Python` (also its subclasses): the
//! register of open strings (`StringRegister`, a stack of [type, delimiter]).

use crate::lexer::{LexError, Match, Scan};
use crate::token::{
    LITERAL_STRING, LITERAL_STRING_AFFIX, LITERAL_STRING_ESCAPE, LITERAL_STRING_HEREDOC,
    LITERAL_STRING_INTERPOL,
};

#[derive(Default)]
struct Vars {
    /// `@string_register`: [type (the lowercased prefix), delimiter].
    strings: Vec<(String, String)>,
}

/// `self.last` of the register; Ruby raises on an empty one (`nil[1]`).
fn last(s: &mut Scan<'_, '_>) -> Result<(String, String), LexError> {
    s.lexer
        .vars::<Vars>()
        .strings
        .last()
        .cloned()
        .ok_or_else(|| LexError("undefined method `[]' for nil (Python string register)".into()))
}

/// A string start: its prefix and delimiter are registered.
pub(crate) fn rule_127(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[LITERAL_STRING_AFFIX, LITERAL_STRING_HEREDOC]);
    let entry = (m.group(1).to_lowercase(), m.group(2).to_string());
    s.lexer.vars::<Vars>().strings.push(entry);
    s.push("generic_string")
}

/// A quote inside a string: the string's own delimiter closes it.
pub(crate) fn rule_214(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(LITERAL_STRING_HEREDOC, m.text());
    if last(s)?.1 == m.text() {
        s.lexer.vars::<Vars>().strings.pop();
        s.pop(1)?;
    }
    Ok(())
}

/// `{` interpolates in an f-string.
pub(crate) fn rule_224(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    if last(s)?.0.contains('f') {
        s.token(LITERAL_STRING_INTERPOL, m.text());
        s.push("generic_interpol")
    } else {
        s.token(LITERAL_STRING, m.text());
        Ok(())
    }
}

/// An escape sequence is plain string text in a raw string.
pub(crate) fn rule_235(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let token = if last(s)?.0.contains('r') {
        LITERAL_STRING
    } else {
        LITERAL_STRING_ESCAPE
    };
    s.token(token, m.text());
    s.pop(1)
}

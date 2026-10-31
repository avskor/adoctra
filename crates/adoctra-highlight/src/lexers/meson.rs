//! Hand-ported blocks of `Rouge::Lexers::Meson`: its register of open
//! strings (`StringRegister`, a stack of [type, delimiter]).

use crate::lexer::{LexError, Match, Scan};
use crate::token::{LITERAL_STRING, LITERAL_STRING_AFFIX, LITERAL_STRING_INTERPOL};

#[derive(Default)]
struct Vars {
    /// `current_string`: [type (the lowercased prefix), delimiter].
    strings: Vec<(String, String)>,
}

/// `self.last`; Ruby raises on an empty register (`nil[1]`).
fn last(s: &mut Scan<'_, '_>) -> Result<(String, String), LexError> {
    s.lexer
        .vars::<Vars>()
        .strings
        .last()
        .cloned()
        .ok_or_else(|| LexError("undefined method `[]' for nil (Meson string register)".into()))
}

/// A string start (`f'`, `'''`).
pub(crate) fn rule_58(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[LITERAL_STRING_AFFIX, LITERAL_STRING]);
    let entry = (m.group(1).to_lowercase(), m.group(2).to_string());
    s.lexer.vars::<Vars>().strings.push(entry);
    s.push("generic_string")
}

/// A quote: the string's own delimiter closes it.
pub(crate) fn rule_93(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(LITERAL_STRING, m.text());
    if last(s)?.1 == m.text() {
        s.lexer.vars::<Vars>().strings.pop();
        s.pop(1)?;
    }
    Ok(())
}

/// `@` interpolates in an f-string.
pub(crate) fn rule_103(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    if last(s)?.0.contains('f') {
        s.token(LITERAL_STRING_INTERPOL, m.text());
        s.push("generic_interpol")
    } else {
        s.token(LITERAL_STRING, m.text());
        Ok(())
    }
}

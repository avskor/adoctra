//! Hand-ported blocks of `Rouge::Lexers::Rust`: bracket counts inside a
//! macro invocation (`@macro_delims`).

use crate::lexer::{LexError, Lexer, Match, Scan};
use crate::token::PUNCTUATION;

#[derive(Default)]
struct Vars {
    /// Open `]`, `)` and `}` counts (Ruby integers, possibly negative).
    delims: [i64; 3],
}

fn slot(close: &str) -> Option<usize> {
    match close {
        "]" => Some(0),
        ")" => Some(1),
        "}" => Some(2),
        _ => None,
    }
}

/// `start { @macro_delims = { … => 0 }; push :bol }`.
pub(crate) fn start_57(lexer: &mut Lexer) -> Result<(), LexError> {
    lexer.vars::<Vars>().delims = [0; 3];
    lexer.push_named("bol")
}

/// An opening bracket in a macro.
pub(crate) fn rule_189(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let close = match m.text() {
        "[" => "]",
        "(" => ")",
        _ => "}",
    };
    if let Some(slot) = slot(close) {
        s.lexer.vars::<Vars>().delims[slot] += 1;
    }
    s.token(PUNCTUATION, m.text());
    Ok(())
}

/// A closing bracket: the macro ends when every count is back to zero.
pub(crate) fn rule_195(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let vars = s.lexer.vars::<Vars>();
    if let Some(slot) = slot(m.text()) {
        vars.delims[slot] -= 1;
    }
    if vars.delims.iter().all(|&count| count == 0) {
        s.pop(1)?;
    }
    s.token(PUNCTUATION, m.text());
    Ok(())
}

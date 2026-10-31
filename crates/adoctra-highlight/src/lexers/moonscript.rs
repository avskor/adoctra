//! Hand-ported parts of `Rouge::Lexers::Moonscript` (see `lua.rs`).

use crate::lexer::{LexError, Lexer, Match, Scan};

/// `initialize`: `function_highlighting` and `disabled_modules`.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    super::lua::init_moonscript(lexer)
}

/// A name: a builtin or a (dotted) name.
pub(crate) fn rule_72(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    super::lua::moonscript_rule_72(s, m)
}

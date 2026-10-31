//! Hand-ported blocks of `Rouge::Lexers::Crystal`: the same heredoc queue
//! and `%`-sigil strings as Ruby's (`ruby.rs`), with Crystal's sigils.

use super::ruby::{self, Sigils};
use crate::lexer::{LexError, Lexer, Match, Scan};

/// `start { push :expr_start; @heredoc_queue = [] }`.
pub(crate) fn start_137(lexer: &mut Lexer) -> Result<(), LexError> {
    ruby::start_149(lexer)
}

/// `%`-sigil strings.
pub(crate) fn rule_38(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    ruby::sigil(s, m, Sigils::Crystal)
}

/// `<<-NAME` heredocs.
pub(crate) fn rule_228(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    ruby::rule_239(s, m)
}

/// `<<""`.
pub(crate) fn rule_235(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    ruby::rule_246(s, m)
}

/// A heredoc line.
pub(crate) fn rule_260(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    ruby::rule_271(s, m)
}

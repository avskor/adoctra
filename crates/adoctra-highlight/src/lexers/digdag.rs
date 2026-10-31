//! Hand-ported block of `Rouge::Lexers::Digdag`, a YAML dialect.

use crate::lexer::{LexError, Match, Scan};
use crate::token::KEYWORD_RESERVED;

/// A digdag operator key (`sh>:`, `_do:` …): a YAML mapping key.
pub(crate) fn rule_63(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    super::yaml::implicit_key(s, m, KEYWORD_RESERVED)
}

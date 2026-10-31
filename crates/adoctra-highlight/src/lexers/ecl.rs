//! Hand-ported block of `Rouge::Lexers::ECL`: identifiers with a numeric
//! size suffix (`integer4`, `real8`, `string20`).
//!
//! Data: ECL.template, ECL.typed, ECL.type, ECL.keywords, ECL.functions.

use crate::generated::lexers::{
    DATA_ECL_FUNCTIONS, DATA_ECL_KEYWORDS, DATA_ECL_TEMPLATE, DATA_ECL_TYPE, DATA_ECL_TYPED,
};
use crate::lexer::{contains, LexError, Match, Scan};
use crate::token::{ERROR, KEYWORD, KEYWORD_CONSTANT, KEYWORD_TYPE, NAME_FUNCTION, NAME_OTHER};

/// `(#?)\b([a-z_][\w]*?)(\d*)\b`: the name's class, its size (`m[3].to_i`,
/// nil when empty) deciding the sized types.
pub(crate) fn rule_144(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let name_only = m.group(2).to_lowercase();
    let name = format!("{name_only}{}", m.group(3));
    // Digits only; a value too large for u64 is in no range below.
    let number = (!m.group(3).is_empty()).then(|| m.group(3).parse::<u64>().unwrap_or(u64::MAX));
    let token = if m.group(1) == "#" {
        if contains(DATA_ECL_TEMPLATE, &name) {
            KEYWORD_TYPE
        } else {
            ERROR
        }
    } else if (contains(DATA_ECL_TYPED, &name_only) && number.is_some())
        || contains(DATA_ECL_TYPE, &name)
    {
        KEYWORD_TYPE
    } else if contains(DATA_ECL_KEYWORDS, &name) {
        KEYWORD
    } else if contains(DATA_ECL_FUNCTIONS, &name) {
        NAME_FUNCTION
    } else if ((name_only == "integer" || name_only == "unsigned")
        && number.is_some_and(|n| (1..=8).contains(&n)))
        || (name_only == "real" && matches!(number, Some(4 | 8)))
    {
        KEYWORD_TYPE
    } else if name == "true" || name == "false" {
        KEYWORD_CONSTANT
    } else {
        NAME_OTHER
    };
    s.token(token, m.text());
    Ok(())
}

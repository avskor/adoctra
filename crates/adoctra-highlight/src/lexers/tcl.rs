//! Hand-ported blocks of `Rouge::Lexers::TCL`: braces inside a string are
//! counted (`@brace_count`, kept across `reset!`), so a close brace of an
//! enclosing braced word still ends it.

use crate::lexer::{LexError, Match, Scan};
use crate::token::{LITERAL_STRING_DOUBLE, PUNCTUATION};

#[derive(Default)]
struct Vars {
    /// `@brace_count`: None is nil.
    brace_count: Option<i64>,
}

/// `{` in a string.
pub(crate) fn rule_178(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let count = &mut s.lexer.vars::<Vars>().brace_count;
    *count = Some(count.unwrap_or(0) + 1);
    s.token(LITERAL_STRING_DOUBLE, m.text());
    Ok(())
}

/// `}` in a string: it closes the enclosing brace when the string's own
/// braces are balanced (`@brace_count.to_i == 0`).
pub(crate) fn rule_185(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let count = s.lexer.vars::<Vars>().brace_count;
    if s.in_state("brace") && count.unwrap_or(0) == 0 {
        while !s.state_is("brace") {
            s.pop(1)?;
        }
        s.pop(1)?;
        s.token(PUNCTUATION, m.text());
    } else {
        // `nil - 1` raises.
        let count = count.ok_or_else(|| LexError("undefined method `-' for nil".into()))?;
        s.lexer.vars::<Vars>().brace_count = Some(count - 1);
        s.token(LITERAL_STRING_DOUBLE, m.text());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::lexer::{find_def, Lexer, Options};

    #[test]
    fn an_unopened_close_brace_in_a_string_fails_like_ruby() {
        let mut lexer = Lexer::new(find_def("tcl").unwrap(), Options::new()).unwrap();
        let error = lexer.lex("puts \"a}\"\n").unwrap_err();
        assert_eq!(error.0, "undefined method `-' for nil");
    }
}

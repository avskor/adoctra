//! Hand-ported block of `Rouge::Lexers::Elixir`: `~`-sigil strings, whose
//! state is built from the delimiters while lexing.

use crate::lexer::{compiled, dynamic_state, Action, LexError, Match, Next, Rule, Scan};
use crate::regex::MULTILINE;
use crate::token::{Token, LITERAL_STRING_OTHER, LITERAL_STRING_REGEX};

/// `~r/…/`, `~w(…)`, `~S"…"` …
pub(crate) fn rule_107(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let delimiter = m.group(2);
    let open = crate::lexer::regexp_escape(delimiter);
    let closing = match delimiter {
        "{" => "}",
        "[" => "]",
        "(" => ")",
        "<" => ">",
        other => other,
    };
    let close = crate::lexer::regexp_escape(closing);
    // `'Rr'.include? m[1]` raises without a sigil letter.
    let kind = m
        .get(1)
        .ok_or_else(|| LexError("no implicit conversion of nil into String".into()))?;
    let interp = matches!(kind, "S" | "R" | "C" | "W");
    let mut toktype = LITERAL_STRING_OTHER;
    if "Rr".contains(kind) {
        toktype = LITERAL_STRING_REGEX;
        s.push("regex_flags")?;
    }
    if "Ww".contains(kind) {
        s.push("list_flags")?;
    }
    s.token(toktype, m.text());

    let lexer = &*s.lexer;
    let tag = lexer.def.tag;
    let mixin = lexer
        .def
        .states
        .iter()
        .position(|state| state.name == "interpoling");
    let key = format!("{tag} sigil {open} {close} {interp} {}", toktype.id());
    let state = dynamic_state(key, || rules(&open, &close, interp, toktype, mixin))?;
    s.push_state(state);
    Ok(())
}

fn rules(
    open: &str,
    close: &str,
    interp: bool,
    toktype: Token,
    mixin: Option<usize>,
) -> Result<Vec<Rule>, LexError> {
    let rule = |source: &str, options: u32| -> Result<Rule, LexError> {
        Ok(Rule::Compiled {
            regex: compiled(source, options)?,
            bol: source.starts_with('^'),
            action: Action::Emit(
                toktype,
                if source == close {
                    Next::Pop
                } else {
                    Next::Stay
                },
            ),
        })
    };
    let mut out = vec![rule(close, 0)?];
    if interp {
        let mixin = mixin.ok_or_else(|| LexError("unknown state: :interpoling".into()))?;
        out.push(Rule::Mixin(mixin));
        out.push(rule("#", 0)?);
    } else {
        out.push(rule("[\\\\#]", 0)?);
    }
    let uniq_chars = if open == close {
        open.to_string()
    } else {
        format!("{open}{close}")
    };
    out.push(rule(&format!("[^#{uniq_chars}\\\\]+"), MULTILINE)?);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::lexer::{LexError, Lexer};

    #[test]
    fn a_sigil_without_a_letter_fails_like_ruby() {
        // Rouge: `'Rr'.include? nil` raises TypeError.
        let mut lexer = Lexer::find("elixir").unwrap();
        assert_eq!(
            lexer.lex("x = ~(abc)\n").unwrap_err(),
            LexError("no implicit conversion of nil into String".into())
        );
    }
}

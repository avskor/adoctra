//! Hand-ported block of `Rouge::Lexers::Markdown`: a fenced code block is
//! lexed by the lexer its info string names (`Lexer.find_fancy`), or a
//! guessed one; an unclosed fence lexes line by line to its closing fence.

use crate::guess::{lookup_fancy, Guess};
use crate::lexer::{
    compiled, dynamic_state, plaintext, Action, LexError, Lexer, Match, OptionValue, Rule, Scan,
};
use crate::token::{NAME_LABEL, PUNCTUATION, TEXT};

/// `name = m[3].strip`: `find_fancy(name.empty? ? "guess" : name, m[5],
/// @options)`, the first alternative of an ambiguous guess, else
/// PlainText with `Str::Backtick` tokens.
fn sublexer(s: &Scan<'_, '_>, name: &str, code: Option<&str>) -> Result<Lexer, LexError> {
    let name = if name.is_empty() { "guess" } else { name };
    let options = &s.lexer.options;
    match lookup_fancy(name, code, options) {
        (Some(Guess::Found(def)), options) => Lexer::new(def, options),
        (Some(Guess::Ambiguous(lexers)), _) => Lexer::new(lexers[0], options.clone()),
        (None, _) => {
            let mut options = options.clone();
            options.insert(
                "token".into(),
                OptionValue::Str("Literal.String.Backtick".into()),
            );
            Lexer::new(plaintext(), options)
        }
    }
}

/// A fence: `` ```lang `` … `` ``` ``.
pub(crate) fn rule_35(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let name = m
        .group(3)
        .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    let mut sub = sublexer(s, name, m.get(5))?;
    sub.reset()?;
    s.token(TEXT, m.group(1));
    s.token(PUNCTUATION, m.group(2));
    s.token(NAME_LABEL, m.group(3));
    if let Some(code) = m.get(5) {
        s.delegate_to(&mut sub, code)?;
    }
    s.token(TEXT, m.group(6));
    if let Some(close) = m.get(7) {
        s.token(PUNCTUATION, close);
    } else {
        // `push do … end`: the closing fence, else a line for `sublexer`.
        s.lexer.subs.insert("fence", sub);
        let fence = m.group(2);
        let state = dynamic_state(format!("markdown fence {fence}"), || {
            Ok(vec![
                Rule::Compiled {
                    regex: compiled(&format!("^([ \\t]*)({fence})"), 0)?,
                    bol: true,
                    action: Action::Call(fence_close),
                },
                Rule::Compiled {
                    regex: compiled("^.*\\n", 0)?,
                    bol: true,
                    action: Action::Call(fence_line),
                },
            ])
        })?;
        s.push_state(state);
    }
    Ok(())
}

fn fence_close(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.pop(1)?;
    s.token(TEXT, m.group(1));
    s.token(PUNCTUATION, m.group(2));
    Ok(())
}

/// `delegate sublexer, mb[1]`: no group, so the whole line.
fn fence_line(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.delegate_existing("fence", m.text())
}

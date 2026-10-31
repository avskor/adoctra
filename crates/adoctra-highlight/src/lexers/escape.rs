//! `Rouge::Lexers::Escape`: escaped sections between `start` and `end`
//! delimiters are `Escape` tokens, the rest goes to the `lang` lexer.
//!
//! Delegates (escape): plaintext.

use crate::lexer::{find_def, plaintext, LexError, Lexer, OptionValue, Options};
use crate::token::{Token, ESCAPE};

#[derive(Default)]
struct Vars {
    start: String,
    end: String,
}

/// `initialize`: `string_option(:start) { '<!' }`,
/// `string_option(:end) { '!>' }`, `lexer_option(:lang) { PlainText.new }`.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let start = lexer
        .options
        .remove("start")
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "<!".into());
    let end = lexer
        .options
        .remove("end")
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "!>".into());
    let lang = match lexer.options.remove("lang") {
        None => Lexer::new(plaintext(), Options::new())?,
        Some(value) => {
            let name = match value {
                OptionValue::Str(name) => Some(name),
                OptionValue::List(names) => names.last().cloned(),
                OptionValue::Bool(_) => None,
            };
            // An unknown name is nil: `continue_lex` raises in Ruby.
            let def = name.as_deref().and_then(find_def).ok_or_else(|| {
                LexError("undefined method `continue_lex' for nil (@lang)".into())
            })?;
            Lexer::new(def, lexer.options.clone())?
        }
    };
    lexer.subs.insert("lang", lang);
    let vars = lexer.vars::<Vars>();
    vars.start = start;
    vars.end = end;
    Ok(())
}

fn lang(lexer: &mut Lexer, text: &str, out: &mut Vec<(Token, String)>) -> Result<(), LexError> {
    lexer
        .subs
        .get_mut("lang")
        .expect("escape lang")
        .continue_lex(text, out)
}

/// `stream_tokens`: up to each start delimiter with `@lang`, then the
/// escaped text up to the end delimiter.
pub(crate) fn stream(
    lexer: &mut Lexer,
    text: &str,
    out: &mut Vec<(Token, String)>,
) -> Result<(), LexError> {
    let (start, end) = {
        let vars = lexer.vars::<Vars>();
        (vars.start.clone(), vars.end.clone())
    };
    let mut rest = text;
    loop {
        // `/(.*?)(#{Regexp.escape(@start)})/m` at the scan position.
        let Some(at) = rest.find(start.as_str()) else {
            return lang(lexer, rest, out);
        };
        lang(lexer, &rest[..at], out)?;
        rest = &rest[at + start.len()..];
        let Some(at) = rest.find(end.as_str()) else {
            push(out, rest);
            return Ok(());
        };
        push(out, &rest[..at]);
        rest = &rest[at + end.len()..];
    }
}

/// `yield Token::Tokens::Escape, text` (`continue_lex` skips empty values).
fn push(out: &mut Vec<(Token, String)>, text: &str) {
    if !text.is_empty() {
        out.push((ESCAPE, text.to_string()));
    }
}

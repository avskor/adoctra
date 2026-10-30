//! `Rouge::Lexers::PlainText`: the whole input as one token, `Text` unless
//! the `token` option names another token type.

use crate::lexer::{LexError, Lexer};
use crate::token::{Token, TEXT};

#[derive(Default)]
struct Vars {
    token: Option<Token>,
}

/// `@token = token_option(:token) || Text` (the option is removed).
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let token = lexer
        .options
        .remove("token")
        .and_then(|value| value.as_string())
        .and_then(|name| Token::named(&name))
        .unwrap_or(TEXT);
    lexer.vars::<Vars>().token = Some(token);
    Ok(())
}

/// `yield self.token, string`.
pub(crate) fn stream(
    lexer: &mut Lexer,
    text: &str,
    out: &mut Vec<(Token, String)>,
) -> Result<(), LexError> {
    let token = lexer.vars::<Vars>().token.unwrap_or(TEXT);
    out.push((token, text.to_string()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::lexer::{Lexer, OptionValue, Options};
    use crate::token::{COMMENT, TEXT};

    #[test]
    fn yields_the_whole_text_as_one_token() {
        let mut lexer = Lexer::find("text").unwrap();
        assert_eq!(
            lexer.lex("a <b>\n").unwrap(),
            vec![(TEXT, "a <b>\n".to_string())]
        );
        let mut options = Options::new();
        options.insert("token".into(), OptionValue::Str("Comment".into()));
        let mut lexer = Lexer::new(crate::lexer::plaintext(), options).unwrap();
        assert!(lexer.options.is_empty());
        assert_eq!(lexer.lex("x").unwrap(), vec![(COMMENT, "x".to_string())]);
    }
}

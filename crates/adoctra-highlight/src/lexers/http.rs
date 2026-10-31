//! Hand-ported blocks of `Rouge::Lexers::HTTP`: a `Content-Type` header
//! chooses the body's lexer (`content_lexer`, memoized for the instance),
//! unless the `content` option names one.

use crate::guess::{self, Guess};
use crate::lexer::{find_def, LexError, Lexer, LexerDef, Match, OptionValue, Scan};
use crate::token::{LITERAL_STRING, NAME_ATTRIBUTE, PUNCTUATION, TEXT};

#[derive(Default)]
struct Vars {
    /// `@content_lexer`: None until the body is reached.
    content: Option<Content>,
}

enum Content {
    /// `lexer_option(:content)`: an instance, kept in `Lexer.subs`.
    Instance,
    /// A guessed class: a new instance for every delegation.
    Class(&'static LexerDef),
}

/// A header: `Content-Type` sets `@content_type` (its media type).
pub(crate) fn rule_63(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    // `key.strip.casecmp('content-type').zero?` (the key has no spaces).
    if m.group(1).eq_ignore_ascii_case("content-type") {
        // `value.split(';')[0].downcase`: nil (NoMethodError) when the
        // value is only semicolons.
        let value = m.group(5);
        if value.chars().all(|c| c == ';') {
            return Err(LexError("undefined method `downcase' for nil".into()));
        }
        let first = value.split(';').next().unwrap_or_default();
        s.lexer.strings.insert("content_type", first.to_lowercase());
    }
    s.groups(
        m,
        &[
            NAME_ATTRIBUTE,
            TEXT,
            PUNCTUATION,
            TEXT,
            LITERAL_STRING,
            TEXT,
        ],
    );
    Ok(())
}

/// `content_lexer`: the `content` option (`as_lexer`), else
/// `guess_content_lexer` (by mimetype; ambiguous or none is PlainText).
fn content_lexer(lexer: &mut Lexer) -> Result<(), LexError> {
    if lexer.vars::<Vars>().content.is_some() {
        return Ok(());
    }
    let option = match lexer.options.remove("content") {
        Some(OptionValue::Str(name)) => Some(name),
        Some(OptionValue::List(names)) => names.last().cloned(),
        _ => None,
    };
    let instance = match option.as_deref().and_then(find_def) {
        Some(def) => Some(Lexer::new(def, lexer.options.clone())?),
        None => None,
    };
    let content = match instance {
        Some(instance) => {
            lexer.subs.insert("content", instance);
            Content::Instance
        }
        None => {
            let mimetype = lexer.strings.get("content_type").cloned();
            Content::Class(match mimetype {
                None => crate::lexer::plaintext(),
                Some(mimetype) => match guess::guess(None, Some(&mimetype)) {
                    Guess::Found(def) => def,
                    Guess::Ambiguous(_) => crate::lexer::plaintext(),
                },
            })
        }
    };
    lexer.vars::<Vars>().content = Some(content);
    Ok(())
}

/// The body: `delegate(content_lexer)`.
pub(crate) fn rule_81(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    content_lexer(s.lexer)?;
    match s.lexer.vars::<Vars>().content {
        Some(Content::Class(def)) => s.delegate_class(def.tag, m.text()),
        _ => s.delegate_existing("content", m.text()),
    }
}

#[cfg(test)]
mod tests {
    use crate::lexer::{find_def, Lexer, Options};

    #[test]
    fn a_content_type_of_only_semicolons_fails_like_ruby() {
        let mut lexer = Lexer::new(find_def("http").unwrap(), Options::new()).unwrap();
        let error = lexer
            .lex("GET / HTTP/1.1\nContent-Type: ;;\n\nx\n")
            .unwrap_err();
        assert_eq!(error.0, "undefined method `downcase' for nil");
    }
}

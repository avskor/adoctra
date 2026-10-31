//! Hand-ported blocks of `Rouge::Lexers::Coq`: a (dotted) identifier is
//! collected in `@name`/`@id_dotted` and classified when it ends.
//!
//! Data: Coq.coq, Coq.gallina, Coq.ltac, Coq.terminators, Coq.tacticals.

use crate::generated::lexers::{
    DATA_COQ_COQ, DATA_COQ_GALLINA, DATA_COQ_LTAC, DATA_COQ_TACTICALS, DATA_COQ_TERMINATORS,
};
use crate::lexer::{contains, LexError, Match, Scan};
use crate::token::{
    Token, KEYWORD, KEYWORD_PSEUDO, KEYWORD_RESERVED, NAME_CONSTANT, NAME_EXCEPTION,
    NAME_NAMESPACE, PUNCTUATION, PUNCTUATION_INDICATOR, TEXT_WHITESPACE,
};

#[derive(Default)]
struct Vars {
    /// `@name`: None is `false` (or nil).
    name: Option<String>,
    id_dotted: bool,
}

/// `Coq.classify`.
fn classify(name: &str) -> Token {
    if contains(DATA_COQ_COQ, name) {
        KEYWORD
    } else if contains(DATA_COQ_GALLINA, name) {
        KEYWORD_RESERVED
    } else if contains(DATA_COQ_LTAC, name) {
        KEYWORD_PSEUDO
    } else if contains(DATA_COQ_TERMINATORS, name) {
        NAME_EXCEPTION
    } else if contains(DATA_COQ_TACTICALS, name) {
        KEYWORD_PSEUDO
    } else {
        NAME_CONSTANT
    }
}

fn start_id(s: &mut Scan<'_, '_>, m: &Match<'_>) {
    let vars = s.lexer.vars::<Vars>();
    vars.name = Some(m.text().to_string());
    vars.id_dotted = false;
}

/// An identifier starting a sentence.
pub(crate) fn rule_93(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    start_id(s, m);
    s.push("sentence_postid")?;
    s.push("continue_id")
}

/// An identifier in proof mode.
pub(crate) fn rule_131(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    rule_93(s, m)
}

/// An identifier after the first one of a sentence.
pub(crate) fn rule_179(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    start_id(s, m);
    s.push("continue_id")
}

/// `token tok, @name`: `false` has no `empty?` in Rouge.
fn name_token(s: &mut Scan<'_, '_>, token: Option<Token>) -> Result<(), LexError> {
    let vars = s.lexer.vars::<Vars>();
    let name = vars
        .name
        .clone()
        .ok_or_else(|| LexError("undefined method `empty?' for false".into()))?;
    let token = token.unwrap_or(if vars.id_dotted {
        NAME_CONSTANT
    } else {
        classify(&name)
    });
    s.token(token, &name);
    Ok(())
}

/// `.name`: the collected part was a namespace.
pub(crate) fn rule_219(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    name_token(s, Some(NAME_NAMESPACE))?;
    s.token(PUNCTUATION, ".");
    let vars = s.lexer.vars::<Vars>();
    vars.id_dotted = true;
    vars.name = Some(m.group(1).to_string());
    Ok(())
}

fn finish(s: &mut Scan<'_, '_>) -> Result<(), LexError> {
    let vars = s.lexer.vars::<Vars>();
    vars.name = None;
    vars.id_dotted = false;
    s.pop(1)
}

/// `. `: the identifier ends the sentence.
pub(crate) fn rule_226(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    name_token(s, None)?;
    s.token(PUNCTUATION_INDICATOR, ".");
    s.token(TEXT_WHITESPACE, m.group(1));
    finish(s)?;
    s.pop(1)
}

/// The identifier ends.
pub(crate) fn rule_241(s: &mut Scan<'_, '_>, _: &Match<'_>) -> Result<(), LexError> {
    name_token(s, None)?;
    finish(s)
}

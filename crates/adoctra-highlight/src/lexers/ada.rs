//! Hand-ported blocks of `Rouge::Lexers::Ada`: identifiers classified by
//! `Ada.idents` (a Hash defaulting to `Name`).

use crate::lexer::{LexError, Match, Scan};
use crate::token::{
    Token, KEYWORD, KEYWORD_DECLARATION, NAME, NAME_BUILTIN, NAME_NAMESPACE, OPERATOR_WORD,
};

const KEYWORDS: &[&str] = &[
    "abort",
    "abstract",
    "accept",
    "access",
    "aliased",
    "all",
    "array",
    "at",
    "begin",
    "body",
    "case",
    "constant",
    "declare",
    "delay",
    "delta",
    "digits",
    "do",
    "else",
    "elsif",
    "end",
    "exception",
    "exit",
    "for",
    "generic",
    "goto",
    "if",
    "in",
    "interface",
    "is",
    "limited",
    "loop",
    "new",
    "null",
    "of",
    "others",
    "out",
    "overriding",
    "parallel",
    "pragma",
    "private",
    "protected",
    "raise",
    "range",
    "record",
    "renames",
    "requeue",
    "return",
    "reverse",
    "select",
    "separate",
    "some",
    "synchronized",
    "tagged",
    "task",
    "terminate",
    "then",
    "until",
    "use",
    "when",
    "while",
    "with",
];
const OPERATOR_WORDS: &[&str] = &["abs", "and", "mod", "not", "or", "rem", "xor"];
const DECLARATIONS: &[&str] = &[
    "entry",
    "function",
    "package",
    "procedure",
    "subtype",
    "type",
];
const BUILTINS: &[&str] = &[
    "boolean",
    "character",
    "constraint_error",
    "duration",
    "float",
    "integer",
    "natural",
    "positive",
    "long_float",
    "long_integer",
    "long_long_float",
    "long_long_integer",
    "program_error",
    "short_float",
    "short_integer",
    "short_short_integer",
    "storage_error",
    "string",
    "tasking_error",
    "wide_character",
    "wide_string",
    "wide_wide_character",
    "wide_wide_string",
];

/// `self.class.idents[m[0].downcase]`.
fn ident(text: &str) -> Token {
    let name = text.to_lowercase();
    let name = name.as_str();
    if KEYWORDS.contains(&name) {
        KEYWORD
    } else if OPERATOR_WORDS.contains(&name) {
        OPERATOR_WORD
    } else if DECLARATIONS.contains(&name) {
        KEYWORD_DECLARATION
    } else if BUILTINS.contains(&name) {
        NAME_BUILTIN
    } else {
        NAME
    }
}

/// A name in a `with` clause; a keyword means the state was entered by
/// mistake.
pub(crate) fn rule_94(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let t = ident(m.text());
    // `t <= Name`: Name and its subtokens.
    if t == NAME || t == NAME_BUILTIN {
        s.token(NAME_NAMESPACE, m.text());
        Ok(())
    } else {
        s.token(t, m.text());
        if t == KEYWORD_DECLARATION {
            s.goto("decl_name")
        } else {
            s.pop(1)
        }
    }
}

/// An identifier; a declaration keyword names what follows.
pub(crate) fn rule_149(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let t = ident(m.text());
    s.token(t, m.text());
    if t == KEYWORD_DECLARATION {
        s.push("decl_name")?;
    }
    Ok(())
}

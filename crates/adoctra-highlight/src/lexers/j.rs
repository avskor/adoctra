//! Hand-ported blocks of `Rouge::Lexers::J`: primitives classified by their
//! inflection (`J.primitive`), control words, explicit definitions
//! (`@def_body`) and notes (`@note_next`).
//!
//! Data: J.control_words, J.control_words_id.

use crate::generated::lexers::{DATA_J_CONTROL_WORDS, DATA_J_CONTROL_WORDS_ID};
use crate::lexer::{contains, LexError, Lexer, Match, Scan};
use crate::token::{
    Token, ERROR, KEYWORD, KEYWORD_CONSTANT, KEYWORD_PSEUDO, LITERAL_NUMBER, LITERAL_STRING_SINGLE,
    NAME, NAME_BUILTIN_PSEUDO, NAME_FUNCTION, NAME_LABEL, OPERATOR, PUNCTUATION, TEXT,
};

#[derive(Default)]
struct Vars {
    note_next: bool,
    /// `@def_body`: `noun` or `code`, None for nil.
    def_body: Option<&'static str>,
}

/// A part of speech (`token_map` keys; None is nil, an `Error`).
#[derive(Clone, Copy)]
enum Pos {
    Noun,
    Verb,
    Modifier,
    Name,
    Param,
    Other,
}

use Pos::*;

/// `token_map`.
fn token(pos: Option<Pos>) -> Token {
    match pos {
        Some(Noun) => KEYWORD_CONSTANT,
        Some(Verb) => NAME_FUNCTION,
        Some(Modifier) => OPERATOR,
        Some(Name) => NAME,
        Some(Param) => NAME_BUILTIN_PSEUDO,
        Some(Other) => PUNCTUATION,
        None => ERROR,
    }
}

const V: Option<Pos> = Some(Verb);
const M: Option<Pos> = Some(Modifier);
const N: Option<Pos> = Some(Name);
const P: Option<Pos> = Some(Param);
const O: Option<Pos> = Some(Other);

/// `primitive_table[char]` (default `[:name]`).
fn primitive_table(c: char) -> &'static [Option<Pos>] {
    match c {
        '(' | ')' => &[O],
        '=' => &[V, O, O],
        '<' | '>' | '+' | '-' | '*' | '%' | '$' | '|' | ',' | '#' => &[V, V, V],
        '^' => &[V, V, M],
        '~' | '"' => &[M, V, V],
        '.' | ':' | '@' => &[M, M, M],
        ';' => &[V, M, V],
        '!' => &[V, M, M],
        '/' | '\\' => &[M, M, V],
        '[' => &[V, None, V],
        ']' => &[V],
        '{' => &[V, V, V, None, None, None, V],
        '}' => &[M, V, V, None, None, None, M],
        '`' => &[M, None, M],
        '&' => &[M, M, M, None, M],
        '?' => &[V, V],
        'a' => &[N, Some(Noun), Some(Noun)],
        'A' | 'C' | 'e' | 'E' | 'I' | 'j' | 'o' | 'r' => &[N, V],
        'b' | 'd' | 'f' | 'H' | 'M' | 'T' => &[N, M],
        'D' | 't' => &[N, M, M],
        'F' => &[N, M, M, M, M, M, M],
        'i' => &[N, V, V],
        'L' => &[N, V, M],
        'm' | 'n' | 'y' => &[P],
        'p' => &[N, V, V, V],
        'q' | 's' | 'Z' => &[N, None, V],
        'S' => &[N, None, M],
        // `'iu'` is overwritten by the later `'u'` entry.
        'u' => &[P, V, V],
        'v' => &[P, V],
        'x' => &[P, None, V],
        _ => &[N],
    }
}

/// `J.primitive(char, inflection)`.
fn primitive(char: &str, inflection: &str) -> Token {
    const INFLECTIONS: [&str; 7] = ["", ".", ":", "..", ".:", ":.", "::"];
    let Some(i) = INFLECTIONS.iter().position(|&x| x == inflection) else {
        return ERROR;
    };
    let c = char.chars().next().unwrap_or(' ');
    token(primitive_table(c).get(i).copied().flatten())
}

fn vars(lexer: &mut Lexer) -> &mut Vars {
    lexer.vars::<Vars>()
}

/// `start { @note_next = false }`.
pub(crate) fn start_134(lexer: &mut Lexer) -> Result<(), LexError> {
    vars(lexer).note_next = false;
    Ok(())
}

/// A primitive.
pub(crate) fn rule_91(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(primitive(m.group(1), m.group(2)), m.text());
    Ok(())
}

/// `1:` and the like.
pub(crate) fn rule_95(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let token = if m.group(1).is_empty() {
        NAME_FUNCTION
    } else {
        ERROR
    };
    s.token(token, m.text());
    Ok(())
}

/// A number.
pub(crate) fn rule_99(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let token = if m.group(1).is_empty() {
        LITERAL_NUMBER
    } else {
        ERROR
    };
    s.token(token, m.text());
    Ok(())
}

/// A name, or a control word (`if.`, `for_x.`, `label_a.`).
pub(crate) fn rule_107(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    if m.group(2) != "." {
        let token = if m.group(2).is_empty() { NAME } else { ERROR };
        s.token(token, m.text());
        return Ok(());
    }
    // `word, sep, id = m[1].partition '_'`.
    let name = m.group(1);
    let (word, sep, id) = match name.find('_') {
        Some(at) => (&name[..at], "_", &name[at + 1..]),
        None => (name, "", ""),
    };
    let list = if sep.is_empty() {
        Some(DATA_J_CONTROL_WORDS)
    } else if !id.is_empty() {
        Some(DATA_J_CONTROL_WORDS_ID)
    } else {
        None
    };
    if list.is_some_and(|list| contains(list, word)) {
        s.token(KEYWORD, &format!("{word}{sep}"));
        s.token(if word == "for" { NAME } else { NAME_LABEL }, id);
        s.token(KEYWORD, m.group(2));
    } else {
        s.token(ERROR, m.text());
    }
    Ok(())
}

/// A line break: a note starts after a `Note` line.
pub(crate) fn rule_139(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT, m.text());
    if vars(s.lexer).note_next {
        s.push("note")?;
        vars(s.lexer).note_next = false;
    }
    Ok(())
}

/// `verb define`, `3 : 0` …: an explicit definition.
pub(crate) fn rule_149(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[KEYWORD_PSEUDO, TEXT, KEYWORD_PSEUDO, TEXT]);
    let body = if m.group(1) == "0" || m.group(1) == "noun" {
        "noun"
    } else {
        "code"
    };
    vars(s.lexer).def_body = Some(body);
    if m.group(3) == "define" {
        if s.lexer.stack.len() > 1 {
            s.pop(1)?;
        }
        s.push(body)?;
        s.push("def_next")
    } else {
        s.push("expl_def")
    }
}

/// `Note`: the following lines are a note.
pub(crate) fn rule_166(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[TEXT, NAME, TEXT]);
    vars(s.lexer).note_next = true;
    Ok(())
}

fn def_body(s: &mut Scan<'_, '_>) -> Result<&'static str, LexError> {
    vars(s.lexer)
        .def_body
        .ok_or_else(|| LexError("invalid state: nil".into()))
}

/// `0` after `:`: the body follows on the next lines.
pub(crate) fn rule_181(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(KEYWORD_PSEUDO, m.text());
    if s.lexer.stack.len() > 2 {
        s.pop(1)?;
    }
    let body = def_body(s)?;
    s.goto(body)?;
    s.push("def_next")
}

/// `'`: a one-line body (a noun's string, else code).
pub(crate) fn rule_189(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    if vars(s.lexer).def_body == Some("noun") {
        s.token(LITERAL_STRING_SINGLE, m.text());
        s.goto("str")
    } else {
        s.token(PUNCTUATION, m.text());
        s.goto("q_expr")
    }
}

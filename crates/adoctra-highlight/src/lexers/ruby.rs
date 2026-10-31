//! Hand-ported blocks of `Rouge::Lexers::Ruby` (also `irb_output`): the
//! heredoc queue and `%`-sigil strings, whose state is built from the
//! delimiters while lexing.

use crate::lexer::{compiled, dynamic_state, Action, LexError, Lexer, Match, Next, Rule, Scan};
use crate::regex::MULTILINE;
use crate::token::{
    Token, LITERAL_STRING_ESCAPE, LITERAL_STRING_HEREDOC, LITERAL_STRING_OTHER,
    LITERAL_STRING_REGEX, NAME_CONSTANT, OPERATOR,
};
use std::collections::VecDeque;

#[derive(Default)]
struct Vars {
    /// `@heredoc_queue`: [tolerant (`<<-`/`<<~`), name].
    heredoc_queue: VecDeque<(bool, String)>,
}

/// `start { push :expr_start; @heredoc_queue = [] }`.
pub(crate) fn start_149(lexer: &mut Lexer) -> Result<(), LexError> {
    lexer.push_named("expr_start")?;
    lexer.vars::<Vars>().heredoc_queue.clear();
    Ok(())
}

/// The two `%`-sigil dialects: Ruby's, and Crystal's (no interpolation
/// without a type letter, delimiters repeated in the character classes).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sigils {
    Ruby,
    Crystal,
}

/// `%`-sigil strings: `%r(…)`, `%w[…]`, `%Q<…>`, `%.abc.` … The nested
/// state depends on the delimiters, the interpolation and the token type.
pub(crate) fn rule_42(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    sigil(s, m, Sigils::Ruby)
}

pub(crate) fn sigil(s: &mut Scan<'_, '_>, m: &Match<'_>, dialect: Sigils) -> Result<(), LexError> {
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
    let kind = m.get(1);
    // Ruby: `/[rQWxI]/ === m[1] || !m[1]`; Crystal without `|| !m[1]`.
    let interp = kind.map_or(dialect == Sigils::Ruby, |k| {
        matches!(k, "r" | "Q" | "W" | "x" | "I")
    });
    let mut toktype = LITERAL_STRING_OTHER;
    if kind == Some("r") {
        toktype = LITERAL_STRING_REGEX;
        s.push("regex_flags")?;
    }
    s.token(toktype, m.text());

    let lexer = &*s.lexer;
    let tag = lexer.def.tag;
    let mixin = lexer
        .def
        .states
        .iter()
        .position(|state| state.name == "string_intp_escaped");
    let key = format!("{tag} sigil {open} {close} {interp} {}", toktype.id());
    let state = dynamic_state(key, || {
        sigil_rules(&open, &close, interp, toktype, mixin, dialect)
    })?;
    s.push_state(state);
    Ok(())
}

fn sigil_rules(
    open: &str,
    close: &str,
    interp: bool,
    toktype: Token,
    mixin: Option<usize>,
    dialect: Sigils,
) -> Result<Vec<Rule>, LexError> {
    let rule = |source: &str, options: u32, action: Action| -> Result<Rule, LexError> {
        Ok(Rule::Compiled {
            regex: compiled(source, options)?,
            bol: source.starts_with('^'),
            action,
        })
    };
    let uniq_chars = match dialect {
        Sigils::Crystal => format!("{open}{close}"),
        Sigils::Ruby if open != close => format!("{open}{close}"),
        Sigils::Ruby if open == "\\#" => String::new(),
        Sigils::Ruby => open.to_string(),
    };
    let mut rules = vec![rule(
        &format!("\\\\[#{uniq_chars}\\\\]"),
        0,
        Action::Emit(LITERAL_STRING_ESCAPE, Next::Stay),
    )?];
    // Nesting rules only with asymmetric delimiters.
    if open != close {
        rules.push(rule(open, 0, Action::Emit(toktype, Next::Push))?);
    }
    rules.push(rule(close, 0, Action::Emit(toktype, Next::Pop))?);
    if interp {
        let mixin = mixin.ok_or_else(|| LexError("unknown state: :string_intp_escaped".into()))?;
        rules.push(Rule::Mixin(mixin));
        rules.push(rule("#", 0, Action::Emit(toktype, Next::Stay))?);
    } else {
        rules.push(rule("[\\\\#]", 0, Action::Emit(toktype, Next::Stay))?);
    }
    rules.push(rule(
        &format!("[^#{uniq_chars}\\\\]+"),
        MULTILINE,
        Action::Emit(toktype, Next::Stay),
    )?);
    Ok(rules)
}

fn queue_heredoc(s: &mut Scan<'_, '_>, m: &Match<'_>, name: &str) -> Result<(), LexError> {
    s.token(OPERATOR, m.group(1));
    let constant = format!("{}{}{}", m.group(2), m.group(3), m.group(4));
    s.token(NAME_CONSTANT, &constant);
    let tolerant = matches!(m.get(1), Some("<<-" | "<<~"));
    s.lexer
        .vars::<Vars>()
        .heredoc_queue
        .push_back((tolerant, name.to_string()));
    if !s.state_is("heredoc_queue") {
        s.push("heredoc_queue")?;
    }
    Ok(())
}

/// `<<~NAME`, `<<-"NAME"` …: the heredoc waits for the end of the line.
pub(crate) fn rule_239(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    queue_heredoc(s, m, m.group(3))
}

/// `<<""` (an empty heredoc name).
pub(crate) fn rule_246(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    queue_heredoc(s, m, "")
}

/// A heredoc line: the first queued name ends that heredoc.
pub(crate) fn rule_271(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let text = m.text();
    let first = s.lexer.vars::<Vars>().heredoc_queue.front().cloned();
    // `tolerant ? m[0].strip : m[0].rstrip`; nil when the queue is empty.
    let found = first.is_some_and(|(tolerant, name)| {
        let check = if tolerant {
            ruby_strip(text)
        } else {
            ruby_rstrip(text)
        };
        check == name
    });
    if found {
        let vars = s.lexer.vars::<Vars>();
        vars.heredoc_queue.pop_front();
        if vars.heredoc_queue.is_empty() {
            s.pop(1)?;
        }
        s.token(NAME_CONSTANT, text);
    } else {
        s.token(LITERAL_STRING_HEREDOC, text);
    }
    s.pop(1)
}

fn ruby_space(c: char) -> bool {
    c.is_ascii_whitespace() || c == '\u{b}' || c == '\0'
}

/// `String#strip`.
fn ruby_strip(text: &str) -> &str {
    text.trim_matches(ruby_space)
}

/// `String#rstrip`.
fn ruby_rstrip(text: &str) -> &str {
    text.trim_end_matches(ruby_space)
}

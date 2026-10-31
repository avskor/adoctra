//! Hand-ported blocks of `Rouge::Lexers::YAML` (also `digdag`): the
//! indentation stack of block collections and block scalars.

use crate::lexer::{LexError, Lexer, Match, Scan};
use crate::token::{ERROR, NAME_CONSTANT, NAME_NAMESPACE, NAME_TAG, PUNCTUATION_INDICATOR, TEXT};

#[derive(Default)]
struct Vars {
    indent_stack: Vec<usize>,
    next_indent: usize,
    /// `@block_scalar_indent` (Ruby integers; nil is `None`).
    block_scalar_indent: Option<i64>,
}

fn vars<'a>(s: &'a mut Scan<'_, '_>) -> &'a mut Vars {
    s.lexer.vars::<Vars>()
}

/// `reset_indent`.
fn reset_indent(vars: &mut Vars) {
    vars.indent_stack = vec![0];
    vars.next_indent = 0;
    vars.block_scalar_indent = None;
}

/// `indent`: the top of the indentation stack.
fn indent(vars: &Vars) -> Result<usize, LexError> {
    vars.indent_stack
        .last()
        .copied()
        .ok_or_else(|| LexError("empty indent stack!".into()))
}

/// `save_indent(match)`: dedents to the match's level; returns the part of
/// the match within the remaining indentation and the excess.
fn save_indent<'m>(vars: &mut Vars, text: &'m str) -> Result<(&'m str, &'m str), LexError> {
    vars.next_indent = text.chars().count();
    if vars.next_indent < indent(vars)? {
        while vars.next_indent < indent(vars)? {
            vars.indent_stack.pop();
        }
        let at = indent(vars)?.min(text.len());
        Ok((&text[..at], &text[at..]))
    } else {
        Ok((text, ""))
    }
}

/// `continue_indent(match)`.
fn continue_indent(vars: &mut Vars, text: &str) {
    vars.next_indent += text.chars().count();
}

/// `set_indent(match, implicit:)`.
fn set_indent(vars: &mut Vars, text: &str, implicit: bool) -> Result<(), LexError> {
    if indent(vars)? < vars.next_indent {
        vars.indent_stack.push(vars.next_indent);
    }
    if !implicit {
        vars.next_indent += text.chars().count();
    }
    Ok(())
}

/// `start { reset_indent }`.
pub(crate) fn start_78(lexer: &mut Lexer) -> Result<(), LexError> {
    reset_indent(lexer.vars::<Vars>());
    Ok(())
}

/// `%YAML`.
pub(crate) fn rule_92(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_TAG, m.text());
    reset_indent(vars(s));
    s.push("yaml_directive")
}

/// `%TAG`.
pub(crate) fn rule_98(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_TAG, m.text());
    reset_indent(vars(s));
    s.push("tag_directive")
}

/// A document marker `---` or `...`.
pub(crate) fn rule_105(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_NAMESPACE, m.text());
    reset_indent(vars(s));
    s.push("block_line")
}

/// A line's indentation: excess indentation after a dedent is an error.
pub(crate) fn rule_112(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let (text, err) = save_indent(vars(s), m.text())?;
    s.token(TEXT, text);
    s.token(ERROR, err);
    s.push("block_line")?;
    s.push("indentation")
}

/// Spaces before an indicator continue the indentation.
pub(crate) fn rule_123(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT, m.text());
    continue_indent(vars(s), m.text());
    Ok(())
}

/// `?`, `:` or `-` opens a block collection level.
pub(crate) fn rule_129(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    set_indent(vars(s), m.text(), false)?;
    s.token(PUNCTUATION_INDICATOR, m.text());
    Ok(())
}

/// The end of the indentation.
pub(crate) fn rule_135(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT, m.text());
    continue_indent(vars(s), m.text());
    s.pop(1)
}

/// A mapping key `key:` (an implicit indentation level).
pub(crate) fn rule_176(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    implicit_key(s, m, crate::token::NAME_ATTRIBUTE)
}

/// A mapping key with its token (digdag's keywords share it).
pub(crate) fn implicit_key(
    s: &mut Scan<'_, '_>,
    m: &Match<'_>,
    key: crate::token::Token,
) -> Result<(), LexError> {
    s.groups(m, &[key, PUNCTUATION_INDICATOR]);
    set_indent(vars(s), m.text(), true)
}

/// Ruby's `text[0...n]` and `text[n..-1]` of an all-space line (nil when
/// `n` is past the end).
fn split_spaces(text: &str, mark: i64) -> (&str, &str) {
    let len = text.len() as i64;
    let mark = if mark < 0 {
        (len + mark).max(0)
    } else {
        mark.min(len)
    };
    text.split_at(mark as usize)
}

/// A blank line in a block scalar: spaces past its indentation are content.
pub(crate) fn rule_222(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let text = m.text();
    let indent_size = text.chars().count() as i64;
    let mark = vars(s).block_scalar_indent.unwrap_or(indent_size);
    let (indentation, content) = split_spaces(text, mark);
    s.token(TEXT, indentation);
    s.token(NAME_CONSTANT, content);
    Ok(())
}

/// A block scalar line's indentation: less than the scalar's ends it.
pub(crate) fn rule_234(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT, m.text());
    let indent_size = m.text().chars().count() as i64;
    let vars_ref = vars(s);
    let dedent_level = match vars_ref.block_scalar_indent {
        Some(level) => level,
        None => indent(vars_ref)? as i64,
    };
    vars_ref.block_scalar_indent.get_or_insert(indent_size);
    if indent_size < dedent_level {
        save_indent(vars(s), m.text())?;
        s.pop(1)?;
        s.push("indentation")?;
    }
    Ok(())
}

/// A block scalar header (`|2-`, `>+`): an explicit indentation indicator.
pub(crate) fn rule_254(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    vars(s).block_scalar_indent = None;
    s.goto("ignored_line")?;
    if m.text().is_empty() {
        return Ok(());
    }
    // `increment = m[1] || m[2]`: the whole indicator (group 1) always
    // matched, so `to_i` reads its sign and digit ("+" is 0).
    if let Some(increment) = m.get(1).or(m.get(2)) {
        let level = indent(vars(s))? as i64 + ruby_to_i(increment);
        vars(s).block_scalar_indent = Some(level);
    }
    s.token(PUNCTUATION_INDICATOR, m.text());
    Ok(())
}

/// The indentation of a plain scalar's next line: at most the collection's
/// indentation ends the scalar.
pub(crate) fn rule_313(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT, m.text());
    s.pop(1)?;
    let indent_size = m.text().chars().count();
    if indent_size <= indent(vars(s))? {
        s.pop(1)?;
        save_indent(vars(s), m.text())?;
        s.push("indentation")?;
    }
    Ok(())
}

/// Ruby's `String#to_i` of an indicator (`+`, `-3`, `2`).
fn ruby_to_i(text: &str) -> i64 {
    let (sign, digits) = match text.as_bytes().first() {
        Some(b'-') => (-1, &text[1..]),
        Some(b'+') => (1, &text[1..]),
        _ => (1, text),
    };
    let value: i64 = digits
        .chars()
        .take_while(char::is_ascii_digit)
        .fold(0, |n, d| n * 10 + (d as i64 - '0' as i64));
    sign * value
}

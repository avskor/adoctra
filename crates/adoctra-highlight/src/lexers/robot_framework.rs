//! Hand-ported blocks of `Rouge::Lexers::RobotFramework`: the cell column
//! (`@col`), the state of the next cell (`@next`) and template tests
//! (`@is_template`), set in `initialize` and kept across `reset!`.
//!
//! Data: RobotFramework.settings_with_keywords, RobotFramework.settings_with_args.

use crate::generated::lexers::{
    DATA_ROBOTFRAMEWORK_SETTINGS_WITH_ARGS, DATA_ROBOTFRAMEWORK_SETTINGS_WITH_KEYWORDS,
};
use crate::lexer::{contains, LexError, Lexer, Match, Scan};
use crate::token::{
    GENERIC_HEADING, NAME_BUILTIN_PSEUDO, NAME_FUNCTION, NAME_LABEL, NAME_VARIABLE, PUNCTUATION,
    TEXT_WHITESPACE,
};

#[derive(Default)]
struct Vars {
    col: i64,
    /// `@next`: a state name, None for nil.
    next: Option<&'static str>,
    is_template: bool,
}

fn vars<'a>(s: &'a mut Scan<'_, '_>) -> &'a mut Vars {
    s.lexer.vars::<Vars>()
}

/// `initialize`: `@col = 0; @next = nil; @is_template = false`.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    *lexer.vars::<Vars>() = Vars::default();
    Ok(())
}

/// `:keyword` or `:args` for a setting name, else nil.
fn setting_state(name: &str) -> Option<&'static str> {
    if contains(DATA_ROBOTFRAMEWORK_SETTINGS_WITH_KEYWORDS, name) {
        Some("keyword")
    } else if contains(DATA_ROBOTFRAMEWORK_SETTINGS_WITH_ARGS, name) {
        Some("args")
    } else {
        None
    }
}

/// A cell separator: the next cell is a keyword or arguments.
pub(crate) fn rule_75(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT_WHITESPACE, m.text());
    let vars = vars(s);
    vars.col += 1;
    let state = if let Some(next) = vars.next {
        next
    } else if vars.is_template {
        "args"
    } else if vars.col == 1 {
        vars.next = Some("keyword");
        "keyword"
    } else {
        "args"
    };
    s.push(state)?;
    s.push("cell_start")
}

/// `...`: a continuation.
pub(crate) fn rule_91(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT_WHITESPACE, m.text());
    vars(s).col += 1;
    s.push("args")
}

/// A line break starts a row.
pub(crate) fn rule_101(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(TEXT_WHITESPACE, m.text());
    let vars = vars(s);
    vars.col = 0;
    vars.next = None;
    s.push("cell_start")
}

/// A section heading (`*** Test Cases ***`).
pub(crate) fn rule_114(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(GENERIC_HEADING, m.text());
    let name = m.group(1);
    let name = name.strip_suffix('s').unwrap_or(name).to_lowercase();
    let state = match name.as_str() {
        "setting" => "section_settings",
        "test case" => "section_tests",
        "task" => "section_tasks",
        "keyword" => "section_keywords",
        "variable" => "section_variables",
        _ => return Ok(()),
    };
    s.push(state)
}

/// A setting name.
pub(crate) fn rule_129(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    vars(s).next = setting_state(&m.group(1).to_lowercase());
    s.groups(m, &[NAME_BUILTIN_PSEUDO, PUNCTUATION]);
    Ok(())
}

/// A test name.
pub(crate) fn rule_146(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    vars(s).is_template = false;
    s.token(NAME_LABEL, m.text());
    Ok(())
}

/// A variable definition.
pub(crate) fn rule_173(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_VARIABLE, m.text());
    vars(s).next = Some("args");
    s.push("var")
}

/// `[Setting]` in a test or keyword.
pub(crate) fn rule_186(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.groups(m, &[PUNCTUATION, NAME_BUILTIN_PSEUDO, PUNCTUATION]);
    let name = m.group(2).to_lowercase();
    let vars = vars(s);
    if name == "template" {
        vars.is_template = true;
    }
    if let Some(state) = setting_state(&name) {
        vars.next = Some(state);
    }
    s.pop(1)
}

/// A variable: the following cells are keywords (`@next = :keyword
/// unless @next.nil?`).
pub(crate) fn rule_200(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_VARIABLE, m.text());
    keyword_next(s);
    s.push("var")
}

fn keyword_next(s: &mut Scan<'_, '_>) {
    let vars = vars(s);
    if vars.next.is_some() {
        vars.next = Some("keyword");
    }
}

/// `FOR`.
pub(crate) fn rule_206(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_FUNCTION, m.text());
    keyword_next(s);
    Ok(())
}

/// A keyword name.
pub(crate) fn rule_214(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_FUNCTION, m.text());
    vars(s).next = None;
    Ok(())
}

/// A variable in arguments.
pub(crate) fn rule_223(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    rule_200(s, m)
}

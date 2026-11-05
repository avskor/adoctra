//! Hand-ported parts of `Rouge::Lexers::Lua` and `Moonscript`: builtin
//! function names by module.
//!
//! Lua reads `function_highlighting`/`disabled_modules` with Symbol keys
//! (`opts.delete(:function_highlighting)`), which `lang?options` (String
//! keys) never set: every builtin is highlighted. Moonscript reads them
//! with `bool_option`/`list_option`.
//!
//! Data: Lua.builtins.

use crate::generated::lexers::DATA_LUA_BUILTINS;
use crate::lexer::{LexError, Lexer, Match, OptionValue, Scan};
use crate::token::{NAME, NAME_BUILTIN, PUNCTUATION};

#[derive(Default)]
struct Vars {
    /// `@function_highlighting` is false.
    disabled: bool,
    disabled_modules: Vec<String>,
}

/// Lua's `initialize`: its options are never set through String keys.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    lexer.vars::<Vars>();
    Ok(())
}

/// Moonscript's `initialize`.
pub(crate) fn init_moonscript(lexer: &mut Lexer) -> Result<(), LexError> {
    let highlighting = lexer
        .options
        .get("function_highlighting")
        .is_none_or(OptionValue::as_bool);
    let modules = match lexer.options.remove("disabled_modules") {
        Some(OptionValue::Str(text)) => split(&text),
        Some(OptionValue::List(items)) => items.iter().flat_map(|item| split(item)).collect(),
        _ => Vec::new(),
    };
    let vars = lexer.vars::<Vars>();
    vars.disabled = !highlighting;
    vars.disabled_modules = modules;
    Ok(())
}

fn split(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = text.split(',').map(str::to_string).collect();
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

fn builtin(lexer: &mut Lexer, name: &str) -> bool {
    let vars = lexer.vars::<Vars>();
    !vars.disabled
        && DATA_LUA_BUILTINS
            .iter()
            .filter(|(module, _)| !vars.disabled_modules.iter().any(|m| m == module))
            .any(|(_, functions)| functions.binary_search(&name).is_ok())
}

/// `a.b` is a name, a dot and a name.
fn dotted(s: &mut Scan<'_, '_>, name: &str) {
    if let Some((a, b)) = name.split_once('.') {
        s.token(NAME, a);
        s.token(PUNCTUATION, ".");
        s.token(NAME, b);
    } else {
        s.token(NAME, name);
    }
}

/// A Lua name: `gsub`, a builtin or a (dotted) name.
pub(crate) fn rule_73(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let name = m.text();
    if name == "gsub" {
        s.token(NAME_BUILTIN, name);
        return s.push("gsub");
    }
    if builtin(s.lexer, name) {
        s.token(NAME_BUILTIN, name);
    } else {
        dotted(s, name);
    }
    Ok(())
}

/// A Moonscript name: a builtin or a (dotted) name.
pub(crate) fn moonscript_rule_72(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let name = m.text();
    if builtin(s.lexer, name) {
        s.token(NAME_BUILTIN, name);
    } else {
        dotted(s, name);
    }
    Ok(())
}

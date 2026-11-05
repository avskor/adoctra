//! Hand-ported parts of `Rouge::Lexers::PHP` (also `hack`): its options,
//! the start state they choose and builtin function names.
//!
//! Data: PHP.builtins, PHP.keywords, Hack.keywords.

use crate::generated::lexers::{DATA_HACK_KEYWORDS, DATA_PHP_BUILTINS, DATA_PHP_KEYWORDS};
use crate::lexer::{LexError, Lexer, Match, OptionValue, Scan};
use crate::token::{KEYWORD, NAME_BUILTIN, NAME_FUNCTION};

#[derive(Default)]
struct Vars {
    /// `@start_inline`: `None` is `:guess`.
    start_inline: Option<bool>,
    funcnamehighlighting: bool,
    disabledmodules: Vec<String>,
}

/// `initialize`: `bool_option(:start_inline) { :guess }`,
/// `bool_option(:funcnamehighlighting) { true }` and
/// `list_option(:disabledmodules)`.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let start_inline = lexer.options.get("start_inline").map(OptionValue::as_bool);
    let funcnamehighlighting = lexer
        .options
        .get("funcnamehighlighting")
        .is_none_or(OptionValue::as_bool);
    let disabledmodules = match lexer.options.remove("disabledmodules") {
        Some(OptionValue::Str(text)) => split(&text),
        Some(OptionValue::List(items)) => items.iter().flat_map(|item| split(item)).collect(),
        _ => Vec::new(),
    };
    let vars = lexer.vars::<Vars>();
    vars.start_inline = start_inline;
    vars.funcnamehighlighting = funcnamehighlighting;
    vars.disabledmodules = disabledmodules;
    Ok(())
}

/// Ruby's `split(',')`, without trailing empty fields.
fn split(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = text.split(',').map(str::to_string).collect();
    while parts.last().is_some_and(String::is_empty) {
        parts.pop();
    }
    parts
}

/// `start`: inline PHP starts in `:php`, a guess in `:start`.
pub(crate) fn start_71(lexer: &mut Lexer) -> Result<(), LexError> {
    match lexer.vars::<Vars>().start_inline {
        Some(true) => lexer.push_named("php"),
        None => lexer.push_named("start"),
        Some(false) => Ok(()),
    }
}

/// `builtins.include?(name)`: the functions of the modules not disabled,
/// none without `funcnamehighlighting`.
fn builtin(lexer: &mut Lexer, name: &str) -> bool {
    let vars = lexer.vars::<Vars>();
    if !vars.funcnamehighlighting {
        return false;
    }
    DATA_PHP_BUILTINS
        .iter()
        .filter(|(module, _)| {
            !vars
                .disabledmodules
                .iter()
                .any(|disabled| disabled == module)
        })
        .any(|(_, functions)| functions.binary_search(&name).is_ok())
}

/// A name before `(`: a keyword (`self.class.keywords`: hack has its own),
/// a builtin function or a function.
pub(crate) fn rule_108(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let name = m.text().to_lowercase();
    let keywords = if s.lexer.def.tag == "hack" {
        DATA_HACK_KEYWORDS
    } else {
        DATA_PHP_KEYWORDS
    };
    let token = if crate::lexer::contains(keywords, &name) {
        KEYWORD
    } else if builtin(s.lexer, &name) {
        NAME_BUILTIN
    } else {
        NAME_FUNCTION
    };
    s.token(token, m.text());
    Ok(())
}

//! Hand-ported hooks of `Rouge::Lexers::Lasso`: the `start_inline` option
//! starts in Lasso code.

use crate::lexer::{LexError, Lexer, OptionValue};

#[derive(Default)]
struct Vars {
    start_inline: bool,
}

/// `@start_inline = bool_option(:start_inline)` (false when absent).
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let start_inline = lexer
        .options
        .get("start_inline")
        .is_some_and(OptionValue::as_bool);
    lexer.vars::<Vars>().start_inline = start_inline;
    Ok(())
}

/// `start { push :lasso if start_inline? }`.
pub(crate) fn start_33(lexer: &mut Lexer) -> Result<(), LexError> {
    if lexer.vars::<Vars>().start_inline {
        lexer.push_named("lasso")?;
    }
    Ok(())
}

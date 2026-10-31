//! `Rouge::Indentation` (util.rb), mixed into haml, slim and sass: a block
//! state entered by the next more indented line.

use crate::lexer::{LexError, Lexer, Scan};

#[derive(Default)]
struct Vars {
    block_state: Option<String>,
    block_indentation: Option<String>,
    /// Survives `reset!`, like Ruby's `@last_indentation`.
    last_indentation: Option<String>,
}

/// `reset!`: `@block_state = @block_indentation = nil`.
pub(crate) fn reset(lexer: &mut Lexer) -> Result<(), LexError> {
    let vars = lexer.vars::<Vars>();
    vars.block_state = None;
    vars.block_indentation = None;
    Ok(())
}

/// `starts_block(state)`: the next more indented line enters `state`.
pub(crate) fn starts_block(s: &mut Scan<'_, '_>, state: &str) {
    let vars = s.lexer.vars::<Vars>();
    vars.block_state = Some(state.to_string());
    vars.block_indentation = Some(vars.last_indentation.clone().unwrap_or_default());
}

/// `indentation(indent_str)`: a line more indented than the block start
/// enters the block state, any other line `:content`.
pub(crate) fn indentation(s: &mut Scan<'_, '_>, indent: &str) -> Result<(), LexError> {
    let vars = s.lexer.vars::<Vars>();
    vars.last_indentation = Some(indent.to_string());
    let block = match (&vars.block_state, &vars.block_indentation) {
        (Some(state), Some(base)) if indent.starts_with(base.as_str()) && indent != base => {
            Some(state.clone())
        }
        _ => None,
    };
    match block {
        Some(state) => s.push(&state),
        None => {
            let vars = s.lexer.vars::<Vars>();
            vars.block_state = None;
            vars.block_indentation = None;
            s.push("content")
        }
    }
}

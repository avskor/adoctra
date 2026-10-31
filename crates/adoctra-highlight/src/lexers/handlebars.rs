//! Hand-ported block of `Rouge::Lexers::Handlebars`: text between mustache
//! tags goes to the parent (HTML) lexer.

use crate::lexer::{LexError, Match, Scan};

/// Parent text; an unquoted attribute value ends the parent's `:attr`.
pub(crate) fn rule_30(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.delegate_sub("parent", super::template_lexer::parent, m.text())?;
    let parent = s.lexer.subs.get_mut("parent").expect("parent lexer");
    if parent
        .stack()
        .last()
        .is_some_and(|state| state.name == "attr")
    {
        if parent.stack().is_empty() {
            return Err(LexError("empty stack!".into()));
        }
        parent.stack().pop();
    }
    Ok(())
}

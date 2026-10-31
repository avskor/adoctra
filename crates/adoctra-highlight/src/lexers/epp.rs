//! Hand-ported `initialize` of `Rouge::Lexers::EPP`: its parent defaults
//! to plain text, and it keeps a Puppet lexer.
//!
//! Delegates (epp): plaintext, puppet.

use crate::lexer::{find_def, plaintext, LexError, Lexer, OptionValue};

/// `@parent = lexer_option(:parent) { PlainText.new(opts) }` and
/// `@puppet_lexer = Puppet.new(opts)`.
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let options = lexer.options.clone();
    let parent = match lexer.options.remove("parent") {
        None => Lexer::new(plaintext(), options.clone())?,
        Some(value) => {
            let name = match value {
                OptionValue::Str(name) => Some(name),
                OptionValue::List(names) => names.last().cloned(),
                OptionValue::Bool(_) => None,
            };
            // An unknown name is nil: `parent.reset!` raises in Ruby.
            let def = name
                .as_deref()
                .and_then(find_def)
                .ok_or_else(|| LexError("undefined method `reset!' for nil (@parent)".into()))?;
            Lexer::new(def, lexer.options.clone())?
        }
    };
    lexer.subs.insert("parent", parent);
    let puppet = Lexer::new(find_def("puppet").expect("puppet lexer"), options)?;
    lexer.subs.insert("puppet_lexer", puppet);
    Ok(())
}

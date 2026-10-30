//! `Rouge::TemplateLexer`: the parent language of a template lexer.

use crate::lexer::{find_def, LexError, Lexer, OptionValue};

/// `parent`: `lexer_option(:parent) || Lexers::HTML.new(@options)`.
pub(crate) fn parent(lexer: &Lexer) -> Result<Lexer, LexError> {
    let named = match lexer.options.get("parent") {
        Some(OptionValue::Str(name)) => find_def(name),
        Some(OptionValue::List(names)) => names.last().and_then(|name| find_def(name)),
        _ => None,
    };
    let mut options = lexer.options.clone();
    options.remove("parent");
    Lexer::new(
        named.unwrap_or_else(|| find_def("html").expect("html lexer")),
        options,
    )
}

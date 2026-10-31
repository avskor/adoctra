//! Hand-ported parts of `Rouge::Lexers::Haml`: its filters (`:javascript`,
//! `:ruby` …, created in `initialize` and overridable with the
//! `filters[name]=lexer` options) and `ruby!` (reset the shared Ruby lexer,
//! then push a state).
//!
//! Delegates (haml): javascript, css, ruby, erb, markdown, sass, html, plaintext.

use crate::lexer::{find_def, plaintext, LexError, Lexer, Match, OptionValue, Scan};
use crate::regex::Regex;
use crate::token::{NAME_DECORATOR, PUNCTUATION};
use std::sync::OnceLock;

#[derive(Default)]
struct Vars {
    /// `@filters`, in Hash order.
    filters: Vec<(String, Filter)>,
    /// `@filter_lexer`: an index into `filters`, None for nil.
    filter: Option<usize>,
}

enum Filter {
    /// `@ruby`, shared with `ruby`.
    Ruby,
    Own(Lexer),
}

fn make_ruby(lexer: &Lexer) -> Result<Lexer, LexError> {
    lexer.sub_for("ruby")
}

/// `initialize`: the default filters, then `hash_option(:filters, …)`
/// (`as_lexer(v) || PlainText.new(@options)`).
pub(crate) fn init(lexer: &mut Lexer) -> Result<(), LexError> {
    let mut filters = Vec::new();
    for (name, tag) in [
        ("javascript", "javascript"),
        ("css", "css"),
        ("ruby", ""),
        ("erb", "erb"),
        ("markdown", "markdown"),
        ("sass", "sass"),
    ] {
        let filter = if tag.is_empty() {
            // `ruby`: `@ruby ||= Ruby.new(@options)`.
            if !lexer.subs.contains_key("ruby") {
                let ruby = make_ruby(lexer)?;
                lexer.subs.insert("ruby", ruby);
            }
            Filter::Ruby
        } else {
            Filter::Own(lexer.sub_for(tag)?)
        };
        filters.push((name.to_string(), filter));
    }
    // `filters` itself is not a Hash here (CGI options are Strings).
    lexer.options.remove("filters");
    let keys: Vec<String> = lexer.options.keys().cloned().collect();
    for key in keys {
        // `key =~ /(\w+)\[(\w+)\]/ and $1 == name`.
        let Some(name) = filter_key(&key) else {
            continue;
        };
        let value = lexer.options.remove(&key).expect("option");
        let tag = match value {
            OptionValue::Str(tag) => Some(tag),
            OptionValue::List(tags) => tags.last().cloned(),
            OptionValue::Bool(_) => None,
        };
        let def = tag.as_deref().and_then(find_def).unwrap_or_else(plaintext);
        let filter = Filter::Own(Lexer::new(def, lexer.options.clone())?);
        match filters.iter_mut().find(|(known, _)| *known == name) {
            Some(entry) => entry.1 = filter,
            None => filters.push((name, filter)),
        }
    }
    lexer.vars::<Vars>().filters = filters;
    Ok(())
}

/// The filter name of a `filters[name]` option key: `key =~
/// /(\w+)\[(\w+)\]/ and $1 == 'filters'`.
fn filter_key(key: &str) -> Option<String> {
    static KEY: OnceLock<Regex> = OnceLock::new();
    let regex = KEY.get_or_init(|| Regex::new(r"(\w+)\[(\w+)\]", 0).expect("haml regexp"));
    let captures = regex.search(key, 0)?;
    let (start, end) = captures.get(1)?;
    let (name_start, name_end) = captures.get(2)?;
    (&key[start..end] == "filters").then(|| key[name_start..name_end].to_string())
}

/// `ruby!(state)`.
fn ruby_bang(s: &mut Scan<'_, '_>, state: &str) -> Result<(), LexError> {
    s.lexer.reset_sub("ruby", Some(make_ruby))?;
    s.push(state)
}

/// `-`: Ruby code.
pub(crate) fn rule_97(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(PUNCTUATION, m.text());
    s.reset_stack();
    ruby_bang(s, "ruby_line")
}

/// A filter line `:name`.
pub(crate) fn rule_104(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(NAME_DECORATOR, m.text());
    s.pop(1)?;
    crate::lexers::util::starts_block(s, "filter_block");
    let name = m
        .group(1)
        .trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    let vars = s.lexer.vars::<Vars>();
    let filter = vars.filters.iter().position(|(known, _)| known == name);
    vars.filter = filter;
    let Some(index) = filter else {
        return Ok(());
    };
    match &mut vars.filters[index].1 {
        Filter::Own(lexer) => lexer.reset(),
        Filter::Ruby => s.lexer.reset_sub("ruby", Some(make_ruby)),
    }
}

/// `{`: a Ruby attribute hash.
pub(crate) fn rule_127(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(PUNCTUATION, m.text());
    ruby_bang(s, "ruby_tag")
}

/// `=`, `!`, `&=` …: evaluated Ruby.
pub(crate) fn rule_147(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    s.token(PUNCTUATION, m.text());
    s.reset_stack();
    ruby_bang(s, "ruby_line")
}

/// Filter content: the filter's lexer, else a decorator.
pub(crate) fn rule_194(s: &mut Scan<'_, '_>, m: &Match<'_>) -> Result<(), LexError> {
    let vars = s.lexer.vars::<Vars>();
    let Some(index) = vars.filter else {
        s.token(NAME_DECORATOR, m.text());
        return Ok(());
    };
    let filter = std::mem::replace(&mut vars.filters[index].1, Filter::Ruby);
    let filter = match filter {
        Filter::Ruby => {
            s.delegate_existing("ruby", m.text())?;
            Filter::Ruby
        }
        Filter::Own(mut lexer) => {
            s.delegate_to(&mut lexer, m.text())?;
            Filter::Own(lexer)
        }
    };
    s.lexer.vars::<Vars>().filters[index].1 = filter;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::filter_key;

    #[test]
    fn filter_option_keys() {
        assert_eq!(filter_key("filters[ruby]").as_deref(), Some("ruby"));
        assert_eq!(filter_key("x.filters[js]").as_deref(), Some("js"));
        assert_eq!(filter_key("other[ruby]"), None);
        assert_eq!(filter_key("filters"), None);
    }
}

//! Ruby's rouge adapter (`SyntaxHighlighter::RougeAdapter`) over the
//! adoctra-highlight port: server-side highlighting of source listings.
//!
//! Like Ruby's `highlight_source`, callout marks are taken out of the
//! source, the lexer chosen by `language` (`lang?options` through
//! `find_fancy`, php's `start_inline`) tokenizes it, the formatter of the
//! `rouge-css` mode renders the tokens (lines of `highlight` emphasized,
//! numbered with `linenums` from `start`) and the marks are appended to
//! their lines again. A language without a ported lexer, and a lexer
//! failure where Rouge would raise, fall back to plain text.
//!
//! The adapter keeps Ruby's per-document state: the first listing fixes the
//! class-mode style (`@style ||=`), inline mode sets it per listing, and a
//! class-mode listing makes the head embed (or link) the theme stylesheet.

use adoctra_core::ast::{Block, Callout};
use adoctra_core::Attributes;
use adoctra_highlight::format::{self, Style};
use adoctra_highlight::lexer::{self, Lexer, OptionValue, Options};
use adoctra_highlight::theme::{ThemeData, DEFAULT_THEME};
use adoctra_highlight::token::Token;
use std::cell::RefCell;

/// The adapter instance's state, shared by nested documents.
#[derive(Debug, Default)]
pub(super) struct RougeState {
    /// `@style`.
    style: Option<&'static ThemeData>,
    /// `@requires_stylesheet`.
    requires_stylesheet: bool,
}

/// `(style = opts[:style]) && (style_available? style) || DEFAULT_STYLE`.
fn theme(attrs: &Attributes) -> &'static ThemeData {
    attrs
        .get("rouge-style")
        .and_then(ThemeData::find)
        .or_else(|| ThemeData::find(DEFAULT_THEME))
        .expect("default rouge theme")
}

/// A source listing, like the adapter's `format` around `highlight`.
pub(super) fn listing(
    block: &Block,
    text: &str,
    callouts: &[Callout],
    nowrap: &str,
    state: &RefCell<RougeState>,
    out: &mut String,
) {
    let class_mode = open(block, nowrap, state, out);
    let lang = block.attrs.named.get("language").map(String::as_str);
    out.push_str(&highlight(block, text, callouts, lang, class_mode, state));
    out.push_str("</code></pre>\n");
}

/// A source listing whose substitutions do not highlight (custom `subs`):
/// the adapter's markup around the substituted content.
pub(super) fn listing_with(
    block: &Block,
    nowrap: &str,
    state: &RefCell<RougeState>,
    out: &mut String,
    content: impl FnOnce(&mut String),
) {
    open(block, nowrap, state, out);
    content(out);
    out.push_str("</code></pre>\n");
}

/// The adapter's `format`: inline mode sets `@style` and styles the `<pre>`
/// with the theme's base style. Returns whether the CSS mode is `class`.
fn open(block: &Block, nowrap: &str, state: &RefCell<RougeState>, out: &mut String) -> bool {
    let attrs = block.conversion_attributes();
    let class_mode = attrs.get("rouge-css").unwrap_or("class") == "class";
    let mut pre_style = None;
    if !class_mode {
        let theme = theme(attrs);
        state.borrow_mut().style = Some(theme);
        pre_style = theme.base_style;
    }
    let class = super::class_attr(&["rouge", "highlight", nowrap], &block.roles);
    let id = super::id_attr(&block.id);
    let style = pre_style.map_or_else(String::new, |style| format!(" style=\"{style}\""));
    out.push_str(&format!("<pre{class}{id}{style}><code"));
    if let Some(lang) = block.attrs.named.get("language") {
        // `format` strips the lexer options from the language.
        let data_lang = lang.split('?').next().unwrap_or(lang);
        out.push_str(&format!(" data-lang=\"{}\"", super::escape(data_lang)));
    }
    out.push('>');
    class_mode
}

/// `highlight_source` with the adapter's `highlight`.
fn highlight(
    block: &Block,
    text: &str,
    callouts: &[Callout],
    lang: Option<&str>,
    class_mode: bool,
    state: &RefCell<RougeState>,
) -> String {
    let attrs = block.conversion_attributes();
    let (source, marks) = extract_callouts(text, callouts);
    let named = &block.attrs.named;
    let start = named.get("linenums").map(|_| {
        let start = named.get("start").map_or(1, |start| ruby_to_i(start));
        start.max(1)
    });
    let highlight_lines = named
        .get("highlight")
        .map(|spec| resolve_lines_to_highlight(&source, spec, start));

    let style = {
        let mut state = state.borrow_mut();
        if class_mode {
            state.requires_stylesheet = true;
        }
        *state.style.get_or_insert_with(|| theme(attrs))
    };
    let tokens = lex(block, &source, lang);
    let style = if class_mode {
        Style::Classes
    } else {
        Style::Inline(style)
    };
    let mut highlighted = match &highlight_lines {
        Some(lines) => format::format_highlighted_lines(&tokens, style, lines),
        None => format::format(&tokens, style),
    };
    let mut source_offset = None;
    if let Some(start) = start {
        highlighted = format::line_table(&highlighted, start);
        if !marks.is_empty() {
            const CODE_CELL: &str = "<td class=\"code\">";
            source_offset = highlighted
                .find(CODE_CELL)
                .map(|index| index + CODE_CELL.len());
        }
    }
    let highlighted = if marks.is_empty() {
        highlighted
    } else {
        restore_callouts(&highlighted, &marks, source_offset)
    };
    trim_blank_edge_lines(highlighted)
}

/// `Block#content` of a verbatim block: the substituted (here highlighted)
/// text split into lines without blank (`rstrip.empty?`) leading and
/// trailing ones, unless it has fewer than two lines.
fn trim_blank_edge_lines(text: String) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.len() < 2 {
        return text;
    }
    let blank = |line: &&str| {
        line.trim_end_matches(|c: char| c.is_ascii_whitespace() || c == '\0' || c == '\u{b}')
            .is_empty()
    };
    let start = lines
        .iter()
        .position(|line| !blank(line))
        .unwrap_or(lines.len());
    let end = lines
        .iter()
        .rposition(|line| !blank(line))
        .map_or(start, |end| end + 1);
    lines[start..end.max(start)].join("\n")
}

/// `create_lexer` and the lexing; plain text when the lexer is not ported
/// or fails.
fn lex(block: &Block, source: &str, lang: Option<&str>) -> Vec<(Token, String)> {
    let mixed = block.attrs.options.iter().any(|o| o == "mixed")
        || block.attrs.named.contains_key("mixed-option");
    let lexer = lang.and_then(|lang| {
        if let Some((name, query)) = lang.split_once('?') {
            // `find_fancy`: the query string becomes the lexer's options.
            let def = lexer::find_def(name).filter(|def| def.missing.is_empty())?;
            let mut options = decode_www_form(query);
            if def.tag == "php" && !mixed && !options.contains_key("start_inline") {
                options.insert("start_inline".into(), OptionValue::Bool(true));
            }
            Lexer::new(def, options).ok()
        } else {
            let def = lexer::find_def(lang).filter(|def| def.missing.is_empty())?;
            let mut options = Options::new();
            if def.tag == "php" && !mixed {
                options.insert("start_inline".into(), OptionValue::Bool(true));
            }
            Lexer::new(def, options).ok()
        }
    });
    if let Some(tokens) = lexer.and_then(|mut lexer| lexer.lex(source).ok()) {
        return tokens;
    }
    Lexer::new(lexer::plaintext(), Options::new())
        .and_then(|mut lexer| lexer.lex(source))
        .unwrap_or_default()
}

/// `URI.decode_www_form` grouped like `lookup_fancy`: one value is a string,
/// a repeated key a list.
fn decode_www_form(query: &str) -> Options {
    let mut values: Vec<(String, Vec<String>)> = Vec::new();
    if !query.is_empty() {
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let (key, value) = (decode_component(key), decode_component(value));
            match values.iter_mut().find(|(k, _)| *k == key) {
                Some((_, list)) => list.push(value),
                None => values.push((key, vec![value])),
            }
        }
    }
    values
        .into_iter()
        .map(|(key, mut list)| {
            let value = if list.len() == 1 {
                OptionValue::Str(list.remove(0))
            } else {
                OptionValue::List(list)
            };
            (key, value)
        })
        .collect()
}

/// `URI.decode_www_form_component`: `+` is a space, `%XX` a byte.
fn decode_component(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len()
                && bytes[i + 1].is_ascii_hexdigit()
                && bytes[i + 2].is_ascii_hexdigit() =>
            {
                let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
                out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
                i += 2;
            }
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A callout mark taken out of the source: its 1-based line and markup.
struct Mark {
    line: usize,
    callout: Callout,
}

/// `extract_callouts`: the source without its marks (the space before a
/// guard stays) and the marks by line. A mark on the last line adds a LF.
fn extract_callouts(text: &str, callouts: &[Callout]) -> (String, Vec<Mark>) {
    let mut source = String::new();
    let mut marks = Vec::new();
    let mut copied = 0;
    for callout in callouts {
        source.push_str(&text[copied..callout.start]);
        let line = text[..callout.start].matches('\n').count() + 1;
        marks.push(Mark {
            line,
            callout: callout.clone(),
        });
        copied = callout.end;
    }
    source.push_str(&text[copied..]);
    let last_line = text.matches('\n').count() + 1;
    if marks.last().is_some_and(|mark| mark.line == last_line) {
        source.push('\n');
    }
    (source, marks)
}

/// `restore_callouts`: each line's marks after it, separated by a space.
fn restore_callouts(highlighted: &str, marks: &[Mark], source_offset: Option<usize>) -> String {
    let (preamble, code) = highlighted.split_at(source_offset.unwrap_or(0));
    let mut out = preamble.to_string();
    for (index, line) in code.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line);
        let mut first = true;
        for mark in marks.iter().filter(|mark| mark.line == index + 1) {
            if !first {
                out.push(' ');
            }
            first = false;
            let callout = &mark.callout;
            let guard = callout.guard.as_deref().map(super::escape);
            super::render_conum(
                &callout.number,
                guard.as_deref(),
                callout.xml,
                callout.icons.as_deref(),
                &callout.src,
                &mut out,
            );
        }
    }
    out
}

/// Ruby's `String#to_i`: leading whitespace, a sign and digits (with
/// single underscores between them); 0 without digits.
fn ruby_to_i(text: &str) -> i64 {
    let text = text.trim_start();
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let mut value: i64 = 0;
    let mut previous_underscore = false;
    for (i, c) in digits.char_indices() {
        match c {
            '0'..='9' => {
                value = value
                    .saturating_mul(10)
                    .saturating_add(i64::from(c as u8 - b'0'));
                previous_underscore = false;
            }
            '_' if i > 0 && !previous_underscore => previous_underscore = true,
            _ => break,
        }
    }
    if negative {
        -value
    } else {
        value
    }
}

/// `resolve_lines_to_highlight`: a `1-5, !2, 10` (or `1..5;!2;10`) spec as
/// sorted 1-based lines, relative to `start` when lines are numbered.
fn resolve_lines_to_highlight(source: &str, spec: &str, start: Option<i64>) -> Vec<i64> {
    let spec: String = spec.chars().filter(|&c| c != ' ').collect();
    let separator = if spec.contains(',') { ',' } else { ';' };
    // Ruby's split drops trailing empty fields.
    let mut entries: Vec<&str> = spec.split(separator).collect();
    while entries.last() == Some(&"") {
        entries.pop();
    }
    let line_count = source.matches('\n').count() as i64 + 1;
    let mut lines: Vec<i64> = Vec::new();
    for entry in entries {
        let (negate, entry) = match entry.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, entry),
        };
        let delimiter = if entry.contains("..") {
            Some("..")
        } else if entry.contains('-') {
            Some("-")
        } else {
            None
        };
        if let Some(delimiter) = delimiter {
            let (from, to) = entry.split_once(delimiter).unwrap_or((entry, ""));
            let to = if to.is_empty() || ruby_to_i(to) < 0 {
                line_count
            } else {
                ruby_to_i(to)
            };
            let range = ruby_to_i(from)..=to;
            if negate {
                lines.retain(|line| !range.contains(line));
            } else {
                for line in range {
                    if !lines.contains(&line) {
                        lines.push(line);
                    }
                }
            }
        } else if negate {
            let line = ruby_to_i(entry);
            lines.retain(|&l| l != line);
        } else {
            let line = ruby_to_i(entry);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
    }
    let shift = start.map_or(0, |start| start - 1);
    let mut lines: Vec<i64> = lines.into_iter().map(|line| line - shift).collect();
    lines.sort_unstable();
    lines
}

/// The adapter's head docinfo after a class-mode listing: the theme
/// stylesheet embedded, or linked under `stylesdir` with `linkcss`.
pub(super) fn head(
    state: &RefCell<RougeState>,
    linkcss: bool,
    attrs: &Attributes,
) -> Option<String> {
    let state = state.borrow();
    if !state.requires_stylesheet {
        return None;
    }
    let theme = state
        .style
        .unwrap_or_else(|| ThemeData::find(DEFAULT_THEME).expect("default rouge theme"));
    Some(if linkcss {
        let href = adoctra_core::normalize_web_path(
            &stylesheet_name(theme),
            Some(attrs.get("stylesdir").unwrap_or("")),
            false,
        );
        format!(
            "<link rel=\"stylesheet\" href=\"{}\">\n",
            super::attr_value(&href)
        )
    } else {
        format!("<style>\n{}\n</style>\n", theme.stylesheet)
    })
}

/// `stylesheet_basename`.
fn stylesheet_name(theme: &ThemeData) -> String {
    format!("rouge-{}.css", theme.name)
}

/// The stylesheet the CLI copies with `linkcss` and `copycss`
/// (`write_stylesheet`): its file name and contents.
pub(super) fn stylesheet_file(state: &RefCell<RougeState>) -> Option<(String, &'static str)> {
    let state = state.borrow();
    if !state.requires_stylesheet {
        return None;
    }
    let theme = state
        .style
        .unwrap_or_else(|| ThemeData::find(DEFAULT_THEME).expect("default rouge theme"));
    Some((stylesheet_name(theme), theme.stylesheet))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_lines_like_ruby() {
        let source = "a\nb\nc\nd\ne";
        assert_eq!(
            resolve_lines_to_highlight(source, "1-3, !2, 5", None),
            vec![1, 3, 5]
        );
        assert_eq!(
            resolve_lines_to_highlight(source, "2..;!4", None),
            vec![2, 3, 5]
        );
        assert_eq!(
            resolve_lines_to_highlight(source, "3,1,3", Some(2)),
            vec![0, 2]
        );
        assert_eq!(
            resolve_lines_to_highlight(source, "4-x", None),
            Vec::<i64>::new()
        );
    }

    #[test]
    fn ruby_integer_parsing() {
        assert_eq!(ruby_to_i(" 12abc"), 12);
        assert_eq!(ruby_to_i("-3"), -3);
        assert_eq!(ruby_to_i("1_000"), 1000);
        assert_eq!(ruby_to_i("x"), 0);
    }

    #[test]
    fn decodes_lexer_options() {
        let options = decode_www_form("token=Name+Builtin&a=%41&a=b&flag");
        assert_eq!(
            options.get("token"),
            Some(&OptionValue::Str("Name Builtin".into()))
        );
        assert_eq!(
            options.get("a"),
            Some(&OptionValue::List(vec!["A".into(), "b".into()]))
        );
        assert_eq!(options.get("flag"), Some(&OptionValue::Str(String::new())));
    }
}

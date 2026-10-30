//! Rouge's HTML formatters (`Rouge::Formatters::HTML`/`HTMLInline`) and the
//! two the Asciidoctor adapter wraps them in (`RougeExt::Formatters`).

use crate::theme::ThemeData;
use crate::token::{Token, ERROR, ESCAPE, TEXT};

/// How a token renders.
#[derive(Clone, Copy, Debug)]
pub enum Style {
    /// `<span class="shortname">` (`css_mode: :class`).
    Classes,
    /// `<span style="rules">` from a theme (`css_mode: :inline`).
    Inline(&'static ThemeData),
}

fn escape(value: &str, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

/// One token; `Escape` tokens are `Error` (Rouge's escapes are disabled).
fn span(style: Style, token: Token, value: &str, out: &mut String) {
    let token = if token == ESCAPE { ERROR } else { token };
    if token == TEXT {
        escape(value, out);
        return;
    }
    match style {
        Style::Classes => {
            out.push_str("<span class=\"");
            out.push_str(token.shortname());
            out.push_str("\">");
        }
        Style::Inline(theme) => {
            out.push_str("<span style=\"");
            out.push_str(theme.inline_style(token));
            out.push_str("\">");
        }
    }
    escape(value, out);
    out.push_str("</span>");
}

/// `Formatter#format` of the HTML formatters.
pub fn format(tokens: &[(Token, String)], style: Style) -> String {
    let mut out = String::new();
    for (token, value) in tokens {
        span(style, *token, value, &mut out);
    }
    out
}

/// `Formatter#token_lines`: tokens split at each LF; a final line without
/// tokens is not yielded.
fn token_lines(tokens: &[(Token, String)]) -> Vec<Vec<(Token, String)>> {
    let mut lines = Vec::new();
    let mut line = Vec::new();
    for (token, value) in tokens {
        let mut rest = value.as_str();
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('\n') {
                lines.push(std::mem::take(&mut line));
                rest = after;
            } else {
                let end = rest.find('\n').unwrap_or(rest.len());
                line.push((*token, rest[..end].to_string()));
                rest = &rest[end..];
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Asciidoctor's `HTMLLineHighlighter`: each line ends with a LF, and the
/// 1-based `lines` are wrapped in `<span class="hll">`.
pub fn format_highlighted_lines(tokens: &[(Token, String)], style: Style, lines: &[i64]) -> String {
    let mut out = String::new();
    for (index, line) in token_lines(tokens).iter().enumerate() {
        let highlighted = lines.contains(&(index as i64 + 1));
        if highlighted {
            out.push_str("<span class=\"hll\">");
        }
        out.push_str(&format(line, style));
        out.push('\n');
        if highlighted {
            out.push_str("</span>");
        }
    }
    out
}

/// Asciidoctor's `HTMLTable`: the formatted code beside right-aligned line
/// numbers starting at `start`.
pub fn line_table(formatted: &str, start: i64) -> String {
    let mut code = formatted.to_string();
    if !code.ends_with('\n') && !code.ends_with("\n</span>") {
        code.push('\n');
    }
    let count = code.matches('\n').count() as i64;
    let last = start + count - 1;
    let width = digits(last);
    let mut numbers = String::new();
    for number in start..=last {
        numbers.push_str(&format!("{number:>width$}\n"));
    }
    format!(
        "<table class=\"linenotable\"><tbody><tr><td class=\"linenos gl\"><pre class=\"lineno\">{numbers}</pre></td><td class=\"code\"><pre>{code}</pre></td></tr></tbody></table>"
    )
}

/// `(Math.log10 n).floor + 1` for Ruby's `%Ni` format width.
fn digits(n: i64) -> usize {
    if n <= 0 {
        // log10 of zero or a negative number is -Infinity/NaN in Ruby;
        // the formatter is only reached with a start of at least 1.
        return 1;
    }
    n.to_string().len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::{KEYWORD, NAME_FUNCTION};

    fn tokens() -> Vec<(Token, String)> {
        vec![
            (KEYWORD, "def".into()),
            (TEXT, " <".into()),
            (NAME_FUNCTION, "f\nx".into()),
            (ESCAPE, "&".into()),
        ]
    }

    #[test]
    fn formats_classes_and_escapes() {
        assert_eq!(
            format(&tokens(), Style::Classes),
            "<span class=\"k\">def</span> &lt;<span class=\"nf\">f\nx</span><span class=\"err\">&amp;</span>"
        );
    }

    #[test]
    fn formats_inline_styles() {
        let theme = ThemeData::find("github").unwrap();
        let out = format(&tokens()[..1], Style::Inline(theme));
        assert_eq!(out, "<span style=\"color: #cf222e\">def</span>");
    }

    #[test]
    fn highlights_lines() {
        let out = format_highlighted_lines(&tokens(), Style::Classes, &[2]);
        assert_eq!(
            out,
            "<span class=\"k\">def</span> &lt;<span class=\"nf\">f</span>\n<span class=\"hll\"><span class=\"nf\">x</span><span class=\"err\">&amp;</span>\n</span>"
        );
    }

    #[test]
    fn numbers_lines() {
        let out = line_table("a\nb", 9);
        assert_eq!(
            out,
            "<table class=\"linenotable\"><tbody><tr><td class=\"linenos gl\"><pre class=\"lineno\"> 9\n10\n</pre></td><td class=\"code\"><pre>a\nb\n</pre></td></tr></tbody></table>"
        );
    }
}

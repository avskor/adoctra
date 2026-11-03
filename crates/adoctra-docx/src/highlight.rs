//! Rouge highlighting of source listings (`source-highlighter=rouge`):
//! the shared adapter's tokens (`adoctra_highlight::adapter`) drawn with the
//! theme's rules (`rouge-style`, default `github`) as run properties.
//!
//! Like Ruby's adapter, callout marks are taken out of the source before
//! lexing and put back at the end of their lines; the lexer sees the body
//! with its blank edge lines, which are dropped from the output; with
//! `linenums` (numbered from `start`) the leading ones stay, as in Ruby's
//! line table. Ruby's `hll` class for `highlight` lines
//! has no style in its stylesheets, so those lines look the same.

use adoctra_core::ast::{BlankEdges, Block, Callout};
use adoctra_highlight::adapter;
use adoctra_highlight::theme::{ThemeData, DEFAULT_THEME};

use crate::xml::escape_text;

/// One output line: its runs and the callout marks after it.
pub struct Line<'a> {
    pub number: Option<i64>,
    pub runs: String,
    pub marks: Vec<&'a Callout>,
}

/// A highlighted listing.
pub struct Highlighted<'a> {
    pub lines: Vec<Line<'a>>,
    /// The theme's background (`w:shd` fill), if it has one.
    pub background: Option<String>,
    /// The width of the widest line number, for right alignment.
    pub number_width: usize,
}

/// Whether `block` is highlighted by Rouge.
pub fn applies(block: &Block) -> bool {
    block.attrs.style.as_deref() == Some("source")
        && block.conversion_attributes().get("source-highlighter") == Some("rouge")
}

/// Highlight a listing's `text` (trimmed, with its callout marks).
pub fn highlight<'a>(
    block: &Block,
    text: &str,
    blank_edges: BlankEdges,
    callouts: &'a [Callout],
) -> Highlighted<'a> {
    let attrs = block.conversion_attributes();
    let theme = attrs
        .get("rouge-style")
        .and_then(ThemeData::find)
        .or_else(|| ThemeData::find(DEFAULT_THEME))
        .expect("default rouge theme");
    let base = theme.base_style.map(css).unwrap_or_default();
    // The source without its marks; marks by trimmed line.
    let mut source = String::new();
    let mut marks: Vec<(usize, &Callout)> = Vec::new();
    let mut copied = 0;
    for callout in callouts {
        source.push_str(&text[copied..callout.start]);
        marks.push((text[..callout.start].matches('\n').count(), callout));
        copied = callout.end;
    }
    source.push_str(&text[copied..]);
    let named = &block.attrs.named;
    let mixed =
        block.attrs.options.iter().any(|o| o == "mixed") || named.contains_key("mixed-option");
    let lang = named.get("language").map(String::as_str);
    let start = named.get("linenums").map(|_| {
        named
            .get("start")
            .map_or(1, |s| adapter::ruby_to_i(s))
            .max(1)
    });
    let Some(full) = blank_edges.source(&source) else {
        return Highlighted {
            lines: Vec::new(),
            background: base.background,
            number_width: 0,
        };
    };
    let tokens = adapter::lex(lang, mixed, &full);
    // Runs per line of the untrimmed body.
    let mut lines: Vec<String> = vec![String::new()];
    for (token, value) in &tokens {
        let props = run_props(&css(theme.inline_style(*token)), &base);
        for (i, piece) in value.split('\n').enumerate() {
            if i > 0 {
                lines.push(String::new());
            }
            if !piece.is_empty() {
                lines
                    .last_mut()
                    .expect("a line")
                    .push_str(&run(&props, piece));
            }
        }
    }
    // Ruby trims blank edge lines after highlighting, except in its line
    // number table, where the leading ones stay numbered.
    let (leading, trailing) = match start {
        Some(_) => (0, 0),
        None => (blank_edges.leading, blank_edges.trailing),
    };
    let visible = full.split('\n').count().saturating_sub(leading + trailing);
    let mut lines: Vec<String> = lines
        .into_iter()
        .skip(leading)
        .take(visible.max(1))
        .collect();
    // Rouge's line table splits like Ruby's `String#split`: trailing blank
    // lines have no number (one line stays).
    if start.is_some() {
        while lines.len() > 1 && lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
    }
    let mut out: Vec<Line<'a>> = lines
        .into_iter()
        .enumerate()
        .map(|(i, runs)| Line {
            number: start.map(|s| s + i as i64),
            runs,
            marks: Vec::new(),
        })
        .collect();
    // Marks are counted in the trimmed text.
    let shift = blank_edges.leading - leading;
    for (line, callout) in marks {
        if let Some(target) = out.get_mut(line + shift) {
            target.marks.push(callout);
        }
    }
    let number_width = out
        .iter()
        .filter_map(|l| l.number)
        .map(|n| n.to_string().len())
        .max()
        .unwrap_or(0);
    Highlighted {
        lines: out,
        background: base.background,
        number_width,
    }
}

/// Run formatting from a theme's CSS rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Css {
    pub color: Option<String>,
    pub background: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// Parse a theme's rules (`color: #d14; font-weight: bold`).
pub fn css(rules: &str) -> Css {
    let mut out = Css::default();
    for rule in rules.split(';') {
        let Some((property, value)) = rule.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match property.trim() {
            "color" => out.color = hex(value),
            "background-color" | "background" => out.background = hex(value),
            "font-weight" => {
                out.bold = value == "bold" || value.parse::<u32>().is_ok_and(|w| w >= 600)
            }
            "font-style" => out.italic = value == "italic",
            "text-decoration" => out.underline = value.contains("underline"),
            _ => {}
        }
    }
    out
}

fn hex(value: &str) -> Option<String> {
    let hex = value.strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match hex.len() {
        3 => Some(
            hex.chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_uppercase(),
        ),
        6 => Some(hex.to_uppercase()),
        _ => None,
    }
}

/// `w:rPr` content of a token (the base color when the token has none; a
/// token background differing from the base).
fn run_props(token: &Css, base: &Css) -> String {
    let mut props = String::new();
    if token.bold {
        props.push_str("<w:b/><w:bCs/>");
    }
    if token.italic {
        props.push_str("<w:i/><w:iCs/>");
    }
    if let Some(color) = token.color.as_ref().or(base.color.as_ref()) {
        props.push_str(&format!(r#"<w:color w:val="{color}"/>"#));
    }
    if token.underline {
        props.push_str(r#"<w:u w:val="single"/>"#);
    }
    if let Some(fill) = token
        .background
        .as_ref()
        .filter(|b| Some(*b) != base.background.as_ref())
    {
        props.push_str(&format!(
            r#"<w:shd w:val="clear" w:color="auto" w:fill="{fill}"/>"#
        ));
    }
    props
}

fn run(props: &str, text: &str) -> String {
    let mut out = String::from("<w:r>");
    if !props.is_empty() {
        out.push_str(&format!("<w:rPr>{props}</w:rPr>"));
    }
    for (i, piece) in text.split('\t').enumerate() {
        if i > 0 {
            out.push_str("<w:tab/>");
        }
        if !piece.is_empty() {
            out.push_str(r#"<w:t xml:space="preserve">"#);
            out.push_str(&escape_text(piece));
            out.push_str("</w:t>");
        }
    }
    out.push_str("</w:r>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theme_rules() {
        assert_eq!(
            css("color: #d14; font-weight: bold;font-style: italic"),
            Css {
                color: Some("DD1144".into()),
                bold: true,
                italic: true,
                ..Css::default()
            }
        );
        assert_eq!(
            css("background-color: #272822").background.as_deref(),
            Some("272822")
        );
        assert_eq!(css("color: inherit"), Css::default());
        assert!(css("text-decoration: underline").underline);
    }

    #[test]
    fn tokens_take_the_base_color_and_a_distinct_background() {
        let base = css("color: #f8f8f2;background-color: #272822");
        assert_eq!(
            run_props(&Css::default(), &base),
            r#"<w:color w:val="F8F8F2"/>"#
        );
        let token = css("color: #f92672; background-color: #272822; font-weight: bold");
        assert_eq!(
            run_props(&token, &base),
            r#"<w:b/><w:bCs/><w:color w:val="F92672"/>"#
        );
        assert_eq!(
            run("", "a\tb<"),
            r#"<w:r><w:t xml:space="preserve">a</w:t><w:tab/><w:t xml:space="preserve">b&lt;</w:t></w:r>"#
        );
    }
}

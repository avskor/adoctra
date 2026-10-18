//! Math delimiters and typesetter configuration from the pinned HTML5 converter.
use adoctra_core::{Attributes, StemStyle};

pub(super) fn equation(style: StemStyle, source: &str, out: &mut String) {
    let (open, close) = match style {
        StemStyle::AsciiMath => (r"\$", r"\$"),
        StemStyle::LatexMath => (r"\[", r"\]"),
    };
    let equation = if style == StemStyle::AsciiMath {
        asciimath_breaks(source)
    } else {
        source.to_owned()
    };
    let wrapped = equation.starts_with(open) && equation.ends_with(close);
    if !wrapped {
        out.push_str(open);
    }
    out.push_str(&equation);
    if !wrapped {
        out.push_str(close);
    }
}

/// Ruby's StemBreakRx: spaces followed by backslash/newline (and further
/// blank or backslash-only lines), or two or more consecutive newlines.
fn asciimath_breaks(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::new();
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        let mut end = at;
        while bytes.get(end) == Some(&b' ') {
            end += 1;
        }
        let slashed = bytes.get(end..end + 2) == Some(b"\\\n");
        let mut lines = 0;
        if slashed {
            end += 2;
            lines = 1;
            loop {
                if bytes.get(end) == Some(&b'\n') {
                    end += 1;
                } else if bytes.get(end..end + 2) == Some(b"\\\n") {
                    end += 2;
                } else {
                    break;
                }
                lines += 1;
            }
        } else if bytes.get(at..at + 2) == Some(b"\n\n") {
            end = at;
            while bytes.get(end) == Some(&b'\n') {
                end += 1;
                lines += 1;
            }
        }
        if lines > 0 {
            out.push_str(&source[copied..at]);
            out.push_str(r"\$");
            for _ in 1..lines {
                out.push_str("\n<br>");
            }
            out.push_str("\n\\$");
            copied = end;
            at = end;
        } else {
            at = end.max(at + 1);
        }
    }
    out.push_str(&source[copied..]);
    out
}

pub(super) fn configuration(attrs: &Attributes, header: &Attributes, out: &mut String) {
    if attrs.get("stem").is_none() {
        return;
    }
    let eqnums = match attrs.get("eqnums") {
        None => "none",
        Some("") => "AMS",
        Some(value) => value,
    };
    // A JSON string in a script needs '<' escaped as well: attribute values
    // must not terminate the script element or become JavaScript source.
    let mut number = String::new();
    adoctra_core::json::escape_into(eqnums, &mut number);
    let number = number
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let (before, after) = include_str!("../assets/mathjax-config.js.html")
        .split_once("__EQNUMS__")
        .expect("equation numbering placeholder");
    out.push_str(before);
    out.push_str(&number);
    out.push_str(after);
    out.push_str("<script src=\"");
    let scheme = header.get("asset-uri-scheme").unwrap_or("https");
    if !scheme.is_empty() {
        out.push_str(&super::attr_value(scheme));
        out.push(':');
    }
    out.push_str("//cdnjs.cloudflare.com/ajax/libs/mathjax/2.7.9/MathJax.js?config=TeX-MML-AM_HTMLorMML\"></script>\n");
}

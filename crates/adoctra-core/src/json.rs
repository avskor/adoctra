//! Minimal JSON string helper, shared so the JSON backend and the WASM editor
//! layer don't keep two byte-for-byte copies of the same escaper.
//!
//! This is a plain string utility — *not* a backend. It lives in core because
//! `adoctra-json` and `adoctra-wasm` (which both emit JSON and both depend only
//! on core) are the two callers, and core is their only shared home.

/// Append `value` to `out` as a quoted, escaped JSON string (including the
/// surrounding `"`).
pub fn escape_into(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Convenience wrapper returning a freshly allocated escaped string.
pub fn escape(value: &str) -> String {
    let mut s = String::new();
    escape_into(value, &mut s);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_quotes_and_controls() {
        assert_eq!(escape("a\"b\\c\n"), r#""a\"b\\c\n""#);
    }

    #[test]
    fn escapes_low_control_chars_as_unicode() {
        // input is a single U+0001; output is the 8-char JSON string ""
        assert_eq!(escape("\u{1}"), "\"\\u0001\"");
    }
}

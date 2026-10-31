//! Ruby's `String#strip`/`lstrip`/`rstrip` whitespace: ASCII whitespace and
//! NUL only. Rust's `trim*` also removes Unicode spaces (U+00A0, U+2003, …),
//! which Ruby keeps: a line of no-break spaces is not blank, and a trailing
//! one stays in the text.

/// One of Ruby's strip characters.
pub(crate) fn is_ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | '\0')
}

pub(crate) trait RubyStrip {
    fn ruby_strip(&self) -> &str;
    fn ruby_lstrip(&self) -> &str;
    fn ruby_rstrip(&self) -> &str;
    /// Blank as Ruby's reader sees a line: empty after `rstrip`.
    fn is_ruby_blank(&self) -> bool;
}

impl RubyStrip for str {
    fn ruby_strip(&self) -> &str {
        self.trim_matches(is_ruby_space)
    }

    fn ruby_lstrip(&self) -> &str {
        self.trim_start_matches(is_ruby_space)
    }

    fn ruby_rstrip(&self) -> &str {
        self.trim_end_matches(is_ruby_space)
    }

    fn is_ruby_blank(&self) -> bool {
        self.chars().all(is_ruby_space)
    }
}

#[cfg(test)]
mod tests {
    use super::RubyStrip;

    #[test]
    fn strips_ascii_whitespace_and_nul_but_not_unicode_spaces() {
        assert_eq!(" \t\0a b\r\n\u{b}\u{c}".ruby_strip(), "a b");
        assert_eq!("\u{a0}a\u{2003}".ruby_strip(), "\u{a0}a\u{2003}");
        assert_eq!("  a ".ruby_lstrip(), "a ");
        assert_eq!("  a \u{a0} ".ruby_rstrip(), "  a \u{a0}");
        assert!(" \t\r".is_ruby_blank());
        assert!("".is_ruby_blank());
        assert!(!"\u{a0}".is_ruby_blank());
        assert!(!"\u{3000}".is_ruby_blank());
    }
}

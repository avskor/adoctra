//! Typographic replacements (asciidoctor's `REPLACEMENTS`), applied once in
//! core to resolved inline text.
//!
//! These are *semantic* substitutions: `(C)` means the copyright sign, `--`
//! (spaced) means an em dash, `...` means an ellipsis, and so on. Following the
//! model's core/backend split, [`apply`] resolves each construct to its real
//! Unicode character and stores that in the `Inline::Text` value, leaving the
//! span pointing at the original source. *How* a character is encoded for a
//! given output (a backend may emit `&#8212;` for `—`, or the literal char)
//! stays a backend concern — see the HTML backend's text encoder.
//!
//! Behaviour is pinned against asciidoctor 2.0.23. A single left-to-right pass
//! recognises, in precedence order:
//!
//! - a leading `\` escapes the following construct (it renders literally, the
//!   backslash is dropped): `\(C)` -> `(C)`, `\--` -> `--`, `\'` -> `'`;
//! - `(C)` / `(R)` / `(TM)` -> `©` / `®` / `™`;
//! - `--` between blanks (or at a line boundary) -> thin space + em dash + thin
//!   space, consuming one adjacent blank on each side; `word--word` (no spaces)
//!   -> em dash + zero-width space;
//! - `...` -> ellipsis + zero-width space (only the first three dots);
//! - `'` directly between two word characters -> a right single quote (the
//!   typographic apostrophe), e.g. `it's`;
//! - the arrows `->` `=>` `<-` `<=` -> `→` `⇒` `←` `⇐`, where a right-pointing
//!   arrow wins over a left one (so `<=>` is `<` + `⇒`, not `⇐` + `>`).

use std::borrow::Cow;

// Resolved characters (asciidoctor emits the matching numeric entity).
const COPY: char = '\u{a9}'; // © &#169;
const REG: char = '\u{ae}'; // ® &#174;
const TRADE: char = '\u{2122}'; // ™ &#8482;
const THIN: char = '\u{2009}'; // thin space &#8201;
const EMDASH: char = '\u{2014}'; // — &#8212;
const ZWSP: char = '\u{200b}'; // zero-width space &#8203;
const ELLIPSIS: char = '\u{2026}'; // … &#8230;
const RSQUO: char = '\u{2019}'; // ’ &#8217;
const RARROW: char = '\u{2192}'; // → &#8594;
const RDARROW: char = '\u{21d2}'; // ⇒ &#8658;
const LARROW: char = '\u{2190}'; // ← &#8592;
const LDARROW: char = '\u{21d0}'; // ⇐ &#8656;

/// Apply typographic replacements to a run of resolved text. Returns the input
/// untouched (borrowed) when it holds none of the trigger characters, so the
/// common case allocates nothing.
pub(crate) fn apply(src: &str) -> Cow<'_, str> {
    if !src.bytes().any(is_trigger) {
        return Cow::Borrowed(src);
    }
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            // A backslash escapes the construct that follows it.
            b'\\' => {
                if let Some(len) = escapable_at(src, i + 1) {
                    out.push_str(&src[i + 1..i + 1 + len]);
                    i += 1 + len;
                } else {
                    out.push('\\');
                    i += 1;
                }
            }
            // (C) (R) (TM)
            b'(' => {
                if let Some((ch, len)) = symbol_at(src, i) {
                    out.push(ch);
                    i += len;
                } else {
                    out.push('(');
                    i += 1;
                }
            }
            // `--` (em dash, two forms) or `->` (arrow).
            b'-' => {
                if bytes.get(i + 1) == Some(&b'-') {
                    i = take_dash(src, i, &mut out);
                } else if bytes.get(i + 1) == Some(&b'>') {
                    out.push(RARROW);
                    i += 2;
                } else {
                    out.push('-');
                    i += 1;
                }
            }
            // `...` ellipsis (first three dots only).
            b'.' => {
                if src[i..].starts_with("...") {
                    out.push(ELLIPSIS);
                    out.push(ZWSP);
                    i += 3;
                } else {
                    out.push('.');
                    i += 1;
                }
            }
            // `'` apostrophe, only between two word characters.
            b'\'' => {
                if prev_is_word(bytes, i) && next_is_word(bytes, i + 1) {
                    out.push(RSQUO);
                } else {
                    out.push('\'');
                }
                i += 1;
            }
            // `=>` arrow.
            b'=' => {
                if bytes.get(i + 1) == Some(&b'>') {
                    out.push(RDARROW);
                    i += 2;
                } else {
                    out.push('=');
                    i += 1;
                }
            }
            // `<-` / `<=` arrows — but a following `>` means the right arrow
            // (`->` / `=>`) wins, so emit `<` and let it match next.
            b'<' => match bytes.get(i + 1) {
                Some(&b'-') | Some(&b'=') if bytes.get(i + 2) == Some(&b'>') => {
                    out.push('<');
                    i += 1;
                }
                Some(&b'-') => {
                    out.push(LARROW);
                    i += 2;
                }
                Some(&b'=') => {
                    out.push(LDARROW);
                    i += 2;
                }
                _ => {
                    out.push('<');
                    i += 1;
                }
            },
            _ => {
                let step = utf8_step(bytes, i);
                out.push_str(&src[i..i + step]);
                i += step;
            }
        }
    }
    Cow::Owned(out)
}

/// Bytes that can begin (or escape) a replacement; the fast path skips text
/// holding none of them.
fn is_trigger(b: u8) -> bool {
    matches!(b, b'(' | b'-' | b'.' | b'\'' | b'<' | b'=' | b'\\')
}

/// If `src[at..]` starts with `(C)`/`(R)`/`(TM)`, the resolved char and its
/// source length.
fn symbol_at(src: &str, at: usize) -> Option<(char, usize)> {
    let rest = &src[at..];
    if rest.starts_with("(C)") {
        Some((COPY, 3))
    } else if rest.starts_with("(R)") {
        Some((REG, 3))
    } else if rest.starts_with("(TM)") {
        Some((TRADE, 4))
    } else {
        None
    }
}

/// Length of an escapable construct starting at `at`, or `None` for a stray
/// backslash. Mirrors what [`apply`] otherwise replaces, plus the apostrophe.
fn escapable_at(src: &str, at: usize) -> Option<usize> {
    if at >= src.len() {
        return None;
    }
    if let Some((_, len)) = symbol_at(src, at) {
        return Some(len);
    }
    let rest = &src[at..];
    for pat in ["--", "...", "->", "=>", "<-", "<=", "'"] {
        if rest.starts_with(pat) {
            return Some(pat.len());
        }
    }
    None
}

/// Handle a `--` at `i`. The spaced form (blank or line boundary on both sides)
/// becomes thin + em dash + thin, consuming one adjacent blank per side; the
/// word form (`\w--\w`) becomes em dash + zero-width space. Anything else stays
/// literal. Returns the index just past what was consumed.
fn take_dash(src: &str, i: usize, out: &mut String) -> usize {
    let bytes = src.as_bytes();
    // `---` (a third dash) is never an em dash.
    if bytes.get(i + 2) == Some(&b'-') {
        out.push_str("--");
        return i + 2;
    }
    if prev_is_word(bytes, i) && next_is_word(bytes, i + 2) {
        out.push(EMDASH);
        out.push(ZWSP);
        return i + 2;
    }
    let before_ok = i == 0 || is_blank(bytes[i - 1]);
    let after = bytes.get(i + 2).copied();
    let after_ok = after.is_none() || is_blank(after.unwrap());
    if before_ok && after_ok {
        // Replace the single emitted leading blank (if any) with a thin space.
        if out.ends_with(' ') || out.ends_with('\t') {
            out.pop();
        }
        out.push(THIN);
        out.push(EMDASH);
        out.push(THIN);
        // Consume one trailing blank too.
        return if after.is_some_and(is_blank) {
            i + 3
        } else {
            i + 2
        };
    }
    out.push_str("--");
    i + 2
}

fn is_blank(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn prev_is_word(bytes: &[u8], i: usize) -> bool {
    i > 0 && is_word(bytes[i - 1])
}

fn next_is_word(bytes: &[u8], j: usize) -> bool {
    bytes.get(j).is_some_and(|&b| is_word(b))
}

/// Byte width of the UTF-8 char starting at `i` (at least 1).
fn utf8_step(bytes: &[u8], i: usize) -> usize {
    let b = bytes[i];
    if b < 0x80 {
        1
    } else if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else if b >= 0xC0 {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        apply(s).into_owned()
    }

    #[test]
    fn plain_text_is_borrowed_unchanged() {
        assert!(matches!(apply("nothing to do here"), Cow::Borrowed(_)));
    }

    #[test]
    fn registered_symbols() {
        assert_eq!(r("(C) (R) (TM)"), "\u{a9} \u{ae} \u{2122}");
        // adjacency
        assert_eq!(r("(C)(R)(TM)"), "\u{a9}\u{ae}\u{2122}");
    }

    #[test]
    fn spaced_em_dash_consumes_one_blank_each_side() {
        assert_eq!(r("a -- b"), "a\u{2009}\u{2014}\u{2009}b");
        // extra spaces are preserved beyond the one consumed
        assert_eq!(r("a  --  b"), "a \u{2009}\u{2014}\u{2009} b");
    }

    #[test]
    fn spaced_em_dash_at_line_boundaries() {
        assert_eq!(r("-- start"), "\u{2009}\u{2014}\u{2009}start");
        assert_eq!(r("end --"), "end\u{2009}\u{2014}\u{2009}");
    }

    #[test]
    fn word_em_dash_keeps_words_and_adds_zwsp() {
        assert_eq!(r("word--word"), "word\u{2014}\u{200b}word");
    }

    #[test]
    fn dash_without_a_full_boundary_stays_literal() {
        assert_eq!(r("a-- b"), "a-- b");
        assert_eq!(r("a --b"), "a --b");
        assert_eq!(r("three --- dashes"), "three --- dashes");
    }

    #[test]
    fn ellipsis_takes_three_dots_only() {
        assert_eq!(r("Wait..."), "Wait\u{2026}\u{200b}");
        assert_eq!(r("dots.... four"), "dots\u{2026}\u{200b}. four");
        assert_eq!(r("two.. dots"), "two.. dots");
    }

    #[test]
    fn apostrophe_only_between_word_chars() {
        assert_eq!(r("it's"), "it\u{2019}s");
        // not between two word chars -> literal
        assert_eq!(r("rock 'n' roll"), "rock 'n' roll");
        assert_eq!(r("kids' toys"), "kids' toys");
    }

    #[test]
    fn arrows() {
        assert_eq!(r("-> => <- <="), "\u{2192} \u{21d2} \u{2190} \u{21d0}");
        // no boundary needed
        assert_eq!(r("a->b"), "a\u{2192}b");
    }

    #[test]
    fn right_arrow_wins_over_left_in_ambiguous_runs() {
        assert_eq!(r("<=>"), "<\u{21d2}");
        assert_eq!(r("<->"), "<\u{2192}");
    }

    #[test]
    fn backslash_escapes_each_construct() {
        assert_eq!(r("\\(C)"), "(C)");
        assert_eq!(r("\\--"), "--");
        assert_eq!(r("\\..."), "...");
        assert_eq!(r("\\->"), "->");
        assert_eq!(r("it\\'s"), "it's");
        // a stray backslash survives
        assert_eq!(r("a\\b"), "a\\b");
    }

    #[test]
    fn replacements_compose_in_one_pass() {
        assert_eq!(
            r("(C) 2026 -- it's a test..."),
            "\u{a9} 2026\u{2009}\u{2014}\u{2009}it\u{2019}s a test\u{2026}\u{200b}"
        );
    }

    #[test]
    fn non_ascii_text_passes_through() {
        // a trigger byte forces the slow path; multibyte chars must survive it
        assert_eq!(r("café -- thé"), "café\u{2009}\u{2014}\u{2009}thé");
    }
}

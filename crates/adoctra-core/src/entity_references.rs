//! The entity-restoration replacement inside a resolved attribute value.
//! Work on the reference's lexical view, then preserve Text/Raw boundaries;
//! document attributes and source coordinates are never rewritten here.
use crate::attributes::push_part;
use crate::AttributePart;
use std::ops::Range;

struct Fragment {
    range: Range<usize>,
    raw: bool,
    protected: Option<char>,
}

struct LexicalValue {
    text: String,
    fragments: Vec<Fragment>,
}

impl LexicalValue {
    fn new(parts: &[AttributePart]) -> Self {
        let mut value = Self {
            text: String::new(),
            fragments: Vec::new(),
        };
        for part in parts {
            if part.is_raw() {
                value.append(part.text(), true, None);
                continue;
            }
            let mut start = 0;
            for (offset, ch) in part.text().char_indices() {
                let encoded = match ch {
                    '&' => "&amp;",
                    '<' => "&lt;",
                    '>' => "&gt;",
                    _ => continue,
                };
                value.append(&part.text()[start..offset], false, None);
                value.append(encoded, false, Some(ch));
                start = offset + ch.len_utf8();
            }
            value.append(&part.text()[start..], false, None);
        }
        value
    }

    fn append(&mut self, text: &str, raw: bool, protected: Option<char>) {
        if text.is_empty() {
            return;
        }
        let start = self.text.len();
        self.text.push_str(text);
        self.fragments.push(Fragment {
            range: start..self.text.len(),
            raw,
            protected,
        });
    }

    fn copy(&self, range: Range<usize>, cursor: &mut usize, output: &mut Vec<AttributePart>) {
        while *cursor < self.fragments.len() {
            let fragment = &self.fragments[*cursor];
            if fragment.range.start >= range.end {
                break;
            }
            if fragment.range.end <= range.start {
                *cursor += 1;
                continue;
            }
            let start = range.start.max(fragment.range.start);
            let end = range.end.min(fragment.range.end);
            if let Some(ch) = fragment.protected {
                // Match boundaries never bisect a generated special-character
                // reference: the matcher consumes an entire &amp; prefix.
                debug_assert_eq!(start..end, fragment.range);
                push_part(output, &ch.to_string(), false);
            } else {
                push_part(output, &self.text[start..end], fragment.raw);
            }
            if end < fragment.range.end {
                break;
            }
            *cursor += 1;
        }
    }
}

/// None means no entity-restoration match; callers can keep their original value.
/// Restored entity syntax is raw content, not an already HTML-escaped Text value.
pub(crate) fn restore(value: &str, parts: &[AttributePart]) -> Option<Vec<AttributePart>> {
    if !value.contains('&') {
        return None;
    }
    let literal;
    let parts = if parts.is_empty() {
        literal = [AttributePart::Text(value.to_owned())];
        &literal[..]
    } else {
        parts
    };
    let lexical = LexicalValue::new(parts);
    let bytes = lexical.text.as_bytes();
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut copied = 0;
    let mut offset = 0;
    let mut changed = false;
    while offset < bytes.len() {
        let escaped = bytes[offset] == b'\\';
        let amp = offset + usize::from(escaped);
        if let Some(end) = entity_end(bytes, amp) {
            lexical.copy(copied..offset, &mut cursor, &mut output);
            if escaped {
                lexical.copy(amp..end, &mut cursor, &mut output);
            } else {
                push_part(&mut output, "&", true);
                push_part(&mut output, &lexical.text[amp + 5..end], true);
            }
            changed = true;
            copied = end;
            offset = end;
        } else {
            // Matching uses ASCII bytes; copy ranges are only taken at ASCII
            // match boundaries, so Unicode text remains intact.
            offset += 1;
        }
    }
    if !changed {
        return None;
    }
    lexical.copy(copied..bytes.len(), &mut cursor, &mut output);
    Some(output)
}

/// Asciidoctor 2.0.23 REPLACEMENTS entity grammar, not an entity-name registry:
/// >=2 ASCII letters + 0..2 digits, # + 2..6 digits, #x + 2..5 hex digits.
fn entity_end(bytes: &[u8], start: usize) -> Option<usize> {
    if !bytes.get(start..)?.starts_with(b"&amp;") {
        return None;
    }
    let mut end = start + 5;
    let (min, max, digits): (usize, usize, fn(&u8) -> bool) =
        if bytes.get(end..)?.starts_with(b"#x") {
            end += 2;
            (2, 5, u8::is_ascii_hexdigit)
        } else if bytes.get(end) == Some(&b'#') {
            end += 1;
            (2, 6, u8::is_ascii_digit)
        } else {
            let letters = end;
            while bytes.get(end).is_some_and(u8::is_ascii_alphabetic) {
                end += 1;
            }
            if end - letters < 2 {
                return None;
            }
            (0, 2, u8::is_ascii_digit)
        };
    let beginning = end;
    while bytes.get(end).is_some_and(digits) {
        end += 1;
    }
    ((min..=max).contains(&(end - beginning)) && bytes.get(end) == Some(&b';')).then_some(end + 1)
}

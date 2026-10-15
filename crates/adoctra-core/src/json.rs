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

/// Append optional literal/raw/formatting fragments to an inline JSON object.
pub fn write_attribute_parts(parts: &[crate::AttributePart], out: &mut String) {
    if parts.is_empty() {
        return;
    }
    out.push_str(",\"parts\":");
    write_parts_array(parts, out);
}

/// Optional raw/formatting boundaries for an exported attribute snapshot.
pub fn write_attribute_parts_map(attributes: &crate::Attributes, out: &mut String) {
    let mut first = true;
    for (key, _) in attributes.iter() {
        let Some(parts) = attributes.raw_parts(key) else {
            continue;
        };
        if first {
            out.push_str(",\"attribute_parts\":{");
            first = false;
        } else {
            out.push(',');
        }
        escape_into(key, out);
        out.push(':');
        write_parts_array(parts, out);
    }
    if !first {
        out.push('}');
    }
}

fn write_parts_array(parts: &[crate::AttributePart], out: &mut String) {
    out.push('[');
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if let crate::AttributePart::OpenLink(link) = part {
            out.push_str("{\"kind\":\"open-link\",\"target\":");
            escape_into(&link.target, out);
            for (key, value) in [
                ("id", &link.id),
                ("role", &link.role),
                ("title", &link.title),
                ("window", &link.window),
                ("rel", &link.rel),
            ] {
                if let Some(value) = value {
                    out.push(',');
                    escape_into(key, out);
                    out.push(':');
                    escape_into(value, out);
                }
            }
            out.push('}');
            continue;
        }
        if matches!(part, crate::AttributePart::LineBreak) {
            out.push_str("{\"kind\":\"line-break\"}");
            continue;
        }
        if matches!(part, crate::AttributePart::CloseLink) {
            out.push_str("{\"kind\":\"close-link\"}");
            continue;
        }
        if let crate::AttributePart::Open(format)
        | crate::AttributePart::Close(format)
        | crate::AttributePart::OpenWithAttributes { format, .. } = part
        {
            out.push_str(if !matches!(part, crate::AttributePart::Close(_)) {
                "{\"kind\":\"open\",\"format\":"
            } else {
                "{\"kind\":\"close\",\"format\":"
            });
            escape_into(format.name(), out);
            if let crate::AttributePart::OpenWithAttributes { id, role, .. } = part {
                if let Some(id) = id {
                    out.push_str(",\"id\":");
                    escape_into(id, out);
                }
                if let Some(role) = role {
                    out.push_str(",\"role\":");
                    escape_into(role, out);
                }
            }
            out.push('}');
            continue;
        }
        out.push_str(if part.is_raw() {
            "{\"kind\":\"raw\",\"value\":"
        } else {
            "{\"kind\":\"text\",\"value\":"
        });
        escape_into(part.text(), out);
        out.push('}');
    }
    out.push(']');
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

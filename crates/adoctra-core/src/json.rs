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
        let Some(parts) = attributes.raw_parts_exact(key) else {
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

/// The fields of an inline image or icon (without braces): lexical values,
/// the flat alt text and its parts.
/// A keyboard, button or menu macro's fields (without braces), led by
/// `"<key>":"kbd"|"button"|"menu"` (Ruby's node types): `keys` (parts per
/// key), a button's label `parts`, or a menu's `menu`, `submenus` and
/// `item` parts (`null` for a bare reference) with `font` icons.
pub fn write_ui(ui: &crate::UiMacro, key: &str, out: &mut String) {
    let list = |labels: &[Vec<crate::AttributePart>], out: &mut String| {
        out.push('[');
        for (i, label) in labels.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_parts_array(label, out);
        }
        out.push(']');
    };
    escape_into(key, out);
    match ui {
        crate::UiMacro::Keyboard { keys } => {
            out.push_str(":\"kbd\",\"keys\":");
            list(keys, out);
        }
        crate::UiMacro::Button { label } => {
            out.push_str(":\"button\",\"parts\":");
            write_parts_array(label, out);
        }
        crate::UiMacro::Menu {
            menu,
            submenus,
            item,
            font,
        } => {
            out.push_str(":\"menu\",\"menu\":");
            write_parts_array(menu, out);
            out.push_str(",\"submenus\":");
            list(submenus, out);
            out.push_str(",\"item\":");
            match item {
                Some(item) => write_parts_array(item, out),
                None => out.push_str("null"),
            }
            out.push_str(&format!(",\"font\":{font}"));
        }
    }
}

/// Inline math fields without braces, shared by JSON/ASG and editor output.
pub fn write_stem(stem: &crate::Stem, key: &str, out: &mut String) {
    escape_into(key, out);
    out.push_str(":\"stem\",\"style\":");
    escape_into(stem.style.as_str(), out);
    out.push_str(",\"parts\":");
    write_parts_array(&stem.parts, out);
}

/// An index term's fields (without braces), led by `"<key>":"indexterm"`
/// (Ruby's node type): a flow term's shown `parts` (`null` when concealed),
/// the concealed `terms`, `see` (or `null`) and `see_also`, all lexical.
/// A callout mark from custom block substitutions: its number, guard, form,
/// catalog id, `icons` mode and image icon.
pub fn write_callout(callout: &crate::InlineCallout, key: &str, out: &mut String) {
    out.push_str(&format!("\"{key}\":\"callout\",\"number\":"));
    escape_into(&callout.number, out);
    for (name, value) in [
        ("id", &callout.id),
        ("guard", &callout.guard),
        ("icons", &callout.icons),
    ] {
        out.push_str(&format!(",\"{name}\":"));
        match value {
            Some(value) => escape_into(value, out),
            None => out.push_str("null"),
        }
    }
    out.push_str(&format!(",\"xml\":{},\"src\":", callout.xml));
    escape_into(&callout.src, out);
}

pub fn write_index_term(term: &crate::IndexTerm, key: &str, out: &mut String) {
    let strings = |values: &[String], out: &mut String| {
        out.push('[');
        for (i, value) in values.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            escape_into(value, out);
        }
        out.push(']');
    };
    escape_into(key, out);
    out.push_str(":\"indexterm\",\"parts\":");
    match &term.text {
        Some(text) => write_parts_array(text, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"terms\":");
    strings(&term.terms, out);
    out.push_str(",\"see\":");
    match &term.see {
        Some(see) => escape_into(see, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"see_also\":");
    strings(&term.see_also, out);
}

/// A footnote's fields (without braces): `type` (`definition`, `reference`
/// or `unresolved`), `index`, `id` and the definition's content `parts`.
pub fn write_footnote(footnote: &crate::Footnote, out: &mut String) {
    out.push_str(match footnote.kind {
        crate::FootnoteKind::Definition => "\"type\":\"definition\",\"index\":",
        crate::FootnoteKind::Reference => "\"type\":\"reference\",\"index\":",
        crate::FootnoteKind::Unresolved => "\"type\":\"unresolved\",\"index\":",
    });
    match &footnote.index {
        Some(index) => escape_into(index, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"id\":");
    match &footnote.id {
        Some(id) => escape_into(id, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"parts\":");
    write_parts_array(&footnote.content, out);
}

/// A document's footnotes list (`"footnotes":[…]`, after a comma), omitted
/// when empty. `span` writes each definition's span field, if any.
pub fn write_footnote_definitions(
    footnotes: &[crate::ast::FootnoteDefinition],
    out: &mut String,
    span: Option<fn(&crate::Span, &mut String)>,
) {
    if footnotes.is_empty() {
        return;
    }
    out.push_str(",\"footnotes\":[");
    for (i, footnote) in footnotes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('{');
        if let Some(span_field) = span {
            span_field(&footnote.span, out);
            out.push(',');
        }
        out.push_str("\"index\":");
        escape_into(&footnote.index, out);
        out.push_str(",\"id\":");
        match &footnote.id {
            Some(id) => escape_into(id, out),
            None => out.push_str("null"),
        }
        out.push_str(",\"parts\":");
        write_parts_array(&footnote.content, out);
        out.push('}');
    }
    out.push(']');
}

pub fn write_inline_image(image: &crate::InlineImage, out: &mut String) {
    out.push_str(&format!("\"icon\":{},\"target\":", image.icon));
    escape_into(&image.target, out);
    out.push_str(",\"src\":");
    escape_into(&image.src, out);
    out.push_str(",\"alt\":");
    escape_into(&crate::attributes::parts_text(&image.alt), out);
    out.push_str(",\"icons\":");
    match &image.icons {
        Some(icons) => escape_into(icons, out),
        None => out.push_str("null"),
    }
    out.push_str(",\"attributes\":{");
    for (i, (key, value)) in image.attributes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        escape_into(key, out);
        out.push(':');
        escape_into(value, out);
    }
    out.push_str("},\"options\":[");
    for (i, option) in image.options.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        escape_into(option, out);
    }
    out.push(']');
    write_attribute_parts(&image.alt, out);
    write_svg(image.svg.as_ref(), out);
}

/// An embedded SVG (`,"svg":{…}`), omitted for an ordinary image: `inline`
/// with its `content` (`null` when unreadable), or `interactive` with its
/// `fallback` source.
pub fn write_svg(svg: Option<&crate::Svg>, out: &mut String) {
    match svg {
        None => {}
        Some(crate::Svg::Inline(content)) => {
            out.push_str(",\"svg\":{\"mode\":\"inline\",\"content\":");
            match content {
                Some(content) => escape_into(content, out),
                None => out.push_str("null"),
            }
            out.push('}');
        }
        Some(crate::Svg::Interactive { fallback }) => {
            out.push_str(",\"svg\":{\"mode\":\"interactive\",\"fallback\":");
            match fallback {
                Some(fallback) => escape_into(fallback, out),
                None => out.push_str("null"),
            }
            out.push('}');
        }
    }
}

pub fn write_parts_array(parts: &[crate::AttributePart], out: &mut String) {
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
        if let crate::AttributePart::Anchor(id) = part {
            out.push_str("{\"kind\":\"anchor\",\"id\":");
            escape_into(id, out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::Footnote(footnote) = part {
            out.push_str("{\"kind\":\"footnote\",");
            write_footnote(footnote, out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::Ui(ui) = part {
            out.push('{');
            write_ui(ui, "kind", out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::Stem(stem) = part {
            out.push('{');
            write_stem(stem, "kind", out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::Callout(callout) = part {
            out.push('{');
            write_callout(callout, "kind", out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::IndexTerm(term) = part {
            out.push('{');
            write_index_term(term, "kind", out);
            out.push('}');
            continue;
        }
        if let crate::AttributePart::Image(image) = part {
            out.push_str("{\"kind\":\"image\",");
            write_inline_image(image, out);
            out.push('}');
            continue;
        }
        if matches!(part, crate::AttributePart::CloseLink) {
            out.push_str("{\"kind\":\"close-link\"}");
            continue;
        }
        if let crate::AttributePart::OpenXref(xref) = part {
            out.push_str("{\"kind\":\"open-xref\",\"target\":");
            escape_into(&xref.target, out);
            for (key, value) in [
                ("refid", &xref.refid),
                ("path", &xref.path),
                ("fragment", &xref.fragment),
                ("role", &xref.role),
                ("window", &xref.window),
                ("rel", &xref.rel),
                ("xrefstyle", &xref.xrefstyle),
            ] {
                if let Some(value) = value {
                    out.push_str(&format!(",\"{key}\":"));
                    escape_into(value, out);
                }
            }
            out.push('}');
            continue;
        }
        if matches!(part, crate::AttributePart::CloseXref) {
            out.push_str("{\"kind\":\"close-xref\"}");
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

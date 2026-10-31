/// Asciidoctor 2.0.23 REPLACEMENTS entity grammar, not an entity-name registry:
/// >=2 ASCII letters + 0..2 digits, # + 2..6 digits, #x + 2..5 hex digits.
pub(crate) fn entity_end(bytes: &[u8], start: usize) -> Option<usize> {
    if !bytes.get(start..)?.starts_with(b"&amp;") {
        return None;
    }
    entity_name_end(bytes, start + 5)
}

/// Shared suffix grammar for restored references and section ID sanitization.
/// The caller supplies the byte immediately after the lexical ampersand.
pub(crate) fn entity_name_end(bytes: &[u8], mut end: usize) -> Option<usize> {
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

/// The end of a character reference (the pinned grammar) starting at `at`,
/// as written in HTML: `&` followed by a name or a numeric reference.
pub fn character_reference_end(text: &str, at: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    (bytes.get(at) == Some(&b'&'))
        .then(|| entity_name_end(bytes, at + 1))
        .flatten()
}

/// Remove the special-character protection of a lexical value in one pass.
/// Character references keep their spelling, and so does an `&amp;` that
/// would otherwise read as one (`&amp;#38;` is the literal text `&#38;`), so
/// a decoded field still tells references from text.
pub(crate) fn decode_protection(lexical: &str) -> String {
    let mut decoded = String::with_capacity(lexical.len());
    let mut rest = lexical;
    while let Some(at) = rest.find('&') {
        decoded.push_str(&rest[..at]);
        rest = &rest[at..];
        let (text, len) = if rest.starts_with("&lt;") {
            ("<", 4)
        } else if rest.starts_with("&gt;") {
            (">", 4)
        } else if rest.starts_with("&amp;") && entity_end(rest.as_bytes(), 0).is_none() {
            ("&", 5)
        } else {
            ("&", 1)
        };
        decoded.push_str(text);
        rest = &rest[len..];
    }
    decoded.push_str(rest);
    decoded
}

/// The parts of a decoded value (or a raw source value, which Ruby inserts
/// verbatim): character references are `Raw`, the rest is text.
pub(crate) fn reference_parts(value: &str) -> Vec<crate::AttributePart> {
    let mut parts = Vec::new();
    let mut copied = 0;
    let mut at = 0;
    while let Some(offset) = value[at..].find('&') {
        let start = at + offset;
        match character_reference_end(value, start) {
            Some(end) => {
                crate::attributes::push_part(&mut parts, &value[copied..start], false);
                crate::attributes::push_part(&mut parts, &value[start..end], true);
                copied = end;
                at = end;
            }
            None => at = start + 1,
        }
    }
    crate::attributes::push_part(&mut parts, &value[copied..], false);
    parts
}

/// [`reference_parts`] as inline nodes over `span`.
pub(crate) fn reference_inlines(value: &str, span: crate::Span) -> Vec<crate::ast::Inline> {
    reference_parts(value)
        .into_iter()
        .map(|part| match part {
            crate::AttributePart::Raw(value) => crate::ast::Inline::Raw { value, span },
            part => crate::ast::Inline::Text {
                value: part.text().to_owned(),
                span,
            },
        })
        .collect()
}

/// Ruby's `[refid]` label written from the refid's lexical spelling: the tags
/// earlier substitutions generated stay markup (source `<` is protected as
/// `&lt;` there), the rest reads like [`reference_inlines`] after decoding.
pub(crate) fn lexical_reference_inlines(value: &str, span: crate::Span) -> Vec<crate::ast::Inline> {
    let mut out = Vec::new();
    let mut rest = value;
    while !rest.is_empty() {
        if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                out.push(crate::ast::Inline::Raw {
                    value: rest[..=end].to_owned(),
                    span,
                });
                rest = &rest[end + 1..];
                continue;
            }
        }
        let end = rest[1..].find('<').map_or(rest.len(), |at| at + 1);
        out.extend(reference_inlines(&decode_protection(&rest[..end]), span));
        rest = &rest[end..];
    }
    out
}

/// The `[refid]` label of a reference to an unknown id: from the lexical
/// spelling when `lexical` (Ruby writes it verbatim), else from `id`.
pub(crate) fn unknown_reference_label(
    xref: &crate::Xref,
    id: &str,
    lexical: bool,
    span: crate::Span,
) -> Vec<crate::ast::Inline> {
    let spelling = if lexical {
        xref.refid.as_deref()
    } else {
        xref.spelling.as_deref()
    };
    match spelling {
        Some(spelling) => lexical_reference_inlines(&format!("[{spelling}]"), span),
        None => reference_inlines(&format!("[{id}]"), span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AttributePart;

    #[test]
    fn decoding_keeps_references_and_ambiguous_ampersands() {
        assert_eq!(decode_protection("a&lt;b&gt;c&amp;d"), "a<b>c&d");
        assert_eq!(
            decode_protection("&#38; &copy; &#x26;"),
            "&#38; &copy; &#x26;"
        );
        // The literal text `&#38;` keeps its protection, unlike `&amp;` alone.
        assert_eq!(
            decode_protection("&amp;#38; &amp;copy; &amp;"),
            "&amp;#38; &amp;copy; &"
        );
        assert_eq!(decode_protection("&amp;lt; &amp;a; Ж&"), "&amp;lt; &a; Ж&");
    }

    #[test]
    fn reference_parts_split_only_grammar_references() {
        assert_eq!(
            reference_parts("Ж&#38;x &copy &a; &amp;#38;"),
            vec![
                AttributePart::Text("Ж".into()),
                AttributePart::Raw("&#38;".into()),
                AttributePart::Text("x &copy &a; ".into()),
                AttributePart::Raw("&amp;".into()),
                AttributePart::Text("#38;".into()),
            ]
        );
        assert!(reference_parts("").is_empty());
        assert_eq!(character_reference_end("x&#x41;", 1), Some(7));
        assert_eq!(character_reference_end("x&#x41", 1), None);
        assert_eq!(character_reference_end("x", 0), None);
    }
}

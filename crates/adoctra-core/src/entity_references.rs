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

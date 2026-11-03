//! Plain text from the HTML-flavoured values core keeps: `Raw` inline values
//! (character references, passthrough markup) and lexical fields (link
//! targets with entity protection). WordprocessingML has no entities or
//! markup in text, so references are decoded and tags dropped.

/// Decode the character references of `value` (numeric and the HTML names
/// Asciidoctor content uses); an unknown name stays as written.
pub fn decode_references(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match adoctra_core::character_reference_end(rest, 0)
            .and_then(|end| Some((reference(&rest[1..end - 1])?, end)))
        {
            Some((ch, end)) => {
                out.push(ch);
                rest = &rest[end..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The character of reference `name` (without `&` and `;`).
fn reference(name: &str) -> Option<char> {
    if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        return u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
    }
    if let Some(dec) = name.strip_prefix('#') {
        return dec.parse().ok().and_then(char::from_u32);
    }
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "shy" => '\u{ad}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "middot" => '·',
        "bull" => '•',
        "sect" => '§',
        "para" => '¶',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "sbquo" => '\u{201a}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "bdquo" => '\u{201e}',
        "lsaquo" => '\u{2039}',
        "rsaquo" => '\u{203a}',
        "ndash" => '\u{2013}',
        "mdash" => '\u{2014}',
        "hellip" => '\u{2026}',
        "prime" => '\u{2032}',
        "Prime" => '\u{2033}',
        "larr" => '\u{2190}',
        "uarr" => '\u{2191}',
        "rarr" => '\u{2192}',
        "darr" => '\u{2193}',
        "harr" => '\u{2194}',
        "lArr" => '\u{21d0}',
        "rArr" => '\u{21d2}',
        "hArr" => '\u{21d4}',
        "thinsp" => '\u{2009}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "zwsp" => '\u{200b}',
        "zwj" => '\u{200d}',
        "zwnj" => '\u{200c}',
        "check" => '\u{2713}',
        "cross" => '\u{2717}',
        _ => return None,
    })
}

/// The text of a raw HTML value: tags dropped, references decoded.
pub fn raw_text(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                text.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    text.push_str(rest);
    decode_references(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_numeric_and_named_references() {
        assert_eq!(
            decode_references("a&#169;b&#x2014;c&nbsp;d&rsquo;"),
            "a©b—c\u{a0}d\u{2019}"
        );
        assert_eq!(decode_references("&amp;&lt;x&gt;"), "&<x>");
        assert_eq!(decode_references("&unknown; & &#;"), "&unknown; & &#;");
    }

    #[test]
    fn drops_tags_of_raw_values() {
        assert_eq!(
            raw_text("<b>bold</b> &amp; <i class=\"x\">it</i>"),
            "bold & it"
        );
        assert_eq!(raw_text("a < b"), "a < b");
    }
}

//! Just enough XML for OOXML parts: escaping for the writer and a tolerant
//! tag scanner for reading a template.
//!
//! The scanner does not build a tree. It reports each tag with its byte range
//! so callers can read names and attributes (style ids, SDT tags, relationship
//! ids) and splice new markup into a part without re-serializing the rest:
//! a template's parts stay byte-for-byte what the designer saved.

use std::ops::Range;

/// Escape character data.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            // XML 1.0 forbids most control characters, even escaped.
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Escape an attribute value written in double quotes.
pub fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Decode the predefined and numeric character references.
pub fn unescape(value: &str) -> String {
    if !value.contains('&') {
        return value.to_string();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(ch) => {
                out.push(ch);
                rest = &rest[end + 1..];
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

/// Whether a tag opens, closes or is self-closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKind {
    Start,
    End,
    Empty,
}

/// One tag of a part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag<'a> {
    /// The qualified name as written (`w:p`).
    pub name: &'a str,
    pub kind: TagKind,
    /// The byte range of the whole tag, `<` to `>`.
    pub range: Range<usize>,
    attrs: &'a str,
}

impl<'a> Tag<'a> {
    /// An attribute's decoded value by qualified name.
    pub fn attr(&self, name: &str) -> Option<String> {
        attributes(self.attrs)
            .find(|(key, _)| *key == name)
            .map(|(_, value)| unescape(value))
    }

    /// Whether the tag opens or is self-closing.
    pub fn opens(&self) -> bool {
        self.kind != TagKind::End
    }
}

fn attributes(mut s: &str) -> impl Iterator<Item = (&str, &str)> {
    std::iter::from_fn(move || {
        s = s.trim_start();
        let eq = s.find('=')?;
        let key = s[..eq].trim();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let close = after[1..].find(quote)?;
        let value = &after[1..1 + close];
        s = &after[close + 2..];
        Some((key, value))
    })
}

/// Iterate the tags of `xml` from byte `from`, skipping comments, processing
/// instructions, CDATA and the doctype. Malformed markup ends the scan.
pub fn tags(xml: &str, from: usize) -> impl Iterator<Item = Tag<'_>> {
    let mut pos = from;
    std::iter::from_fn(move || loop {
        let start = pos + xml.get(pos..)?.find('<')?;
        let rest = &xml[start..];
        let skip_to = |end: &str| rest.find(end).map(|i| start + i + end.len());
        if rest.starts_with("<!--") {
            pos = skip_to("-->")?;
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            pos = skip_to("]]>")?;
            continue;
        }
        if rest.starts_with("<?") {
            pos = skip_to("?>")?;
            continue;
        }
        if rest.starts_with("<!") {
            pos = skip_to(">")?;
            continue;
        }
        let end = start + tag_end(rest)? + 1;
        pos = end;
        let inner = &xml[start + 1..end - 1];
        let (kind, inner) = if let Some(name) = inner.strip_prefix('/') {
            (TagKind::End, name)
        } else if let Some(body) = inner.strip_suffix('/') {
            (TagKind::Empty, body)
        } else {
            (TagKind::Start, inner)
        };
        let name_end = inner
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(inner.len());
        return Some(Tag {
            name: &inner[..name_end],
            kind,
            range: start..end,
            attrs: &inner[name_end..],
        });
    })
}

/// The index of the `>` closing the tag at the start of `s`, outside quotes.
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (i, b) in s.bytes().enumerate() {
        match (quote, b) {
            (None, b'"' | b'\'') => quote = Some(b),
            (Some(q), _) if q == b => quote = None,
            (None, b'>') => return Some(i),
            _ => {}
        }
    }
    None
}

/// The element opened by `open` (a start or empty tag): its full byte range
/// and the range of its content (empty for a self-closing tag).
pub fn element(xml: &str, open: &Tag<'_>) -> Option<(Range<usize>, Range<usize>)> {
    if open.kind == TagKind::Empty {
        return Some((open.range.clone(), open.range.end..open.range.end));
    }
    let mut depth = 0usize;
    for tag in tags(xml, open.range.end) {
        if tag.name != open.name {
            continue;
        }
        match tag.kind {
            TagKind::Start => depth += 1,
            TagKind::End if depth == 0 => {
                return Some((
                    open.range.start..tag.range.end,
                    open.range.end..tag.range.start,
                ));
            }
            TagKind::End => depth -= 1,
            TagKind::Empty => {}
        }
    }
    None
}

/// The first element named `name` at or after `from`.
pub fn find_element(xml: &str, from: usize, name: &str) -> Option<(Range<usize>, Range<usize>)> {
    let open = tags(xml, from).find(|tag| tag.name == name && tag.opens())?;
    element(xml, &open)
}

/// The child elements of the content range `content`: the opening tag of
/// each direct child with its full range.
pub fn children<'a>(xml: &'a str, content: Range<usize>) -> Vec<(Tag<'a>, Range<usize>)> {
    let mut out = Vec::new();
    let mut pos = content.start;
    while let Some(tag) = tags(xml, pos).next() {
        if tag.range.start >= content.end || !tag.opens() {
            break;
        }
        let Some((range, _)) = element(xml, &tag) else {
            break;
        };
        pos = range.end;
        out.push((tag, range));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_text_and_attributes() {
        assert_eq!(escape_text("a<b>&c\u{1}"), "a&lt;b&gt;&amp;c");
        assert_eq!(escape_attr("x\"y\n"), "x&quot;y&#10;");
    }

    #[test]
    fn unescapes_references() {
        assert_eq!(unescape("a&amp;b&#65;&#x42;&bogus;"), "a&bAB&bogus;");
    }

    #[test]
    fn scans_tags_with_ranges_and_attributes() {
        let xml = r#"<?xml version="1.0"?><!-- c --><a x="1 > 2"><b y='q'/></a>"#;
        let found: Vec<_> = tags(xml, 0).collect();
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].name, "a");
        assert_eq!(found[0].kind, TagKind::Start);
        assert_eq!(found[0].attr("x").as_deref(), Some("1 > 2"));
        assert_eq!(&xml[found[1].range.clone()], "<b y='q'/>");
        assert_eq!(found[1].kind, TagKind::Empty);
        assert_eq!(found[2].kind, TagKind::End);
    }

    #[test]
    fn finds_nested_elements_of_the_same_name() {
        let xml = "<r><s><s>in</s></s><t/></r>";
        let (outer, content) = find_element(xml, 0, "s").unwrap();
        assert_eq!(&xml[outer], "<s><s>in</s></s>");
        assert_eq!(&xml[content], "<s>in</s>");
        let (_, r) = find_element(xml, 0, "r").unwrap();
        let kids: Vec<_> = children(xml, r).into_iter().map(|(t, _)| t.name).collect();
        assert_eq!(kids, ["s", "t"]);
    }

    #[test]
    fn malformed_markup_ends_the_scan() {
        assert_eq!(tags("<a x=\"open", 0).count(), 0);
        assert!(find_element("<a>", 0, "a").is_none());
    }
}

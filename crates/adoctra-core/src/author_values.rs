//! Effective author fields. Source declarations remain in header::parse_authors.
//! Literal characters and passthrough markup must stay distinct during derivation.
use crate::attributes::{parts_text, push_part};
use crate::{AttributePart, Attributes};

#[derive(Clone, Copy)]
struct NameChar {
    ch: char,
    raw: bool,
    escaped: Option<(char, usize, usize)>,
}

fn chars(parts: &[AttributePart]) -> Vec<NameChar> {
    let mut result = Vec::new();
    for part in parts {
        for ch in part.text().chars() {
            let entity = if part.is_raw() {
                None
            } else {
                match ch {
                    '<' => Some("&lt;"),
                    '>' => Some("&gt;"),
                    '&' => Some("&amp;"),
                    _ => None,
                }
            };
            if let Some(entity) = entity {
                for (index, encoded) in entity.chars().enumerate() {
                    result.push(NameChar {
                        ch: encoded,
                        raw: false,
                        escaped: Some((ch, index, entity.len())),
                    });
                }
            } else {
                result.push(NameChar {
                    ch,
                    raw: part.is_raw(),
                    escaped: None,
                });
            }
        }
    }
    result
}

fn parts(chars: &[NameChar]) -> Vec<AttributePart> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some((original, 0, len)) = c.escaped {
            if chars.get(i..i + len).is_some_and(|seq| {
                seq.iter()
                    .enumerate()
                    .all(|(index, c)| c.escaped == Some((original, index, len)))
            }) {
                push_part(&mut result, &original.to_string(), false);
                i += len;
                continue;
            }
        }
        // A delimiter can split an entity in the reference's lexical view.
        push_part(&mut result, &c.ch.to_string(), c.raw || c.escaped.is_some());
        i += 1;
    }
    result
}

fn space(c: NameChar) -> bool {
    matches!(c.ch, ' ' | '\t'..='\r')
}

pub(crate) fn split_authors(value: &[AttributePart], multiple: bool) -> Vec<Vec<AttributePart>> {
    if !multiple {
        return vec![value.to_vec()];
    }
    let value = chars(value);
    let mut entries = Vec::new();
    let mut start = 0;
    for i in 0..value.len() {
        if value[i].ch == ';' && (i + 1 == value.len() || value[i + 1].ch == ' ') {
            entries.push(parts(&value[start..i]));
            start = (i + 2).min(value.len());
        }
    }
    if start < value.len() {
        entries.push(parts(&value[start..]));
    }
    entries
}

pub(crate) fn derive_authors(entries: Vec<Vec<AttributePart>>) -> Attributes {
    let mut result = Attributes::new();
    let mut names = Vec::new();
    let mut count = 0;
    for entry in entries {
        if parts_text(&entry).is_empty() {
            continue;
        }
        count += 1;
        let fields = derive_author(&entry);
        if count > 1 {
            push_part(&mut names, ", ", false);
        }
        for part in fields.value_parts("author").unwrap() {
            push_part(&mut names, part.text(), part.is_raw());
        }
        if count == 2 {
            for key in [
                "author",
                "firstname",
                "middlename",
                "lastname",
                "authorinitials",
            ] {
                if let Some(parts) = result.value_parts(key) {
                    result.set_parts(format!("{key}_1"), parts);
                }
            }
        }
        for (key, _) in fields.iter() {
            let target = if count == 1 {
                key.to_owned()
            } else {
                format!("{key}_{count}")
            };
            result.set_parts(target, fields.value_parts(key).unwrap());
        }
    }
    if count > 0 {
        result.set_parts("authors", names);
    }
    result.set("authorcount", count.to_string());
    result
}

fn derive_author(value: &[AttributePart]) -> Attributes {
    let original = chars(value);
    let markup = original.iter().any(|c| c.raw && c.ch == '<');
    let mut clean = Vec::new();
    let mut i = 0;
    while i < original.len() {
        // Ruby XmlSanitizeRx: <[^>]+>. Empty and unclosed tags remain text.
        if original[i].raw && original[i].ch == '<' {
            if let Some(end) =
                (i + 1..original.len()).find(|&j| original[j].raw && original[j].ch == '>')
            {
                if end > i + 1 {
                    i = end + 1;
                    continue;
                }
            }
        }
        clean.push(original[i]);
        i += 1;
    }
    // Ruby split(nil, 3): ASCII whitespace, with the third field retaining its
    // whitespace except repeated literal spaces. NBSP is part of a name.
    let mut fields = Vec::new();
    i = 0;
    while i < clean.len() && fields.len() < 3 {
        while i < clean.len() && space(clean[i]) {
            i += 1;
        }
        if i == clean.len() {
            fields.push(Vec::new());
            break;
        }
        let start = i;
        if fields.len() == 2 {
            i = clean.len();
        } else {
            while i < clean.len() && !space(clean[i]) {
                i += 1;
            }
        }
        let mut field: Vec<NameChar> = Vec::new();
        let mut previous_space = false;
        for mut c in clean[start..i].iter().copied() {
            if c.ch == ' ' && previous_space {
                continue;
            }
            previous_space = c.ch == ' ';
            // Squeeze precedes underscore expansion in the reference.
            if c.ch == '_' {
                c.ch = ' ';
            }
            field.push(c);
        }
        fields.push(field);
    }
    // Ruby can raise for a tag-only empty name; editor parsing returns an empty
    // first name and initials while preserving the original effective author.
    if fields.is_empty() {
        fields.push(Vec::new());
    }
    let mut result = Attributes::new();
    let mut name = Vec::new();
    let mut initials = String::new();
    for (index, field) in fields.iter().enumerate() {
        let key = match (index, fields.len()) {
            (0, _) => "firstname",
            (1, 3) => "middlename",
            _ => "lastname",
        };
        result.set_parts(key, parts(field));
        if index > 0 {
            push_part(&mut name, " ", false);
        }
        for part in parts(field) {
            push_part(&mut name, part.text(), part.is_raw());
        }
        if let Some(c) = field.first() {
            initials.push(c.ch);
        }
    }
    if markup {
        name = value
            .iter()
            .map(|p| {
                if p.is_raw() {
                    AttributePart::Raw(p.text().replace('_', " "))
                } else {
                    AttributePart::Text(p.text().replace('_', " "))
                }
            })
            .collect();
    }
    result.set_parts("author", name);
    result.set("authorinitials", initials);
    result
}

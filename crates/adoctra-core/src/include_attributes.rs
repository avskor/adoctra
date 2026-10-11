//! AttributeList grammar used by the pinned PreprocessorReader. Substitution
//! happens on the whole list, before tokenization. Unlike block lists, include
//! lists do not request `sub_result`: single quotes only delimit a value.
use crate::Attributes;
use std::collections::BTreeMap;

pub(crate) fn parse(
    input: &str,
    attributes: &Attributes,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (input, warnings) = substitute(input, attributes);
    let mut rest = input.as_str();
    let mut named = BTreeMap::new();
    while !rest.is_empty() {
        rest = rest.trim_start_matches([' ', '\t']);
        if rest.is_empty() || rest.starts_with('\n') {
            break;
        }
        if rest.starts_with(['\'', '"']) {
            // Positional fields have no meaning to the built-in include reader.
            value(&mut rest);
        } else {
            let start = rest;
            let end = rest
                .char_indices()
                .find(|(i, c)| {
                    !c.is_alphanumeric() && *c != '_' && !(*i > 0 && matches!(c, '-' | '.'))
                })
                .map_or(rest.len(), |(i, _)| i);
            let name = &rest[..end];
            rest = rest[end..].trim_start_matches([' ', '\t']);
            if !name.is_empty() && rest.starts_with('=') {
                rest = rest[1..].trim_start_matches([' ', '\t']);
                let quoted = rest.starts_with(['\'', '"']);
                let value = value(&mut rest);
                if quoted || value != "None" {
                    if matches!(name, "opts" | "options") {
                        // Only a comma-separated options value deletes spaces;
                        // tabs and spaces in a single value remain significant.
                        if value.contains(',') {
                            for option in
                                value.replace(' ', "").split(',').filter(|s| !s.is_empty())
                            {
                                named.insert(format!("{option}-option"), String::new());
                            }
                        } else if !value.is_empty() {
                            named.insert(format!("{value}-option"), String::new());
                        }
                    } else {
                        named.insert(name.to_owned(), value);
                    }
                }
            } else {
                rest = start;
                unquoted(&mut rest);
            }
        }
        rest = rest.trim_start_matches([' ', '\t']);
        if let Some(next) = rest.strip_prefix(',') {
            rest = next;
        }
        // Ruby also accepts an adjacent field after a closing quote without a
        // comma. Each iteration consumes a field or delimiter, including blanks.
    }
    (named, warnings)
}

fn unquoted(rest: &mut &str) -> String {
    let end = rest.find([',', '\n']).unwrap_or(rest.len());
    let value = rest[..end].trim_end_matches([' ', '\t']).to_owned();
    *rest = &rest[end..];
    value
}

fn value(rest: &mut &str) -> String {
    let Some(quote @ (b'\'' | b'"')) = rest.as_bytes().first().copied() else {
        return unquoted(rest);
    };
    let body = &rest[1..];
    let close = body
        .bytes()
        .enumerate()
        .take_while(|(_, c)| *c != b'\n')
        .find(|(i, c)| *c == quote && (*i == 0 || body.as_bytes()[i - 1] != b'\\'));
    if let Some((end, _)) = close {
        let value = body[..end].replace(
            &format!("\\{}", quote as char),
            &(quote as char).to_string(),
        );
        *rest = &body[end + 1..];
        value
    } else {
        // An unmatched quote becomes literal, and commas delimit again.
        unquoted(rest)
    }
}

fn substitute(input: &str, attributes: &Attributes) -> (String, Vec<String>) {
    let mut output = String::new();
    let mut warnings = Vec::new();
    let mut dropped = Vec::new();
    let policy = attributes.get("attribute-missing").unwrap_or("skip");
    let mut chars = input.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        let escaped = c == '\\' && chars.peek().is_some_and(|(_, c)| *c == '{');
        if c != '{' && !escaped {
            output.push(c);
            continue;
        }
        let open = index + usize::from(escaped);
        let Some(close) = input[open + 1..].find('}').map(|i| open + 1 + i) else {
            output.push(c);
            continue;
        };
        let name = &input[open + 1..close];
        let trailing_escape = name.ends_with('\\');
        let name = name.strip_suffix('\\').unwrap_or(name);
        if name.is_empty()
            || !name
                .chars()
                .enumerate()
                .all(|(i, c)| c.is_alphanumeric() || c == '_' || (i > 0 && c == '-'))
        {
            output.push(c);
            continue;
        }
        if escaped || trailing_escape {
            output.push('{');
            output.push_str(name);
            output.push('}');
        } else if let Some(value) = attributes
            .get(name)
            .or_else(|| crate::context::intrinsic(&name.to_lowercase()))
        {
            output.push_str(value);
        } else {
            match policy {
                "drop" | "drop-line" => dropped.push(output.len()),
                _ => {
                    output.push_str(&input[open..=close]);
                    if policy == "warn" {
                        warnings.push(format!(
                            "skipping reference to missing attribute: {}",
                            name.to_lowercase()
                        ));
                    }
                }
            }
        }
        while chars.peek().is_some_and(|(i, _)| *i <= close) {
            chars.next();
        }
    }
    if !dropped.is_empty() {
        let mut start = 0;
        let mut positions = dropped.into_iter().peekable();
        output = output
            .split('\n')
            .filter(|line| {
                let end = start + line.len();
                let mut missing = false;
                while positions.peek().is_some_and(|i| *i <= end) {
                    positions.next();
                    missing = true;
                }
                start = end + 1;
                !missing || (policy != "drop-line" && !line.is_empty())
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    (output, warnings)
}

//! Include parameters use the shared AttributeList grammar. Substitution
//! happens on the whole list, before tokenization. Unlike block lists, include
//! lists do not request `sub_result`: single quotes only delimit a value.
use crate::Attributes;
use std::collections::BTreeMap;

pub(crate) fn parse(
    input: &str,
    attributes: &Attributes,
) -> (BTreeMap<String, String>, Vec<String>) {
    let (input, warnings) = substitute(
        input,
        attributes,
        attributes.get("attribute-missing").unwrap_or("skip"),
    );
    let mut named = BTreeMap::new();
    // Positional fields have no meaning to the built-in include reader.
    for field in crate::attribute_list::parse(&input) {
        let Some(name) = field.name(&input) else {
            continue;
        };
        let value = field.text(&input).unwrap_or_default();
        if matches!(name, "opts" | "options") {
            // Only a comma-separated options value deletes spaces;
            // tabs and spaces in a single value remain significant.
            if value.contains(',') {
                for option in value.replace(' ', "").split(',').filter(|s| !s.is_empty()) {
                    named.insert(format!("{option}-option"), String::new());
                }
            } else if !value.is_empty() {
                named.insert(format!("{value}-option"), String::new());
            }
        } else {
            named.insert(name.to_owned(), value);
        }
    }
    (named, warnings)
}

/// Shared, single-pass include substitutions. Targets override `warn` with
/// `drop-line`; parameter lists use the document policy unchanged.
pub(crate) fn substitute(
    input: &str,
    attributes: &Attributes,
    policy: &str,
) -> (String, Vec<String>) {
    let mut output = String::new();
    let mut warnings = Vec::new();
    let mut dropped = Vec::new();
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

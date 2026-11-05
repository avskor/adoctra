//! The image metadata asciidoctor-diagram stores next to its cache
//! (`<image>.cache`, `JSON.dump` of `{checksum, options, width, height}`).

use crate::image::Number;

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Metadata {
    pub checksum: Option<String>,
    /// The options object as JSON text (compared as written).
    pub options: Option<String>,
    pub width: Option<Number>,
    pub height: Option<Number>,
}

impl Metadata {
    pub fn to_json(&self) -> String {
        let mut fields = Vec::new();
        if let Some(checksum) = &self.checksum {
            fields.push(format!(
                "\"checksum\":{}",
                crate::converter::json_string(checksum)
            ));
        }
        fields.push(format!(
            "\"options\":{}",
            self.options.as_deref().unwrap_or("{}")
        ));
        let number = |n: Option<Number>| n.map_or("null".to_owned(), Number::json);
        fields.push(format!("\"width\":{}", number(self.width)));
        fields.push(format!("\"height\":{}", number(self.height)));
        format!("{{{}}}", fields.join(","))
    }

    /// Read what [`to_json`](Self::to_json) (or Ruby) wrote; anything
    /// unreadable is empty metadata, which regenerates the image.
    pub fn parse(text: &str) -> Self {
        let mut metadata = Metadata::default();
        let mut rest = text.trim();
        let Some(body) = rest.strip_prefix('{') else {
            return metadata;
        };
        rest = body;
        loop {
            rest = rest.trim_start().trim_start_matches(',').trim_start();
            let Some((key, after)) = json_string_token(rest) else {
                return metadata;
            };
            let Some(after) = after.trim_start().strip_prefix(':') else {
                return metadata;
            };
            let after = after.trim_start();
            let (value, next) = match value_end(after) {
                Some(end) => (&after[..end], &after[end..]),
                None => return metadata,
            };
            match key.as_str() {
                "checksum" => metadata.checksum = json_string_token(value).map(|(s, _)| s),
                "options" => metadata.options = Some(value.to_owned()),
                "width" => metadata.width = Number::parse(value),
                "height" => metadata.height = Number::parse(value),
                _ => {}
            }
            rest = next;
            if rest.trim_start().starts_with('}') {
                return metadata;
            }
        }
    }
}

/// A JSON string at the start of `text` and the rest after it.
fn json_string_token(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &body[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = (0..4)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The end of the JSON value at the start of `text`.
fn value_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in text.char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => {
                    in_string = false;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' if depth == 0 => return Some(i),
            '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            ',' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    (depth == 0 && !in_string).then_some(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_rubys_json() {
        let ruby = r#"{"checksum":"graphviz-md5-13328802c64df661e756f74bef55deb8","options":{"layout":null},"width":83,"height":155}"#;
        let metadata = Metadata::parse(ruby);
        assert_eq!(
            metadata.checksum.as_deref(),
            Some("graphviz-md5-13328802c64df661e756f74bef55deb8")
        );
        assert_eq!(metadata.options.as_deref(), Some(r#"{"layout":null}"#));
        assert_eq!(metadata.width, Some(Number::Int(83)));
        assert_eq!(metadata.to_json(), ruby);
        assert_eq!(Metadata::parse("garbage"), Metadata::default());
    }
}
